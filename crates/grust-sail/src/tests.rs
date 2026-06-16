use std::io::Cursor;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::RwLock;

use arrow::array::Array as _;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use grust_core::prelude::*;
use grust_memory::MemoryGraphStore;
use tonic::transport::Channel;

use super::*;

fn sail_available() -> bool {
    TcpStream::connect("127.0.0.1:50051").is_ok()
}

async fn store() -> SailGraphStore {
    assert!(
        sail_available(),
        "live Sail integration tests require a Sail server on 127.0.0.1:50051; run scripts/integration-test.sh --backend sail"
    );
    let store = SailGraphStore::connect(SailConfig::default())
        .await
        .expect("connect to Sail");
    store.bootstrap().await.expect("bootstrap Sail tables");
    store.clear().await.expect("clear Sail tables");
    store
}

fn request_store() -> SailGraphStore {
    SailGraphStore {
        config: SailConfig::default(),
        client: SparkConnectServiceClient::new(
            Channel::from_static("http://127.0.0.1:50051").connect_lazy(),
        ),
        schema: RwLock::new(None),
    }
}

fn query_string_rows(chunks: Vec<Vec<u8>>, columns: usize) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for chunk in chunks {
        let reader = StreamReader::try_new(Cursor::new(chunk), None).expect("Arrow IPC stream");
        for batch in reader {
            let batch = batch.expect("Arrow record batch");
            for row in 0..batch.num_rows() {
                let mut values = Vec::with_capacity(columns);
                for column in 0..columns {
                    let strings = batch
                        .column(column)
                        .as_any()
                        .downcast_ref::<StringArray>()
                        .expect("string column");
                    assert!(!strings.is_null(row), "unexpected null string value");
                    values.push(strings.value(row).to_string());
                }
                rows.push(values);
            }
        }
    }
    rows
}

fn is_cypher_planning_error(error: &GrustError) -> bool {
    matches!(
        error,
        GrustError::CypherSyntax(_)
            | GrustError::CypherUnresolvedIdentity(_)
            | GrustError::CypherUnsupportedCardinality(_)
            | GrustError::Unsupported(_)
    )
}

fn sample_graph() -> Graph {
    let mut b = Graph::builder();
    let _ = b
        .node("Person", "person-1")
        .prop("name", "Ada Lovelace")
        .prop("age", 36i64)
        .finish();
    let _ = b
        .node("Talk", "talk-1")
        .prop("title", "Analytical Engine")
        .finish();
    let _ = b.edge("presents", "person-1", "talk-1").finish();
    b.build()
}

fn person_schema() -> GraphSchema {
    GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("name", FieldType::String),
                Field::optional("age", FieldType::Int),
            ],
        )
        .edge(
            "presents",
            vec![Label::new("Person")],
            vec![Label::new("Talk")],
            vec![Field::optional("source", FieldType::String)],
        )
        .build()
}

#[test]
fn schema_sql_creates_typed_delta_tables() {
    let sql = sail_schema_sql(&person_schema()).unwrap();

    assert!(sql.iter().any(|statement| statement.contains(
        "CREATE TABLE IF NOT EXISTS grust_node_person (id STRING NOT NULL, `name` STRING, `age` BIGINT) USING delta"
    )));
    assert!(sql.iter().any(|statement| {
        statement.contains("'grust.graph.kind' = 'node'")
            && statement.contains("'grust.graph.label' = 'Person'")
    }));
    assert!(sql.iter().any(|statement| {
        statement.contains("CREATE TABLE IF NOT EXISTS grust_edge_presents")
            && statement.contains("`source` STRING")
    }));
    assert!(sql.iter().any(|statement| {
        statement.contains("'grust.graph.kind' = 'edge'")
            && statement.contains("'grust.graph.label' = 'presents'")
    }));
}

#[test]
fn graph_schema_typed_table_contract_is_public() {
    let tables = sail_graph_schema_typed_tables(&person_schema()).unwrap();

    assert!(tables.contains(&SailGraphTypedTable {
        kind: SailGraphTypedTableKind::Node,
        label: "Person".to_string(),
        table: "grust_node_person".to_string(),
        columns: vec!["id".to_string(), "name".to_string(), "age".to_string()],
    }));
    assert!(tables.contains(&SailGraphTypedTable {
        kind: SailGraphTypedTableKind::Edge,
        label: "presents".to_string(),
        table: "grust_edge_presents".to_string(),
        columns: vec![
            "edge_key".to_string(),
            "id".to_string(),
            "src_id".to_string(),
            "dst_id".to_string(),
            "source".to_string(),
        ],
    }));

    let schema = person_schema();
    assert_eq!(
        sail_typed_node_columns(schema.node_type(&Label::new("Person")).unwrap()).unwrap(),
        ["id", "name", "age"]
    );
    assert_eq!(
        sail_typed_edge_columns(schema.edge_type(&Label::new("presents")).unwrap()).unwrap(),
        ["edge_key", "id", "src_id", "dst_id", "source"]
    );
}

#[test]
fn typed_node_merge_extracts_fields_from_staged_json() {
    let schema = person_schema();
    let node_type = schema.node_type(&Label::new("Person")).unwrap();

    let sql = typed_node_merge_from_view_sql(node_type).unwrap();

    assert!(sql.contains("MERGE INTO grust_node_person"));
    assert!(sql.contains("FROM grust_stage_nodes s WHERE s.label = 'Person'"));
    assert!(sql.contains("GET_JSON_OBJECT(s.props, '$.name') AS `name`"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(s.props, '$.age') AS BIGINT) AS `age`"));
}

#[test]
fn typed_edge_merge_extracts_fields_from_staged_json() {
    let schema = person_schema();
    let edge_type = schema.edge_type(&Label::new("presents")).unwrap();

    let sql = typed_edge_merge_from_view_sql(edge_type).unwrap();

    assert!(sql.contains("MERGE INTO grust_edge_presents"));
    assert!(sql.contains("FROM grust_stage_edges s WHERE s.edge_type = 'presents'"));
    assert!(sql.contains("ON t.edge_key = s.edge_key"));
    assert!(sql.contains("GET_JSON_OBJECT(s.props, '$.source') AS `source`"));
}

#[test]
fn generic_edge_merge_persists_edge_identity_columns() {
    let sql = merge_edges_from_view_sql();

    assert!(sql.contains("t.edge_key = s.edge_key"));
    assert!(sql.contains("t.id = s.id"));
    assert!(
        sql.contains(
            "INSERT (edge_key, id, src_id, src_label, dst_id, dst_label, edge_type, props)"
        )
    );
    assert!(sql.contains(
        "VALUES (s.edge_key, s.id, s.src_id, s.src_label, s.dst_id, s.dst_label, s.edge_type, s.props)"
    ));
}

#[test]
fn cypher_mutation_options_default_to_upsert_compatible_create() {
    assert_eq!(
        CypherMutationOptions::default(),
        CypherMutationOptions {
            create_mode: CypherCreateMode::UpsertCompatible,
            node_id_policy: CypherNodeIdPolicy::ExplicitOnly,
            collect_written_edge_identities: false,
            null_assignment: CypherNullAssignment::StoreNull,
            parameters: CypherParameters::new(),
        }
    );
}

#[test]
fn cypher_parser_classifies_top_level_mutation_statements() {
    use super::cypher_parser::CypherStatement;

    assert_eq!(
        super::cypher_parser::classify_statement("MATCH (n) DELETE n").unwrap(),
        CypherStatement::Match("(n) DELETE n")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("create (:Person {id: 'p'})").unwrap(),
        CypherStatement::Create("(:Person {id: 'p'})")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("MERGE (:Person {id: 'p'})").unwrap(),
        CypherStatement::Merge("(:Person {id: 'p'})")
    );
    assert_eq!(
        super::cypher_parser::classify_statement("DELETE (:Person {id: 'p'})").unwrap(),
        CypherStatement::Delete("(:Person {id: 'p'})")
    );

    let error =
        super::cypher_parser::classify_statement("SET n.name = 'Ada'").expect_err("bare SET");
    assert!(matches!(error, GrustError::CypherSyntax(_)));

    let error = super::cypher_parser::classify_statement("RETURN 1").expect_err("read query");
    assert!(matches!(error, GrustError::CypherSyntax(_)));
}

#[test]
fn strict_create_edge_conflicts_on_sail_write_identity() {
    let structural = Edge::new("KNOWS", "person-1", "person-2", Props::new());
    let explicit = Edge::new("KNOWS", "person-1", "person-2", Props::new()).with_id("edge-1");
    let same_id_elsewhere =
        Edge::new("KNOWS", "person-3", "person-4", Props::new()).with_id("edge-1");
    let same_structural_different_id =
        Edge::new("KNOWS", "person-1", "person-2", Props::new()).with_id("edge-2");
    let unrelated = Edge::new("KNOWS", "person-2", "person-3", Props::new()).with_id("edge-3");

    assert!(strict_create_edge_conflicts(
        &structural,
        &[same_structural_different_id.clone()]
    ));
    assert!(strict_create_edge_conflicts(
        &explicit,
        &[same_id_elsewhere]
    ));
    assert!(strict_create_edge_conflicts(
        &explicit,
        &[same_structural_different_id]
    ));
    assert!(!strict_create_edge_conflicts(&explicit, &[unrelated]));
}

#[test]
fn staged_node_batch_round_trips_through_arrow_ipc() {
    let graph = sample_graph();
    let batch = nodes_record_batch(&graph.nodes).unwrap();
    let bytes = ipc_bytes(&batch).unwrap();

    // The staging schema matches grust_nodes, so the read path parses it.
    let nodes = parse_nodes_from_arrow(&bytes).unwrap();

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0].id, NodeId::new("person-1"));
    assert_eq!(nodes[0].label, Label::new("Person"));
    assert_eq!(
        nodes[0].props.get("name"),
        Some(&Value::from("Ada Lovelace"))
    );
    assert_eq!(nodes[0].props.get("age"), Some(&Value::from(36i64)));
}

#[test]
fn staged_edge_batch_round_trips_through_arrow_ipc() {
    let graph = sample_graph();
    let node_labels: std::collections::BTreeMap<&NodeId, &Label> = graph
        .nodes
        .iter()
        .map(|node| (&node.id, &node.label))
        .collect();
    let batch = edges_record_batch(&graph.edges, &node_labels).unwrap();
    let bytes = ipc_bytes(&batch).unwrap();

    let edges = parse_edges_from_arrow(&bytes).unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].from, NodeId::new("person-1"));
    assert_eq!(edges[0].to, NodeId::new("talk-1"));
    assert_eq!(edges[0].label, Label::new("presents"));
}

#[test]
fn staged_edge_batch_preserves_explicit_arrow_edge_id() {
    let from = NodeId::new("person-1");
    let to = NodeId::new("talk-1");
    let from_label = Label::new("Person");
    let to_label = Label::new("Talk");
    let edge = Edge::new("presents", from.as_str(), to.as_str(), Props::new()).with_id("edge-1");
    let mut node_labels = std::collections::BTreeMap::new();
    node_labels.insert(&from, &from_label);
    node_labels.insert(&to, &to_label);

    let batch = edges_record_batch(&[edge], &node_labels).unwrap();
    let bytes = ipc_bytes(&batch).unwrap();
    let edges = parse_edges_from_arrow(&bytes).unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id.as_ref().map(EdgeId::as_str), Some("edge-1"));
}

#[test]
fn generic_edge_arrow_results_preserve_explicit_edge_id() {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("id", DataType::Utf8, true),
        ArrowField::new("src_id", DataType::Utf8, false),
        ArrowField::new("src_label", DataType::Utf8, false),
        ArrowField::new("dst_id", DataType::Utf8, false),
        ArrowField::new("dst_label", DataType::Utf8, false),
        ArrowField::new("edge_type", DataType::Utf8, false),
        ArrowField::new("props", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from(vec![Some("edge-1")])),
            Arc::new(StringArray::from_iter_values(["person-1"])),
            Arc::new(StringArray::from_iter_values(["Person"])),
            Arc::new(StringArray::from_iter_values(["talk-1"])),
            Arc::new(StringArray::from_iter_values(["Talk"])),
            Arc::new(StringArray::from_iter_values(["presents"])),
            Arc::new(StringArray::from_iter_values([r#"{"source":"schedule"}"#])),
        ],
    )
    .unwrap();
    let bytes = ipc_bytes(&batch).unwrap();

    let edges = parse_edges_from_arrow(&bytes).unwrap();

    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id.as_ref().map(EdgeId::as_str), Some("edge-1"));
    assert_eq!(
        edges[0].props.get("source"),
        Some(&Value::String("schedule".into()))
    );
}

#[test]
fn arrow_temp_view_names_must_be_safe_lower_snake_identifiers() {
    assert!(validate_arrow_view_name("people_arrow").is_ok());
    assert!(validate_arrow_view_name("PeopleArrow").is_err());
    assert!(validate_arrow_view_name("people arrow").is_err());
}

#[test]
fn props_json_is_plain_and_reads_legacy_tagged_form() {
    let mut props = Props::new();
    props.insert("name".to_string(), Value::from("Ada"));
    props.insert("age".to_string(), Value::from(36i64));

    let json = props_to_json(&props).unwrap();
    assert_eq!(json, r#"{"age":36,"name":"Ada"}"#);

    let parsed = props_from_json(&json).unwrap();
    assert_eq!(parsed, props);

    let legacy = r#"{"name":{"type":"string","value":"Ada"},"age":{"type":"int","value":36}}"#;
    let parsed = props_from_json(legacy).unwrap();
    assert_eq!(parsed, props);
}

#[test]
fn props_json_rejects_non_finite_floats() {
    let mut props = Props::new();
    props.insert("score".to_string(), Value::Float(f64::NAN));
    let err = props_to_json(&props).expect_err("NaN must be rejected");
    assert!(err.to_string().contains("non-finite"));
}

#[test]
fn traversal_sql_joins_edges_by_id_and_binds_args() {
    // Edges staged without the full graph in scope carry empty
    // src_label/dst_label, so traversal joins must match on ids alone.
    let (sql, args) =
        traversal_sql(&Traversal::from_node("person-1").out("presents").to("Talk")).unwrap();

    assert!(sql.contains("JOIN grust_edges e0 ON e0.src_id = n0.id AND e0.edge_type = ?"));
    assert!(sql.contains("JOIN grust_nodes n1 ON n1.id = e0.dst_id AND n1.label = ?"));
    assert!(sql.contains("WHERE n0.id = ?"));
    assert!(
        !sql.contains("src_label") && !sql.contains("dst_label"),
        "traversal must not join on edge labels: {sql}"
    );
    assert_eq!(
        args,
        vec![lit_str("presents"), lit_str("Talk"), lit_str("person-1")]
    );

    let (sql, _) = traversal_sql(&Traversal::from_node("person-1").in_("presents")).unwrap();
    assert!(sql.contains("JOIN grust_edges e0 ON e0.dst_id = n0.id"));
    assert!(sql.contains("JOIN grust_nodes n1 ON n1.id = e0.src_id"));

    let (sql, _) = traversal_sql(&Traversal::from_node("person-1").both("presents")).unwrap();
    assert!(sql.contains("(e0.src_id = n0.id OR e0.dst_id = n0.id)"));
    assert!(sql.contains("CASE WHEN e0.src_id = n0.id THEN e0.dst_id ELSE e0.src_id END"));
}

#[test]
fn start_clause_binds_values_and_rejects_unsafe_json_keys() {
    let start = Start::NodesByProperty {
        label: Label::new("Person"),
        key: "name') = '' OR ('1'='1".to_string(),
        value: Value::from("x"),
    };
    let err = start_clause(&start, "n0").expect_err("unsafe key must be rejected");
    assert!(err.to_string().contains("invalid JSON property key"));

    let start = Start::NodesByProperty {
        label: Label::new("Person"),
        key: "age".to_string(),
        value: Value::from(36i64),
    };
    let (clause, args) = start_clause(&start, "n0").unwrap();
    assert!(clause.contains("n0.label = ?"));
    assert!(clause.contains("CAST(GET_JSON_OBJECT(n0.props, '$.age') AS BIGINT) = ?"));
    assert_eq!(args, vec![lit_str("Person"), lit_long(36)]);
}

#[test]
fn sql_str_escapes_backslashes_and_quotes() {
    assert_eq!(sql_str("plain"), "'plain'");
    assert_eq!(sql_str("it's"), "'it''s'");
    assert_eq!(sql_str(r"back\slash"), r"'back\\slash'");
    assert_eq!(sql_str(r"trailing\"), r"'trailing\\'");
    assert_eq!(sql_str(r"a\'b"), r"'a\\''b'");
}

#[tokio::test]
async fn query_request_sends_named_arguments_without_inlining() {
    let request = request_store()
        .query_request(
            "SELECT * FROM grust_nodes WHERE id = ? AND label = ?",
            vec![lit_str("person-1"), lit_str("Person")],
        )
        .unwrap();
    let Some(Plan {
        op_type:
            Some(plan::OpType::Root(Relation {
                rel_type: Some(relation::RelType::Sql(sql)),
                ..
            })),
        ..
    }) = request.plan
    else {
        panic!("expected SQL relation plan");
    };

    assert_eq!(
        sql.query,
        "SELECT * FROM grust_nodes WHERE id = :p1 AND label = :p2"
    );
    assert!(sql.args.is_empty());
    assert!(sql.pos_arguments.is_empty());
    assert_eq!(sql.named_arguments.len(), 2);
    assert!(matches!(
        sql.named_arguments["p1"].expr_type,
        Some(expression::ExprType::Literal(_))
    ));
}

#[tokio::test]
async fn query_request_rejects_placeholder_argument_mismatches() {
    assert!(
        request_store()
            .query_request("SELECT * FROM grust_nodes WHERE id = ?", vec![])
            .is_err()
    );
    assert!(
        request_store()
            .query_request("SELECT * FROM grust_nodes", vec![lit_str("person-1")])
            .is_err()
    );
}

#[tokio::test]
async fn sql_arguments_are_named_for_sail_parser() {
    let request = request_store()
        .query_request(
            "SELECT * FROM grust_edges WHERE from_id = ?",
            vec![lit_str("person-1")],
        )
        .unwrap();
    let Some(Plan {
        op_type:
            Some(plan::OpType::Root(Relation {
                rel_type: Some(relation::RelType::Sql(sql)),
                ..
            })),
        ..
    }) = request.plan
    else {
        panic!("expected SQL relation plan");
    };

    assert_eq!(sql.query, "SELECT * FROM grust_edges WHERE from_id = :p1");
    assert!(sql.pos_arguments.is_empty());
    assert_eq!(sql.named_arguments.len(), 1);
}

#[test]
fn clear_sql_drops_delta_tables_for_robust_reset() {
    assert_eq!(DROP_NODES_SQL, "DROP TABLE IF EXISTS grust_nodes");
    assert_eq!(DROP_EDGES_SQL, "DROP TABLE IF EXISTS grust_edges");
}

#[test]
fn generic_degree_sql_uses_persisted_sail_graph_tables() {
    assert_eq!(
        sail_out_degrees_sql(),
        "SELECT src_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY src_id"
    );
    assert_eq!(
        sail_in_degrees_sql(),
        "SELECT dst_id AS id, COUNT(*) AS degree FROM grust_edges GROUP BY dst_id"
    );
    assert!(sail_degrees_sql().contains("UNION ALL"));
    assert!(sail_degree_pairs_sql().contains("FROM grust_nodes n"));
    assert!(sail_degree_pairs_sql().contains("LEFT JOIN"));
    assert!(sail_triplets_sql().contains("FROM grust_edges e"));
    assert!(sail_triplets_sql().contains("JOIN grust_nodes src ON src.id = e.src_id"));
    assert!(sail_triplets_sql().contains("JOIN grust_nodes dst ON dst.id = e.dst_id"));
    assert_eq!(
        sail_triplets_sql(),
        sail_triplets_sql_for_direction(SailGraphPatternDirection::Outgoing)
    );
    let incoming = sail_triplets_sql_for_direction(SailGraphPatternDirection::Incoming);
    assert!(incoming.contains("dst.id AS src_id"));
    assert!(incoming.contains("src.id AS dst_id"));
    let undirected = sail_triplets_sql_for_direction(SailGraphPatternDirection::Undirected);
    assert!(undirected.contains("UNION ALL"));
    assert!(undirected.contains("src.id AS src_id"));
    assert!(undirected.contains("dst.id AS src_id"));
}

#[test]
fn graph_table_contract_helpers_are_shared() {
    assert_eq!(GRUST_NODES_TABLE, "grust_nodes");
    assert_eq!(GRUST_EDGES_TABLE, "grust_edges");
    assert_eq!(GRAPH_TABLE_KIND_PROPERTY, "grust.graph.kind");
    assert_eq!(GRAPH_TABLE_LABEL_PROPERTY, "grust.graph.label");
    assert_eq!(GRAPH_TABLE_KIND_NODE, "node");
    assert_eq!(GRAPH_TABLE_KIND_EDGE, "edge");
    assert_eq!(
        sail_node_field_projection("id"),
        SailGraphFieldProjection::PhysicalColumn(NODE_ID_COLUMN)
    );
    assert_eq!(
        sail_node_field_projection("name"),
        SailGraphFieldProjection::JsonProperty("name".to_string())
    );
    assert_eq!(
        sail_edge_field_projection("label"),
        SailGraphFieldProjection::PhysicalColumn(EDGE_TYPE_COLUMN)
    );
    assert_eq!(
        sail_edge_field_projection("since"),
        SailGraphFieldProjection::JsonProperty("since".to_string())
    );
    assert_eq!(
        sail_json_property_expr("n.props", "age").unwrap(),
        "GET_JSON_OBJECT(n.props, '$.age')"
    );
    assert!(sail_json_property_expr("n.props", "age') = 1 OR ('1'='1").is_err());
    assert_eq!(
        sail_node_table("Person Profile").unwrap(),
        "grust_node_person_profile"
    );
    assert_eq!(sail_edge_table("Knows").unwrap(), "grust_edge_knows");
    assert!(sail_typed_node_field_compatible("age"));
    assert!(sail_typed_node_field_compatible("label"));
    assert!(!sail_typed_node_field_compatible("props"));
    assert!(sail_typed_edge_field_compatible("since"));
    assert!(sail_typed_edge_field_compatible("label"));
    assert!(!sail_typed_edge_field_compatible("src_label"));
    assert!(!sail_typed_edge_field_compatible("dst_label"));
    assert!(!sail_typed_edge_field_compatible("props"));
    assert!(sail_typed_node_table_has_fields(
        &["id", "label", "age"],
        &["id", "age"]
    ));
    assert!(!sail_typed_node_table_has_fields(
        &["id", "name"],
        &["id", "age"]
    ));
    assert_eq!(
        sail_typed_node_table_missing_fields(&["id", "name", "props"], &["id", "age"]),
        ["name", "props"]
    );
    assert!(!sail_typed_node_table_has_fields(
        &["id", "props"],
        &["id", "props"]
    ));
    assert!(sail_typed_edge_table_has_fields(
        &["src_id", "dst_id", "label", "since"],
        &["edge_key", "src_id", "dst_id", "since"]
    ));
    assert!(!sail_typed_edge_table_has_fields(
        &["src_id", "dst_id", "src_label"],
        &["src_id", "dst_id", "src_label"]
    ));
    assert_eq!(
        sail_typed_edge_table_missing_fields(
            &["src_id", "dst_id", "src_label", "props"],
            &["src_id", "src_label", "props"]
        ),
        ["dst_id", "props", "src_label"]
    );
}

#[test]
fn cypher_node_create_requires_explicit_id_and_lowers_to_mutation() {
    let plan =
        sail_cypher_mutation_plan("CREATE (n:Person {id: 'person-1', name: 'Ada', age: 36})")
            .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            changed_nodes: 1,
            node_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Person",
            "person-1",
            Props::from([
                ("age".to_string(), Value::Int(36)),
                ("id".to_string(), Value::String("person-1".to_string())),
                ("name".to_string(), Value::String("Ada".to_string())),
            ]),
        ))]
    );

    let error = sail_cypher_mutation_plan("CREATE (:Person {name: 'Ada'})")
        .expect_err("missing id should fail");
    assert!(
        error
            .to_string()
            .contains("requires explicit string property 'id'")
    );
}

#[test]
fn cypher_edge_detection_ignores_arrow_inside_string_literals() {
    let create = sail_cypher_mutation_plan("CREATE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        create.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Server",
            "prod->primary",
            Props::from([("id".to_string(), Value::String("prod->primary".to_string()))]),
        ))]
    );

    let merge = sail_cypher_mutation_plan("MERGE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        merge.into_mutations(),
        vec![GraphMutation::UpsertNode(Node::new(
            "Server",
            "prod->primary",
            Props::from([("id".to_string(), Value::String("prod->primary".to_string()))]),
        ))]
    );

    let delete = sail_cypher_mutation_plan("DELETE (:Server {id: 'prod->primary'})").unwrap();
    assert_eq!(
        delete.into_mutations(),
        vec![GraphMutation::DeleteNode(NodeId::new("prod->primary"))]
    );

    let edge = sail_cypher_mutation_plan(
        "CREATE (:Server {id: 'a'})-[:ROUTES {note: 'a->b'}]->(:Server {id: 'b'})",
    )
    .unwrap();
    assert_eq!(
        edge.into_mutations(),
        vec![GraphMutation::UpsertEdge(Edge::new(
            "ROUTES",
            "a",
            "b",
            Props::from([("note".to_string(), Value::from("a->b"))]),
        ))]
    );
}

#[test]
fn cypher_parameters_bind_literal_values_only() {
    let options = CypherMutationOptions {
        parameters: CypherParameters::from([
            ("id".to_string(), Value::from("person-1")),
            ("name".to_string(), Value::from("Ada")),
            ("age".to_string(), Value::Int(36)),
            ("active".to_string(), Value::Bool(true)),
            ("note".to_string(), Value::Null),
        ]),
        ..CypherMutationOptions::default()
    };
    let plan = sail_cypher_mutation_plan_with_options(
        "
        CREATE (:Person {id: $id, name: $name, age: $age, active: $active, note: $note});
        MATCH (n:Person {id: $id}) SET n.name = $name;
        MATCH (n:Person {id: $id}) SET n.quoted = '$name';
        ",
        options,
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-1",
                Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("age".to_string(), Value::Int(36)),
                    ("id".to_string(), Value::from("person-1")),
                    ("name".to_string(), Value::from("Ada")),
                    ("note".to_string(), Value::Null),
                ]),
            )),
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
            GraphMutation::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("quoted".to_string(), Value::from("$name"))]),
            },
        ]
    );

    let missing = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {id: $missing})",
        CypherMutationOptions::default(),
    )
    .expect_err("missing parameter should fail");
    assert!(matches!(missing, GrustError::CypherUnresolvedIdentity(_)));

    let wrong_id_type = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {id: $id})",
        CypherMutationOptions {
            parameters: CypherParameters::from([("id".to_string(), Value::Int(1))]),
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("non-string id parameter should fail");
    assert!(matches!(
        wrong_id_type,
        GrustError::CypherUnresolvedIdentity(_)
    ));
}

#[test]
fn cypher_generated_node_id_policy_is_opt_in_for_create_only() {
    let (plan, generated) = sail_cypher_mutation_plan_with_options(
        "CREATE (n:Person {name: 'Ada'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap();

    assert_eq!(generated.len(), 1);
    assert_eq!(generated[0].variable.as_deref(), Some("n"));
    assert!(generated[0].id.as_str().starts_with("node-"));
    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            changed_nodes: 1,
            node_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(plan.operations.len(), 1);
    let GraphMutationPlanOp::UpsertNode { kind, node } = &plan.operations[0] else {
        panic!("generated node CREATE should lower to node upsert");
    };
    assert_eq!(*kind, GraphMutationPlanKind::Create);
    assert_eq!(node.id, generated[0].id);
    assert_eq!(node.props.get("id"), Some(&Value::from(node.id.as_str())));
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));

    let error = sail_cypher_mutation_plan_with_options(
        "MERGE (:Person {name: 'Ada'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("MERGE must still require a stable explicit id");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    let error = sail_cypher_mutation_plan_with_options(
        "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {id: 'person-2'})",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("edge endpoints must still resolve before writing");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));
}

#[test]
fn cypher_generated_node_id_can_bind_local_create_variable() {
    let (plan, generated) = sail_cypher_mutation_plan_with_options(
        "
        CREATE (a:Person {name: 'Ada'});
        CREATE (:Person {id: 'person-2'});
        CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'});
        ",
        CypherMutationOptions {
            node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap();

    assert_eq!(generated.len(), 1);
    assert_eq!(generated[0].variable.as_deref(), Some("a"));
    assert_eq!(plan.operations.len(), 3);
    let GraphMutationPlanOp::UpsertEdge { edge, .. } = &plan.operations[2] else {
        panic!("third operation should be an edge create");
    };
    assert_eq!(edge.from, generated[0].id);
    assert_eq!(edge.to, NodeId::new("person-2"));
}

#[test]
fn cypher_merge_edge_requires_resolved_endpoint_ids() {
    let plan = sail_cypher_mutation_plan(
        "MERGE (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1', since: 2020}]->(:Person {id: 'person-2'})",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            merges: 1,
            changed_edges: 1,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertEdge(
            Edge::new(
                "KNOWS",
                "person-1",
                "person-2",
                Props::from([
                    ("id".to_string(), Value::String("edge-1".to_string())),
                    ("since".to_string(), Value::Int(2020)),
                ]),
            )
            .with_id("edge-1")
        )]
    );

    let error = sail_cypher_mutation_plan(
        "CREATE (:Person {name: 'Ada'})-[:KNOWS]->(:Person {id: 'person-2'})",
    )
    .expect_err("unresolved source id should fail");
    assert!(error.to_string().contains("edge mutation source node"));
}

#[test]
fn cypher_delete_lowers_resolved_node_and_edge_patterns() {
    let node_delete = sail_cypher_mutation_plan("DELETE (:Person {id: 'person-1'})").unwrap();
    assert_eq!(
        node_delete.into_mutations(),
        vec![GraphMutation::DeleteNode(NodeId::new("person-1"))]
    );

    let edge_delete = sail_cypher_mutation_plan(
        "DELETE (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'})",
    )
    .unwrap();
    assert_eq!(
        edge_delete.into_mutations(),
        vec![GraphMutation::DeleteEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
        }]
    );
}

#[test]
fn cypher_match_delete_lowers_id_resolved_patterns() {
    let node_delete =
        sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) DELETE n").unwrap();
    assert_eq!(
        node_delete.report(),
        GraphMutationReport {
            deletes: 1,
            changed_nodes: 1,
            node_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        node_delete.into_mutations(),
        vec![GraphMutation::DeleteNode(NodeId::new("person-1"))]
    );

    let edge_delete = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e",
    )
    .unwrap();
    assert_eq!(
        edge_delete.report(),
        GraphMutationReport {
            deletes: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        edge_delete.into_mutations(),
        vec![GraphMutation::DeleteEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
        }]
    );

    let broad_edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) DELETE e",
    )
    .unwrap();
    let relationship = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("id".to_string(), Value::from("person-1"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
        },
        id: None,
        props: Props::new(),
        predicates: Vec::new(),
    };
    assert_eq!(
        broad_edge.report(),
        GraphMutationReport {
            deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        broad_edge.into_mutations(),
        vec![GraphMutation::DeleteMatchingEdges { relationship }]
    );
}

#[test]
fn cypher_match_delete_lowers_broad_node_patterns_with_cardinality() {
    let bounded = sail_cypher_mutation_plan("MATCH (n:Person {active: false}) DELETE n").unwrap();
    assert_eq!(
        bounded.report(),
        GraphMutationReport {
            deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        bounded.operations,
        vec![GraphMutationPlanOp::DeleteMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
            predicates: Vec::new(),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
    assert_eq!(
        bounded.into_mutations(),
        vec![GraphMutation::DeleteMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
            predicates: Vec::new(),
        }]
    );

    let unbounded = sail_cypher_mutation_plan("MATCH (n) DELETE n").unwrap();
    assert_eq!(
        unbounded.operations,
        vec![GraphMutationPlanOp::DeleteMatchingNodes {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
            cardinality: GraphMutationCardinality::UnboundedMany,
        }]
    );
}

#[test]
fn cypher_match_delete_rejects_unresolved_or_mismatched_patterns() {
    for cypher in [
        "MATCH (n:Person {id: 'person-1'}) DELETE m",
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE n",
        "MATCH (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'}) DELETE e",
    ] {
        let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn matching_nodes_sql_filters_by_label_and_properties() {
    let (sql, args) = matching_nodes_sql(
        Some(&Label::new("Person")),
        &Props::from([
            ("active".to_string(), Value::Bool(false)),
            ("age".to_string(), Value::Int(36)),
            ("name".to_string(), Value::from("Ada")),
        ]),
        &[],
    )
    .unwrap();

    assert_eq!(
        sql,
        "SELECT id, label, props FROM grust_nodes WHERE label = ? AND CAST(GET_JSON_OBJECT(props, '$.active') AS BOOLEAN) = ? AND CAST(GET_JSON_OBJECT(props, '$.age') AS BIGINT) = ? AND GET_JSON_OBJECT(props, '$.name') = ?"
    );
    assert_eq!(args.len(), 4);
    assert!(matches!(
        &args[0].literal_type,
        Some(expression::literal::LiteralType::String(_))
    ));
    assert!(matches!(
        &args[1].literal_type,
        Some(expression::literal::LiteralType::Boolean(_))
    ));
    assert!(matches!(
        &args[2].literal_type,
        Some(expression::literal::LiteralType::Long(_))
    ));
    assert!(matches!(
        &args[3].literal_type,
        Some(expression::literal::LiteralType::String(_))
    ));
}

#[test]
fn matching_nodes_sql_filters_by_property_predicates() {
    let predicates = vec![
        GraphPropertyPredicate {
            key: "status".to_string(),
            op: GraphPredicateOp::Equal,
            value: Value::from("inactive"),
        },
        GraphPropertyPredicate {
            key: "score".to_string(),
            op: GraphPredicateOp::GreaterThanOrEqual,
            value: Value::Int(10),
        },
        GraphPropertyPredicate {
            key: "nickname".to_string(),
            op: GraphPredicateOp::NotEqual,
            value: Value::Null,
        },
    ];
    let (sql, args) =
        matching_nodes_sql(Some(&Label::new("Person")), &Props::new(), &predicates).unwrap();

    assert!(sql.contains("label = ?"));
    assert!(sql.contains("GET_JSON_OBJECT(props, '$.status') = ?"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(props, '$.score') AS BIGINT) >= ?"));
    assert!(sql.contains("GET_JSON_OBJECT(props, '$.nickname') IS NOT NULL"));
    assert_eq!(args.len(), 3);
}

#[test]
fn matching_edges_sql_filters_by_relationship_properties() {
    let relationship = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::from([("id".to_string(), Value::from("person-1"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: Some(Label::new("Person")),
            props: Props::new(),
            predicates: Vec::new(),
        },
        id: Some(EdgeId::new("edge-1")),
        props: Props::from([
            ("active".to_string(), Value::Bool(true)),
            ("since".to_string(), Value::Int(2020)),
        ]),
        predicates: Vec::new(),
    };

    let (sql, args) = matching_edges_sql(&relationship).unwrap();

    assert!(sql.contains("e.edge_type = ?"));
    assert!(sql.contains("e.id = ?"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(e.props, '$.active') AS BOOLEAN) = ?"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(e.props, '$.since') AS BIGINT) = ?"));
    assert!(sql.contains("src.id = ?"));
    assert_eq!(args.len(), 7);
}

#[test]
fn cypher_match_where_lowers_node_predicates() {
    let plan = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person) WHERE n.status = 'inactive' AND n.score >= $min SET n.archived = true",
        CypherMutationOptions {
            parameters: CypherParameters::from([("min".to_string(), Value::Int(10))]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.operations,
        vec![GraphMutationPlanOp::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::new(),
            predicates: vec![
                GraphPropertyPredicate {
                    key: "status".to_string(),
                    op: GraphPredicateOp::Equal,
                    value: Value::from("inactive"),
                },
                GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::GreaterThanOrEqual,
                    value: Value::Int(10),
                },
            ],
            patch: Props::from([("archived".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_where_keeps_predicated_identity_matches_on_matching_path() {
    let plan = sail_cypher_mutation_plan(
        "MATCH (n:Person {id: 'person-1'}) WHERE n.status <> 'deleted' REMOVE n.nickname",
    )
    .unwrap();

    assert_eq!(
        plan.operations,
        vec![GraphMutationPlanOp::RemoveMatchingNodeProps {
            label: Some(Label::new("Person")),
            props: Props::from([("id".to_string(), Value::from("person-1"))]),
            predicates: vec![GraphPropertyPredicate {
                key: "status".to_string(),
                op: GraphPredicateOp::NotEqual,
                value: Value::from("deleted"),
            }],
            keys: vec!["nickname".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_where_lowers_edge_and_endpoint_predicates() {
    let plan = sail_cypher_mutation_plan(
        "MATCH (a:Person {id: 'a'})-[e:KNOWS]->(b:Person) WHERE e.since >= 2020 AND b.status <> 'blocked' SET e.seen = true",
    )
    .unwrap();

    assert_eq!(
        plan.operations,
        vec![GraphMutationPlanOp::PatchMatchingEdges {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("a"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::new(),
                    predicates: vec![GraphPropertyPredicate {
                        key: "status".to_string(),
                        op: GraphPredicateOp::NotEqual,
                        value: Value::from("blocked"),
                    }],
                },
                id: None,
                props: Props::new(),
                predicates: vec![GraphPropertyPredicate {
                    key: "since".to_string(),
                    op: GraphPredicateOp::GreaterThanOrEqual,
                    value: Value::Int(2020),
                }],
            },
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_where_rejects_deferred_predicate_forms() {
    for cypher in [
        "MATCH (n:Person) WHERE n.status = 'inactive' OR n.score >= 10 SET n.archived = true",
        "MATCH (n:Person) WHERE NOT n.active = true SET n.archived = true",
        "MATCH (n:Person) WHERE size(n.tags) = 2 SET n.archived = true",
        "MATCH (n:Person) WHERE n.active > true SET n.archived = true",
        "MATCH (n:Person) WHERE m.status = 'inactive' SET n.archived = true",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported WHERE predicate should fail");
        assert!(is_cypher_planning_error(&error) || matches!(error, GrustError::CypherSyntax(_)));
    }
}

#[test]
fn cypher_match_merge_lowers_id_resolved_edge_pattern() {
    let plan = sail_cypher_mutation_plan(
        "
        MATCH (a:Person {id: 'person-1', note: 'contains, comma'}), (b:Person {id: 'person-2'})
        MERGE (a)-[:KNOWS {since: 2026}]->(b)
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            merges: 1,
            changed_edges: 1,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertEdge(Edge::new(
            "KNOWS",
            "person-1",
            "person-2",
            Props::from([("since".to_string(), Value::Int(2026))]),
        ))]
    );
}

#[test]
fn cypher_match_create_lowers_id_resolved_edge_pattern() {
    let plan = sail_cypher_mutation_plan(
        "
        MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
        CREATE (a)-[:KNOWS {since: 2026}]->(b)
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            changed_edges: 1,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::UpsertEdge(Edge::new(
            "KNOWS",
            "person-1",
            "person-2",
            Props::from([("since".to_string(), Value::Int(2026))]),
        ))]
    );
}

#[test]
fn cypher_match_create_lowers_row_producing_edge_pattern() {
    let plan = sail_cypher_mutation_plan_with_options(
        "
        MATCH (a:Person {status: 'active'}), (b:Team {id: $team})
        WHERE a.score >= 10
        CREATE (a)-[:MEMBER_OF {source: 'cypher'}]->(b)
        ",
        CypherMutationOptions {
            parameters: CypherParameters::from([("team".to_string(), Value::from("team-1"))]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.operations,
        vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
            kind: GraphMutationPlanKind::Create,
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("active"))]),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::GreaterThanOrEqual,
                    value: Value::Int(10),
                }],
            },
            to: GraphNodeMatch {
                label: Some(Label::new("Team")),
                props: Props::from([("id".to_string(), Value::from("team-1"))]),
                predicates: Vec::new(),
            },
            label: Label::new("MEMBER_OF"),
            props: Props::from([("source".to_string(), Value::from("cypher"))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_merge_lowers_row_producing_edge_pattern() {
    let plan = sail_cypher_mutation_plan_with_options(
        "
        MATCH (a:Person {status: 'active'}), (b:Team {id: $team})
        WHERE a.score >= 10
        MERGE (a)-[:MEMBER_OF {source: 'cypher'}]->(b)
        ",
        CypherMutationOptions {
            parameters: CypherParameters::from([("team".to_string(), Value::from("team-1"))]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            merges: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.operations,
        vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
            kind: GraphMutationPlanKind::Merge,
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("active"))]),
                predicates: vec![GraphPropertyPredicate {
                    key: "score".to_string(),
                    op: GraphPredicateOp::GreaterThanOrEqual,
                    value: Value::Int(10),
                }],
            },
            to: GraphNodeMatch {
                label: Some(Label::new("Team")),
                props: Props::from([("id".to_string(), Value::from("team-1"))]),
                predicates: Vec::new(),
            },
            label: Label::new("MEMBER_OF"),
            props: Props::from([("source".to_string(), Value::from("cypher"))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_merge_rejects_unresolved_or_broad_forms() {
    for cypher in [
        "MATCH (:Person {id: 'person-1'}), (b:Person {id: 'person-2'}) MERGE (:Person {id: 'person-1'})-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) MERGE (a)-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) MERGE (:Person {id: 'person-3'})",
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) MERGE (a)-[e:KNOWS]->(b)",
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) MERGE (a)-[:KNOWS {id: 'edge-1'}]->(b)",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH MERGE must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_match_create_rejects_unresolved_or_broad_forms() {
    for cypher in [
        "MATCH (:Person {id: 'person-1'}), (b:Person {id: 'person-2'}) CREATE (:Person {id: 'person-1'})-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) CREATE (a)-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) CREATE (:Person {id: 'person-3'})",
        "MATCH (a:Person {id: 'person-1'}) CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'})",
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) CREATE (a)-[e:KNOWS]->(b)",
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) CREATE (a)-[:KNOWS {id: 'edge-1'}]->(b)",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH CREATE must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_match_set_map_patch_lowers_id_resolved_node() {
    let plan = sail_cypher_mutation_plan(
        "
        MATCH (n:Person {id: 'person-1'})
        SET n += {name: 'Ada', nickname: null, note: 'literal += stays literal'}
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            patches: 1,
            changed_nodes: 1,
            node_patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([
                ("name".to_string(), Value::from("Ada")),
                ("nickname".to_string(), Value::Null),
                (
                    "note".to_string(),
                    Value::String("literal += stays literal".to_string())
                ),
            ]),
        }]
    );
}

#[test]
fn cypher_match_set_map_patch_lowers_broad_nodes_with_cardinality() {
    let bounded = sail_cypher_mutation_plan(
        "
        MATCH (n:Person {status: 'inactive'})
        SET n += {archived: true, note: null}
        ",
    )
    .unwrap();

    assert_eq!(
        bounded.report(),
        GraphMutationReport {
            patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        bounded.operations,
        vec![GraphMutationPlanOp::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            patch: Props::from([
                ("archived".to_string(), Value::Bool(true)),
                ("note".to_string(), Value::Null),
            ]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
    assert_eq!(
        bounded.into_mutations(),
        vec![GraphMutation::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            patch: Props::from([
                ("archived".to_string(), Value::Bool(true)),
                ("note".to_string(), Value::Null),
            ]),
        }]
    );

    let unbounded = sail_cypher_mutation_plan("MATCH (n) SET n += {touched: true}").unwrap();
    assert_eq!(
        unbounded.operations,
        vec![GraphMutationPlanOp::PatchMatchingNodes {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
            patch: Props::from([("touched".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::UnboundedMany,
        }]
    );
}

#[test]
fn cypher_match_set_map_patch_lowers_id_resolved_edge() {
    let plan = sail_cypher_mutation_plan(
        "
        MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'})
        SET e += {since: 2026, note: null}
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            patches: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![GraphMutation::PatchEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            props: Props::from([
                ("note".to_string(), Value::Null),
                ("since".to_string(), Value::Int(2026)),
            ]),
        }]
    );

    let structural = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) SET e += {since: 2026}",
    )
    .unwrap();
    assert_eq!(
        structural.into_mutations(),
        vec![GraphMutation::PatchEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: None,
            props: Props::from([("since".to_string(), Value::Int(2026))]),
        }]
    );

    let broad = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) SET e += {seen: true}",
    )
    .unwrap();
    assert_eq!(
        broad.into_mutations(),
        vec![GraphMutation::PatchMatchingEdges {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("inactive"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::new(),
                predicates: Vec::new(),
            },
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
        }]
    );
}

#[test]
fn cypher_match_edge_mutations_accept_relationship_property_predicates() {
    let patch = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {since: 2020, active: true}]->(:Person {id: 'person-2'}) SET e.seen = true",
    )
    .unwrap();
    assert_eq!(
        patch.operations,
        vec![GraphMutationPlanOp::PatchMatchingEdges {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-2"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::from([
                    ("active".to_string(), Value::Bool(true)),
                    ("since".to_string(), Value::Int(2020)),
                ]),
                predicates: Vec::new(),
            },
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );

    let remove = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1', since: 2020}]->(:Person {id: 'person-2'}) REMOVE e.note",
    )
    .unwrap();
    assert_eq!(
        remove.operations,
        vec![GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-2"))]),
                    predicates: Vec::new(),
                },
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([("since".to_string(), Value::Int(2020))]),
                predicates: Vec::new(),
            },
            keys: vec!["note".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );

    let delete = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {active: false}]->(:Person {status: 'inactive'}) DELETE e",
    )
    .unwrap();
    assert_eq!(
        delete.operations,
        vec![GraphMutationPlanOp::DeleteMatchingEdges {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("inactive"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::from([("active".to_string(), Value::Bool(false))]),
                predicates: Vec::new(),
            },
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_set_property_assignment_lowers_resolved_node_and_edge() {
    let node =
        sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada'").unwrap();
    assert_eq!(
        node.into_mutations(),
        vec![GraphMutation::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([("name".to_string(), Value::from("Ada"))]),
        }]
    );

    let edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.since = 2026",
    )
    .unwrap();
    assert_eq!(
        edge.report(),
        GraphMutationReport {
            patches: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        edge.into_mutations(),
        vec![GraphMutation::PatchEdge {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            props: Props::from([("since".to_string(), Value::Int(2026))]),
        }]
    );

    let broad_edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) SET e.seen = true",
    )
    .unwrap();
    assert_eq!(
        broad_edge.into_mutations(),
        vec![GraphMutation::PatchMatchingEdges {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("inactive"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::new(),
                predicates: Vec::new(),
            },
            patch: Props::from([("seen".to_string(), Value::Bool(true))]),
        }]
    );

    let broad =
        sail_cypher_mutation_plan("MATCH (n:Person {status: 'inactive'}) SET n.archived = true")
            .unwrap();
    assert_eq!(
        broad.into_mutations(),
        vec![GraphMutation::PatchMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            patch: Props::from([("archived".to_string(), Value::Bool(true))]),
        }]
    );
}

#[test]
fn cypher_match_set_multiple_assignments_lowers_in_order() {
    let plan = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person {id: 'person-1'}) SET n.name = $name, n.updated_at = $ts, n.count = n.count + 1, n.name = 'Ada final'",
        CypherMutationOptions {
            parameters: CypherParameters::from([
                ("name".to_string(), Value::from("Ada")),
                ("ts".to_string(), Value::from("2026-06-16T00:00:00Z")),
            ]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            patches: 4,
            changed_nodes: 3,
            node_patches: 3,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.operations,
        vec![
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([(
                    "updated_at".to_string(),
                    Value::from("2026-06-16T00:00:00Z")
                )]),
            },
            GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: None,
                props: Props::from([("id".to_string(), Value::from("person-1"))]),
                predicates: Vec::new(),
                target_key: "count".to_string(),
                source_key: "count".to_string(),
                op: GraphNumericOp::Add,
                operand: Value::Int(1),
                cardinality: GraphMutationCardinality::SingleIdentity,
            },
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada final"))]),
            },
        ]
    );
}

#[test]
fn cypher_match_set_multiple_assignments_preserves_nested_commas() {
    let node = sail_cypher_mutation_plan(
        "MATCH (n:Person {id: 'person-1'}) SET n += {name: 'Ada, Countess', note: 'x,y'}, n.flag = true",
    )
    .unwrap();
    assert_eq!(
        node.operations,
        vec![
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([
                    ("name".to_string(), Value::from("Ada, Countess")),
                    ("note".to_string(), Value::from("x,y")),
                ]),
            },
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("flag".to_string(), Value::Bool(true))]),
            },
        ]
    );

    let edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.since = 2026, e.note = 'a,b'",
    )
    .unwrap();
    assert_eq!(
        edge.operations,
        vec![
            GraphMutationPlanOp::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([("since".to_string(), Value::Int(2026))]),
            },
            GraphMutationPlanOp::PatchEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
                id: Some(EdgeId::new("edge-1")),
                props: Props::from([("note".to_string(), Value::from("a,b"))]),
            },
        ]
    );
}

#[test]
fn cypher_match_set_multiple_assignments_supports_null_removal() {
    let plan = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person {id: 'person-1'}) SET n.nickname = null, n.name = 'Ada'",
        CypherMutationOptions {
            null_assignment: CypherNullAssignment::RemoveProperty,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;

    assert_eq!(
        plan.operations,
        vec![
            GraphMutationPlanOp::RemoveNodeProps {
                id: NodeId::new("person-1"),
                keys: vec!["nickname".to_string()],
            },
            GraphMutationPlanOp::PatchNode {
                id: NodeId::new("person-1"),
                props: Props::from([("name".to_string(), Value::from("Ada"))]),
            },
        ]
    );
}

#[test]
fn cypher_match_set_multiple_assignments_rejects_invalid_items() {
    for cypher in [
        "MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada', m.name = 'Bob'",
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'}) SET e.weight = e.weight + 1, e.note = 'x'",
        "MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada',",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("invalid assignment list should fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_null_assignment_option_removes_properties() {
    let options = CypherMutationOptions {
        null_assignment: CypherNullAssignment::RemoveProperty,
        ..CypherMutationOptions::default()
    };

    let resolved_node = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person {id: 'person-1'}) SET n.nickname = null",
        options.clone(),
    )
    .unwrap()
    .0;
    assert_eq!(
        resolved_node.operations,
        vec![GraphMutationPlanOp::RemoveNodeProps {
            id: NodeId::new("person-1"),
            keys: vec!["nickname".to_string()],
        }]
    );

    let broad_node = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person {status: 'inactive'}) SET n.nickname = null",
        options.clone(),
    )
    .unwrap()
    .0;
    assert_eq!(
        broad_node.operations,
        vec![GraphMutationPlanOp::RemoveMatchingNodeProps {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            keys: vec!["nickname".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );

    let resolved_edge = sail_cypher_mutation_plan_with_options(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) SET e.note = null",
        options.clone(),
    )
    .unwrap()
    .0;
    assert_eq!(
        resolved_edge.operations,
        vec![GraphMutationPlanOp::RemoveEdgeProps {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            keys: vec!["note".to_string()],
        }]
    );

    let broad_edge = sail_cypher_mutation_plan_with_options(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {active: true}]->(:Person {status: 'inactive'}) SET e.note = null",
        options,
    )
    .unwrap()
    .0;
    assert_eq!(
        broad_edge.operations,
        vec![GraphMutationPlanOp::RemoveMatchingEdgeProps {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("inactive"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::from([("active".to_string(), Value::Bool(true))]),
                predicates: Vec::new(),
            },
            keys: vec!["note".to_string()],
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_null_assignment_defaults_to_storing_null() {
    let node = sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) SET n.nickname = null")
        .unwrap();
    assert_eq!(
        node.operations,
        vec![GraphMutationPlanOp::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([("nickname".to_string(), Value::Null)]),
        }]
    );

    let map_patch = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Person {id: 'person-1'}) SET n += {nickname: null}",
        CypherMutationOptions {
            null_assignment: CypherNullAssignment::RemoveProperty,
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;
    assert_eq!(
        map_patch.operations,
        vec![GraphMutationPlanOp::PatchNode {
            id: NodeId::new("person-1"),
            props: Props::from([("nickname".to_string(), Value::Null)]),
        }]
    );
}

#[test]
fn cypher_match_set_numeric_expression_lowers_node_updates() {
    let resolved =
        sail_cypher_mutation_plan("MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + 1")
            .unwrap();
    assert_eq!(
        resolved.report(),
        GraphMutationReport {
            patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        resolved.operations,
        vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::from([("id".to_string(), Value::from("c1"))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality: GraphMutationCardinality::SingleIdentity,
        }]
    );
    assert_eq!(
        resolved.into_mutations(),
        vec![GraphMutation::UpdateMatchingNodeProperty {
            label: None,
            props: Props::from([("id".to_string(), Value::from("c1"))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
        }]
    );

    let broad = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Counter {active: true}) SET n.count = n.count + $delta",
        CypherMutationOptions {
            parameters: CypherParameters::from([("delta".to_string(), Value::Int(2))]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;
    assert_eq!(
        broad.operations,
        vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: Some(Label::new("Counter")),
            props: Props::from([("active".to_string(), Value::Bool(true))]),
            predicates: Vec::new(),
            target_key: "count".to_string(),
            source_key: "count".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(2),
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );

    let unbounded = sail_cypher_mutation_plan("MATCH (n) SET n.score = n.score / 2").unwrap();
    assert_eq!(
        unbounded.operations,
        vec![GraphMutationPlanOp::UpdateMatchingNodeProperty {
            label: None,
            props: Props::new(),
            predicates: Vec::new(),
            target_key: "score".to_string(),
            source_key: "score".to_string(),
            op: GraphNumericOp::Divide,
            operand: Value::Int(2),
            cardinality: GraphMutationCardinality::UnboundedMany,
        }]
    );
}

#[test]
fn cypher_match_set_numeric_expression_rejects_unsupported_forms() {
    for cypher in [
        "MATCH (n:Counter {id: 'c1'}) SET n.count = m.count + 1",
        "MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + m.delta",
        "MATCH (n:Counter {id: 'c1'}) SET n.count = size([])",
        "MATCH (n:Counter {id: 'c1'}) SET n.count = CASE n.count WHEN 1 THEN 2 END",
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'}) SET e.weight = e.weight + 1",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported expression should fail");
        assert!(is_cypher_planning_error(&error));
    }

    let non_numeric_parameter = sail_cypher_mutation_plan_with_options(
        "MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + $delta",
        CypherMutationOptions {
            parameters: CypherParameters::from([("delta".to_string(), Value::from("one"))]),
            ..CypherMutationOptions::default()
        },
    )
    .expect_err("non-numeric expression parameter should fail");
    assert!(matches!(non_numeric_parameter, GrustError::CypherSyntax(_)));
}

#[test]
fn cypher_match_remove_lowers_resolved_node_and_edge_properties() {
    let node =
        sail_cypher_mutation_plan("MATCH (n:Person {id: 'person-1'}) REMOVE n.nickname").unwrap();
    assert_eq!(
        node.report(),
        GraphMutationReport {
            property_removes: 1,
            changed_nodes: 1,
            node_property_removes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        node.into_mutations(),
        vec![GraphMutation::RemoveNodeProps {
            id: NodeId::new("person-1"),
            keys: vec!["nickname".to_string()],
        }]
    );

    let edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'}) REMOVE e.note",
    )
    .unwrap();
    assert_eq!(
        edge.into_mutations(),
        vec![GraphMutation::RemoveEdgeProps {
            from: NodeId::new("person-1"),
            label: Label::new("KNOWS"),
            to: NodeId::new("person-2"),
            id: Some(EdgeId::new("edge-1")),
            keys: vec!["note".to_string()],
        }]
    );

    let broad_edge = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {status: 'inactive'}) REMOVE e.note",
    )
    .unwrap();
    assert_eq!(
        broad_edge.into_mutations(),
        vec![GraphMutation::RemoveMatchingEdgeProps {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("id".to_string(), Value::from("person-1"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("inactive"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::new(),
                predicates: Vec::new(),
            },
            keys: vec!["note".to_string()],
        }]
    );

    let broad =
        sail_cypher_mutation_plan("MATCH (n:Person {status: 'inactive'}) REMOVE n.nickname")
            .unwrap();
    assert_eq!(
        broad.report(),
        GraphMutationReport {
            property_removes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        broad.into_mutations(),
        vec![GraphMutation::RemoveMatchingNodeProps {
            label: Some(Label::new("Person")),
            props: Props::from([("status".to_string(), Value::from("inactive"))]),
            predicates: Vec::new(),
            keys: vec!["nickname".to_string()],
        }]
    );
}

#[test]
fn cypher_match_set_rejects_deferred_patch_forms() {
    for cypher in ["MATCH (n:Person {id: 'person-1'}) SET m += {name: 'Ada'}"] {
        let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH SET must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_multi_statement_batch_preserves_order_and_aggregates_report() {
    let plan = sail_cypher_mutation_plan(
        "
        CREATE (:Person {id: 'person-1', name: 'Ada; still one literal'});
        MERGE (:Person {id: 'person-2', name: 'Bob'});
        CREATE (:Person {id: 'person-1'})-[:KNOWS {since: 2026}]->(:Person {id: 'person-2'});
        DELETE (:Person {id: 'person-2'});
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 2,
            merges: 1,
            deletes: 1,
            changed_nodes: 3,
            changed_edges: 1,
            node_upserts: 2,
            edge_upserts: 1,
            node_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-1",
                Props::from([
                    ("id".to_string(), Value::String("person-1".to_string())),
                    (
                        "name".to_string(),
                        Value::String("Ada; still one literal".to_string())
                    ),
                ]),
            )),
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-2",
                Props::from([
                    ("id".to_string(), Value::String("person-2".to_string())),
                    ("name".to_string(), Value::String("Bob".to_string())),
                ]),
            )),
            GraphMutation::UpsertEdge(Edge::new(
                "KNOWS",
                "person-1",
                "person-2",
                Props::from([("since".to_string(), Value::Int(2026))]),
            )),
            GraphMutation::DeleteNode(NodeId::new("person-2")),
        ]
    );
}

#[test]
fn sail_cypher_plan_executes_on_memory_facade() {
    let plan = sail_cypher_mutation_plan(
        "
        CREATE (:Person {id: 'person-1', status: 'inactive', score: 11});
        CREATE (:Person {id: 'person-2', status: 'inactive', score: 12});
        CREATE (:Person {id: 'person-3', status: 'active', score: 20});
        MATCH (a:Person {id: 'person-1'}), (b:Person {id: 'person-2'})
        CREATE (a)-[:KNOWS]->(b);
        MATCH (n:Person) WHERE n.status = 'inactive' AND n.score >= 10 SET n += {archived: true};
        MATCH (n:Person) WHERE n.archived = true DELETE n;
        ",
    )
    .unwrap();
    let store = MemoryGraphStore::new();

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 4,
            deletes: 1,
            patches: 1,
            matched_rows: 4,
            changed_nodes: 7,
            changed_edges: 2,
            node_upserts: 3,
            edge_upserts: 1,
            node_deletes: 2,
            edge_deletes: 1,
            node_patches: 2,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-1")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-2")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("person-3")))
            .unwrap()
            .is_some()
    );
}

#[test]
fn sail_cypher_multiple_set_assignments_execute_in_order_on_memory_facade() {
    let plan = sail_cypher_mutation_plan(
        "
        CREATE (:Counter {id: 'c1', count: 1});
        MATCH (n:Counter {id: 'c1'}) SET n.count = n.count + 1, n.count = n.count * 2;
        ",
    )
    .unwrap();
    let store = MemoryGraphStore::new();

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 1,
            patches: 2,
            matched_rows: 2,
            changed_nodes: 3,
            node_upserts: 1,
            node_patches: 2,
            ..GraphMutationReport::default()
        }
    );
    let node = futures_executor::block_on(store.get_node(&NodeId::new("c1")))
        .unwrap()
        .expect("counter node");
    assert_eq!(node.props.get("count"), Some(&Value::Int(4)));
}

#[test]
fn sail_row_producing_match_create_and_merge_execute_on_memory_facade() {
    let plan = sail_cypher_mutation_plan(
        "
        CREATE (:Person {id: 'ada', status: 'active', score: 11});
        CREATE (:Person {id: 'bob', status: 'active', score: 9});
        CREATE (:Team {id: 'eng'});
        MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
        WHERE a.score >= 10
        CREATE (a)-[:MEMBER_OF {source: 'cypher'}]->(b);
        MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
        WHERE a.score >= 10
        MERGE (a)-[:MEMBER_OF {source: 'merge'}]->(b);
        ",
    )
    .unwrap();
    let store = MemoryGraphStore::new();

    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 4,
            merges: 1,
            matched_rows: 2,
            changed_nodes: 3,
            changed_edges: 2,
            node_upserts: 3,
            edge_upserts: 2,
            ..GraphMutationReport::default()
        }
    );
    let edges = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: Some(NodeId::new("ada")),
        to: Some(NodeId::new("eng")),
        label: Some(Label::new("MEMBER_OF")),
    }))
    .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].props.get("source"), Some(&Value::from("merge")));
}

#[test]
fn cypher_parser_polish_handles_comments_keyword_case_and_statement_splitting() {
    let plan = sail_cypher_mutation_plan(
        r#"
        // full-line comment before the batch
        create (:Person {id: 'person-1', note: 'semicolon; and // literal'});
        /* block comment with ; and MATCH (n) DELETE n */
        mErGe (:Person {id: 'person-2'});
        MaTcH (n:Person {status: 'inactive'}) DeLeTe n;
        "#,
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 1,
            merges: 1,
            deletes: 1,
            changed_nodes: 2,
            node_upserts: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(plan.operations.len(), 3);
    assert!(matches!(
        &plan.operations[2],
        GraphMutationPlanOp::DeleteMatchingNodes {
            label,
            cardinality: GraphMutationCardinality::BoundedMany,
            ..
        } if label.as_ref().is_some_and(|label| label.as_str() == "Person")
    ));

    let error = sail_cypher_mutation_plan("CREATE (:Person {id: 'person-1'}); /* nope")
        .expect_err("unterminated block comment should fail");
    assert!(error.to_string().contains("unterminated block comment"));
}

#[test]
fn cypher_local_variables_resolve_edge_endpoints_and_deletes() {
    let plan = sail_cypher_mutation_plan(
        "
        CREATE (a:Person {id: 'person-1', name: 'Ada'});
        MERGE (b:Person {id: 'person-2', name: 'Bob'});
        CREATE (a)-[:KNOWS]->(b);
        DELETE (a)-[:KNOWS]->(b);
        DELETE (a);
        ",
    )
    .unwrap();

    assert_eq!(
        plan.report(),
        GraphMutationReport {
            creates: 2,
            merges: 1,
            deletes: 2,
            changed_nodes: 3,
            changed_edges: 2,
            node_upserts: 2,
            edge_upserts: 1,
            node_deletes: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-1",
                Props::from([
                    ("id".to_string(), Value::String("person-1".to_string())),
                    ("name".to_string(), Value::String("Ada".to_string())),
                ]),
            )),
            GraphMutation::UpsertNode(Node::new(
                "Person",
                "person-2",
                Props::from([
                    ("id".to_string(), Value::String("person-2".to_string())),
                    ("name".to_string(), Value::String("Bob".to_string())),
                ]),
            )),
            GraphMutation::UpsertEdge(Edge::new("KNOWS", "person-1", "person-2", Props::new(),)),
            GraphMutation::DeleteEdge {
                from: NodeId::new("person-1"),
                label: Label::new("KNOWS"),
                to: NodeId::new("person-2"),
            },
            GraphMutation::DeleteNode(NodeId::new("person-1")),
        ]
    );
}

#[test]
fn cypher_local_variables_reject_rebinding_and_unbound_refs() {
    let error = sail_cypher_mutation_plan(
        "
        CREATE (a:Person {id: 'person-1'});
        CREATE (a:Person {id: 'person-2'});
        ",
    )
    .expect_err("rebinding a variable to a different id should fail");
    assert!(error.to_string().contains("already bound to node id"));

    let error = sail_cypher_mutation_plan("CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'})")
        .expect_err("unbound edge endpoint should fail");
    assert!(error.to_string().contains("variable 'a' is not bound"));
}

#[test]
fn cypher_write_rejects_deferred_v1_semantics() {
    for cypher in [
        "CREATE (:Person {id: 'person-1'}) SET n.name = 'Ada'",
        "REMOVE n.name",
    ] {
        let error = sail_cypher_mutation_plan(cypher).expect_err("unsupported Cypher must fail");
        assert!(is_cypher_planning_error(&error));
    }
}

#[test]
fn cypher_errors_are_structured_for_callers() {
    let error = sail_cypher_mutation_plan("RETURN 1").expect_err("unsupported syntax");
    assert!(matches!(error, GrustError::CypherSyntax(_)));

    let error = sail_cypher_mutation_plan("DELETE (:Person {name: 'Ada'})")
        .expect_err("unresolved identity");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    let error = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'}) SET e.weight = e.weight + 1",
    )
    .expect_err("edge expression cardinality");
    assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));

    let error = cypher_execution_error(GrustError::Backend("boom".to_string()));
    assert!(matches!(error, GrustError::CypherExecution(_)));
}

#[test]
fn degree_arrow_results_parse_to_public_rows() {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("id", DataType::Utf8, false),
        ArrowField::new("degree", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(["a", "b"])),
            Arc::new(Int64Array::from_iter_values([3, 5])),
        ],
    )
    .unwrap();

    let rows = parse_degrees_from_arrow(&ipc_bytes(&batch).unwrap()).unwrap();

    assert_eq!(
        rows,
        vec![
            SailDegreeRow {
                id: NodeId::new("a"),
                degree: 3,
            },
            SailDegreeRow {
                id: NodeId::new("b"),
                degree: 5,
            },
        ]
    );
}

#[test]
fn degree_pair_arrow_results_parse_to_public_rows() {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("id", DataType::Utf8, false),
        ArrowField::new("in_degree", DataType::Int64, false),
        ArrowField::new("out_degree", DataType::Int64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(["a", "isolated"])),
            Arc::new(Int64Array::from_iter_values([2, 0])),
            Arc::new(Int64Array::from_iter_values([4, 0])),
        ],
    )
    .unwrap();

    let rows = parse_degree_pairs_from_arrow(&ipc_bytes(&batch).unwrap()).unwrap();

    assert_eq!(
        rows,
        vec![
            SailDegreePairRow {
                id: NodeId::new("a"),
                in_degree: 2,
                out_degree: 4,
            },
            SailDegreePairRow {
                id: NodeId::new("isolated"),
                in_degree: 0,
                out_degree: 0,
            },
        ]
    );
}

#[test]
fn triplet_arrow_results_parse_to_public_rows() {
    let schema = Arc::new(ArrowSchema::new(vec![
        ArrowField::new("src_id", DataType::Utf8, false),
        ArrowField::new("src_label", DataType::Utf8, false),
        ArrowField::new("src_props", DataType::Utf8, true),
        ArrowField::new("edge_id", DataType::Utf8, true),
        ArrowField::new("edge_src_id", DataType::Utf8, false),
        ArrowField::new("edge_src_label", DataType::Utf8, false),
        ArrowField::new("edge_dst_id", DataType::Utf8, false),
        ArrowField::new("edge_dst_label", DataType::Utf8, false),
        ArrowField::new("edge_type", DataType::Utf8, false),
        ArrowField::new("edge_props", DataType::Utf8, true),
        ArrowField::new("dst_id", DataType::Utf8, false),
        ArrowField::new("dst_label", DataType::Utf8, false),
        ArrowField::new("dst_props", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(["person-1"])),
            Arc::new(StringArray::from_iter_values(["Person"])),
            Arc::new(StringArray::from_iter_values([r#"{"name":"Ada"}"#])),
            Arc::new(StringArray::from(vec![Some("edge-1")])),
            Arc::new(StringArray::from_iter_values(["person-1"])),
            Arc::new(StringArray::from_iter_values(["Person"])),
            Arc::new(StringArray::from_iter_values(["talk-1"])),
            Arc::new(StringArray::from_iter_values(["Talk"])),
            Arc::new(StringArray::from_iter_values(["presents"])),
            Arc::new(StringArray::from_iter_values([r#"{"source":"schedule"}"#])),
            Arc::new(StringArray::from_iter_values(["talk-1"])),
            Arc::new(StringArray::from_iter_values(["Talk"])),
            Arc::new(StringArray::from_iter_values([
                r#"{"title":"Sail Graphs"}"#,
            ])),
        ],
    )
    .unwrap();

    let rows = parse_triplets_from_arrow(&ipc_bytes(&batch).unwrap()).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].src.id, NodeId::new("person-1"));
    assert_eq!(rows[0].src.props.get("name"), Some(&Value::from("Ada")));
    assert_eq!(rows[0].edge.id.as_ref().map(EdgeId::as_str), Some("edge-1"));
    assert_eq!(rows[0].edge.label, Label::new("presents"));
    assert_eq!(
        rows[0].edge.props.get("source"),
        Some(&Value::from("schedule"))
    );
    assert_eq!(rows[0].dst.id, NodeId::new("talk-1"));
    assert_eq!(
        rows[0].dst.props.get("title"),
        Some(&Value::from("Sail Graphs"))
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_put_and_get_node() {
    let store = store().await;

    let node = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada Lovelace"));
        p
    });
    let outcome = store.put_node(&node).await.expect("put_node");
    assert!(outcome.written());

    let fetched = store.get_node(&node.id).await.expect("get_node");
    assert!(fetched.is_some(), "node should exist after put");
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id.as_str(), "person-1");
    assert_eq!(fetched.label.as_str(), "Person");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_put_graph_and_traverse() {
    let store = store().await;

    let graph = sample_graph();
    let report = store.put_graph(&graph).await.expect("put_graph");
    assert_eq!(report.nodes, 2);
    assert_eq!(report.edges, 1);

    let result = store
        .traverse(Traversal::from_node("person-1").out("presents"))
        .await
        .expect("traverse");
    assert!(
        !result.is_empty(),
        "traversal should return destination node"
    );
    assert_eq!(result[0].id.as_str(), "talk-1");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_mutations() {
    let store = store().await;

    let report = store
        .execute_cypher_mutation(
            "
            CREATE (a:Person {id: 'person-1', name: 'Ada'});
            MERGE (b:Person {id: 'person-2', name: 'Bob'});
            CREATE (a)-[e:KNOWS {id: 'edge-1', since: 2020}]->(b);
            ",
        )
        .await
        .expect("execute ordered Cypher mutation batch");
    assert_eq!(
        report,
        GraphMutationReport {
            creates: 2,
            merges: 1,
            changed_nodes: 2,
            changed_edges: 1,
            node_upserts: 2,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            to: Some(NodeId::new("person-2")),
            label: Some(Label::new("KNOWS")),
        })
        .await
        .expect("read cypher-created edge");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id.as_ref().map(EdgeId::as_str), Some("edge-1"));

    let report = store
        .execute_cypher_mutation(
            "DELETE (:Person {id: 'person-1'})-[:KNOWS]->(:Person {id: 'person-2'})",
        )
        .await
        .expect("delete edge");
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        store
            .get_edges(EdgeQuery {
                from: Some(NodeId::new("person-1")),
                to: Some(NodeId::new("person-2")),
                label: Some(Label::new("KNOWS")),
            })
            .await
            .expect("read after edge delete")
            .is_empty()
    );

    let report = store
        .execute_cypher_mutation("DELETE (:Person {id: 'person-1'})")
        .await
        .expect("delete node");
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            changed_nodes: 1,
            node_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        store
            .get_node(&NodeId::new("person-1"))
            .await
            .expect("read after node delete")
            .is_none()
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_row_producing_match_create_and_merge_edges() {
    let store = store().await;

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'ada', status: 'active', score: 11});
            CREATE (:Person {id: 'bob', status: 'active', score: 12});
            CREATE (:Person {id: 'cam', status: 'inactive', score: 20});
            CREATE (:Team {id: 'eng'});
            CREATE (:Team {id: 'research'});
            ",
        )
        .await
        .expect("seed nodes");

    let zero = store
        .execute_cypher_mutation(
            "
            MATCH (a:Person {status: 'missing'}), (b:Team {id: 'eng'})
            CREATE (a)-[:MEMBER_OF {source: 'zero'}]->(b)
            ",
        )
        .await
        .expect("zero-row create");
    assert_eq!(
        zero,
        GraphMutationReport {
            creates: 1,
            ..GraphMutationReport::default()
        }
    );

    let one = store
        .execute_cypher_mutation(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            WHERE a.score >= 12
            CREATE (a)-[:MEMBER_OF {source: 'one'}]->(b)
            ",
        )
        .await
        .expect("one-row create");
    assert_eq!(
        one,
        GraphMutationReport {
            creates: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_upserts: 1,
            ..GraphMutationReport::default()
        }
    );

    let many = store
        .execute_cypher_mutation(
            "
            MATCH (a:Person {status: 'active'}), (b:Team)
            CREATE (a)-[:ASSIGNED_TO {source: 'many'}]->(b)
            ",
        )
        .await
        .expect("many-row create");
    assert_eq!(
        many,
        GraphMutationReport {
            creates: 1,
            matched_rows: 4,
            changed_edges: 4,
            edge_upserts: 4,
            ..GraphMutationReport::default()
        }
    );

    let merge = store
        .execute_cypher_mutation(
            "
            MATCH (a:Person {status: 'active'}), (b:Team)
            MERGE (a)-[:ASSIGNED_TO {source: 'many-merge'}]->(b)
            ",
        )
        .await
        .expect("many-row merge");
    assert_eq!(
        merge,
        GraphMutationReport {
            merges: 1,
            matched_rows: 4,
            changed_edges: 4,
            edge_upserts: 4,
            ..GraphMutationReport::default()
        }
    );

    let member_of = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("bob")),
            to: Some(NodeId::new("eng")),
            label: Some(Label::new("MEMBER_OF")),
        })
        .await
        .expect("read one-row edge");
    assert_eq!(member_of.len(), 1);
    assert_eq!(member_of[0].props.get("source"), Some(&Value::from("one")));

    let assigned = store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(Label::new("ASSIGNED_TO")),
        })
        .await
        .expect("read many-row edges");
    assert_eq!(assigned.len(), 4);
    assert!(
        assigned
            .iter()
            .all(|edge| edge.props.get("source") == Some(&Value::from("many-merge")))
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_broad_match_delete_nodes() {
    let store = store().await;

    let report = store
        .execute_cypher_mutation("MATCH (n:Person {status: 'missing'}) DELETE n")
        .await
        .expect("zero-match broad delete");
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            ..GraphMutationReport::default()
        }
    );

    store
        .execute_cypher_mutation(
            "
            CREATE (a:Person {id: 'person-inactive-1', status: 'inactive'});
            CREATE (b:Person {id: 'person-inactive-2', status: 'inactive'});
            CREATE (c:Person {id: 'person-active-1', status: 'active'});
            CREATE (a)-[:KNOWS]->(b);
            CREATE (a)-[:KNOWS]->(c);
            ",
        )
        .await
        .expect("seed graph for broad delete");

    let report = store
        .execute_cypher_mutation("MATCH (n:Person {status: 'inactive'}) DELETE n")
        .await
        .expect("many-match broad delete");
    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 2,
            changed_nodes: 2,
            changed_edges: 2,
            node_deletes: 2,
            edge_deletes: 2,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        store
            .get_node(&NodeId::new("person-inactive-1"))
            .await
            .expect("read deleted inactive node")
            .is_none()
    );
    assert!(
        store
            .get_node(&NodeId::new("person-inactive-2"))
            .await
            .expect("read deleted inactive node")
            .is_none()
    );
    assert!(
        store
            .get_node(&NodeId::new("person-active-1"))
            .await
            .expect("read remaining active node")
            .is_some()
    );
    assert!(
        store
            .get_edges(EdgeQuery::default())
            .await
            .expect("read after broad delete cascade")
            .is_empty()
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_broad_match_set_nodes() {
    let store = store().await;

    let report = store
        .execute_cypher_mutation("MATCH (n:Person {status: 'missing'}) SET n += {archived: true}")
        .await
        .expect("zero-match broad patch");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            ..GraphMutationReport::default()
        }
    );

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'person-inactive-1', status: 'inactive'});
            CREATE (:Person {id: 'person-inactive-2', status: 'inactive'});
            CREATE (:Person {id: 'person-active-1', status: 'active'});
            ",
        )
        .await
        .expect("seed graph for broad patch");

    let report = store
        .execute_cypher_mutation(
            "MATCH (n:Person {status: 'inactive'}) SET n += {archived: true, note: null}",
        )
        .await
        .expect("many-match broad patch");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_patches: 2,
            ..GraphMutationReport::default()
        }
    );
    for id in ["person-inactive-1", "person-inactive-2"] {
        let node = store
            .get_node(&NodeId::new(id))
            .await
            .expect("read patched inactive node")
            .expect("patched inactive node exists");
        assert_eq!(node.props.get("archived"), Some(&Value::Bool(true)));
        assert_eq!(node.props.get("note"), Some(&Value::Null));
    }
    let active = store
        .get_node(&NodeId::new("person-active-1"))
        .await
        .expect("read remaining active node")
        .expect("active node exists");
    assert_eq!(active.props.get("archived"), None);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_broad_match_set_updates_typed_nodes() {
    let store = store().await;
    store
        .apply_schema(&person_schema())
        .await
        .expect("apply Person schema");
    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'person-1', name: 'Ada'});
            CREATE (:Person {id: 'person-2', name: 'Bob'});
            ",
        )
        .await
        .expect("seed typed Person rows");

    let report = store
        .execute_cypher_mutation("MATCH (n:Person) SET n.age = 37")
        .await
        .expect("broad assign typed Person age");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_patches: 2,
            ..GraphMutationReport::default()
        }
    );

    let rows = query_string_rows(
        store
            .query_arrow_ipc(
                "SELECT id, CAST(age AS STRING) AS age FROM grust_node_person ORDER BY id",
            )
            .await
            .expect("query typed Person table"),
        2,
    );
    assert_eq!(
        rows,
        vec![
            vec!["person-1".to_string(), "37".to_string()],
            vec!["person-2".to_string(), "37".to_string()],
        ]
    );

    let report = store
        .execute_cypher_mutation("MATCH (n:Person) REMOVE n.age")
        .await
        .expect("broad remove typed Person age");
    assert_eq!(
        report,
        GraphMutationReport {
            property_removes: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_property_removes: 2,
            ..GraphMutationReport::default()
        }
    );

    let rows = query_string_rows(
        store
            .query_arrow_ipc(
                "SELECT CAST(COUNT(*) AS STRING) AS null_age_count FROM grust_node_person WHERE age IS NULL",
            )
            .await
            .expect("query typed Person table after remove"),
        1,
    );
    assert_eq!(rows, vec![vec!["2".to_string()]]);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_match_set_edge_patch_updates_typed_edges() {
    let store = store().await;
    store
        .apply_schema(&person_schema())
        .await
        .expect("apply Person schema");
    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'person-1'})-[e:presents {id: 'edge-1', source: 'draft'}]->(:Talk {id: 'talk-1'});
            ",
        )
        .await
        .expect("seed typed edge row");

    let report = store
        .execute_cypher_mutation(
            "
            MATCH (:Person {id: 'person-1'})-[e:presents {id: 'edge-1'}]->(:Talk {id: 'talk-1'})
            SET e += {source: 'final'}
            ",
        )
        .await
        .expect("patch typed edge row");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            to: Some(NodeId::new("talk-1")),
            label: Some(Label::new("presents")),
        })
        .await
        .expect("read patched edge");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].props.get("source"), Some(&Value::from("final")));

    let rows = query_string_rows(
        store
            .query_arrow_ipc("SELECT id, source FROM grust_edge_presents ORDER BY id")
            .await
            .expect("query typed presents edge table"),
        2,
    );
    assert_eq!(rows, vec![vec!["edge-1".to_string(), "final".to_string()]]);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_match_set_and_remove_node_properties() {
    let store = store().await;
    store
        .apply_schema(&person_schema())
        .await
        .expect("apply Person schema");
    store
        .execute_cypher_mutation("CREATE (:Person {id: 'person-1', name: 'Ada', age: 36})")
        .await
        .expect("seed typed Person row");

    let report = store
        .execute_cypher_mutation("MATCH (n:Person {id: 'person-1'}) SET n.age = 37")
        .await
        .expect("assign node property");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            changed_nodes: 1,
            node_patches: 1,
            ..GraphMutationReport::default()
        }
    );

    let rows = query_string_rows(
        store
            .query_arrow_ipc(
                "SELECT id, CAST(age AS STRING) AS age FROM grust_node_person ORDER BY id",
            )
            .await
            .expect("query typed Person table after assignment"),
        2,
    );
    assert_eq!(rows, vec![vec!["person-1".to_string(), "37".to_string()]]);

    let report = store
        .execute_cypher_mutation("MATCH (n:Person {id: 'person-1'}) REMOVE n.age")
        .await
        .expect("remove node property");
    assert_eq!(
        report,
        GraphMutationReport {
            property_removes: 1,
            changed_nodes: 1,
            node_property_removes: 1,
            ..GraphMutationReport::default()
        }
    );

    let node = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("read node after property remove")
        .expect("node still exists");
    assert_eq!(node.props.get("age"), None);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_match_set_and_remove_edge_properties() {
    let store = store().await;
    store
        .apply_schema(&person_schema())
        .await
        .expect("apply Person schema");
    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'person-1'})-[e:presents {id: 'edge-1', source: 'draft', note: 'tmp'}]->(:Talk {id: 'talk-1'});
            ",
        )
        .await
        .expect("seed typed edge row");

    let report = store
        .execute_cypher_mutation(
            "
            MATCH (:Person {id: 'person-1'})-[e:presents {id: 'edge-1'}]->(:Talk {id: 'talk-1'})
            SET e.source = 'final'
            ",
        )
        .await
        .expect("assign edge property");
    assert_eq!(
        report,
        GraphMutationReport {
            patches: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );

    let rows = query_string_rows(
        store
            .query_arrow_ipc("SELECT id, source FROM grust_edge_presents ORDER BY id")
            .await
            .expect("query typed presents edge table after assignment"),
        2,
    );
    assert_eq!(rows, vec![vec!["edge-1".to_string(), "final".to_string()]]);

    let report = store
        .execute_cypher_mutation(
            "
            MATCH (:Person {id: 'person-1'})-[e:presents {id: 'edge-1'}]->(:Talk {id: 'talk-1'})
            REMOVE e.note
            ",
        )
        .await
        .expect("remove edge property");
    assert_eq!(
        report,
        GraphMutationReport {
            property_removes: 1,
            changed_edges: 1,
            edge_property_removes: 1,
            ..GraphMutationReport::default()
        }
    );

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            to: Some(NodeId::new("talk-1")),
            label: Some(Label::new("presents")),
        })
        .await
        .expect("read edge after property remove");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].props.get("source"), Some(&Value::from("final")));
    assert_eq!(edges[0].props.get("note"), None);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_mutation_strict_create() {
    let store = store().await;
    let strict = CypherMutationOptions {
        create_mode: CypherCreateMode::ErrorIfExists,
        ..CypherMutationOptions::default()
    };

    store
        .execute_cypher_mutation("CREATE (:Person {id: 'person-1', name: 'Ada'})")
        .await
        .expect("default create writes node");
    store
        .execute_cypher_mutation("CREATE (:Person {id: 'person-1', name: 'Ada Updated'})")
        .await
        .expect("default create remains upsert-compatible");

    let error = store
        .execute_cypher_mutation_with_options(
            "CREATE (:Person {id: 'person-1', name: 'Ada Strict'})",
            strict.clone(),
        )
        .await
        .expect_err("strict create should reject existing node");
    assert!(error.to_string().contains("existing node 'person-1'"));

    store
        .execute_cypher_mutation("MERGE (:Person {id: 'person-2', name: 'Bob'})")
        .await
        .expect("merge destination node");
    store
        .execute_cypher_mutation(
            "CREATE (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-2'})",
        )
        .await
        .expect("default create writes edge");

    let error = store
        .execute_cypher_mutation_with_options(
            "CREATE (:Person {id: 'person-1'})-[e:KNOWS {id: 'edge-2'}]->(:Person {id: 'person-2'})",
            strict.clone(),
        )
        .await
        .expect_err("strict create should reject existing structural edge");
    assert!(error.to_string().contains("existing edge"));

    let error = store
        .execute_cypher_mutation_with_options(
            "CREATE (:Person {id: 'person-3'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'person-4'})",
            strict,
        )
        .await
        .expect_err("strict create should reject existing explicit edge id");
    assert!(error.to_string().contains("existing edge"));
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_mutation_generated_node_ids() {
    let store = store().await;
    let result = store
        .execute_cypher_mutation_result_with_options(
            "CREATE (n:Person {name: 'Ada'})",
            CypherMutationOptions {
                node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
                ..CypherMutationOptions::default()
            },
        )
        .await
        .expect("execute generated node CREATE");

    assert_eq!(
        result.report,
        GraphMutationReport {
            creates: 1,
            changed_nodes: 1,
            node_upserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(result.generated_node_ids.len(), 1);
    assert_eq!(result.generated_node_ids[0].variable.as_deref(), Some("n"));

    let node = store
        .get_node(&result.generated_node_ids[0].id)
        .await
        .expect("read generated node")
        .expect("generated node exists");
    assert_eq!(node.label, Label::new("Person"));
    assert_eq!(node.props.get("id"), Some(&Value::from(node.id.as_str())));
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_mutation_collects_written_edge_identities() {
    let store = store().await;
    let default_result = store
        .execute_cypher_mutation_result_with_options(
            "
            CREATE (:Person {id: 'ada'});
            CREATE (:Person {id: 'bob'});
            CREATE (:Person {id: 'ada'})-[:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'});
            ",
            CypherMutationOptions::default(),
        )
        .await
        .expect("execute default edge create");
    assert!(default_result.written_edge_identities.is_empty());

    store.clear().await.expect("clear graph before collect run");
    let result = store
        .execute_cypher_mutation_result_with_options(
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Team {id: 'eng'});
            CREATE (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[:MEMBER_OF {source: 'create'}]->(b);
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            MERGE (a)-[:MEMBER_OF {source: 'merge'}]->(b);
            ",
            CypherMutationOptions {
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        )
        .await
        .expect("execute edge writes with identity collection");

    assert_eq!(result.generated_node_ids, Vec::new());
    assert_eq!(result.written_edge_identities.len(), 5);
    assert!(
        result
            .written_edge_identities
            .contains(&CypherWrittenEdgeIdentity {
                kind: GraphMutationPlanKind::Create,
                from: NodeId::new("ada"),
                label: Label::new("KNOWS"),
                to: NodeId::new("bob"),
                id: Some(EdgeId::new("edge-1")),
            })
    );
    assert_eq!(
        result
            .written_edge_identities
            .iter()
            .filter(|identity| identity.kind == GraphMutationPlanKind::Create
                && identity.label == Label::new("MEMBER_OF"))
            .count(),
        2
    );
    assert_eq!(
        result
            .written_edge_identities
            .iter()
            .filter(|identity| identity.kind == GraphMutationPlanKind::Merge
                && identity.label == Label::new("MEMBER_OF"))
            .count(),
        2
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_cypher_match_over_grust_backend_tables() {
    let store = store().await;

    let mut builder = Graph::builder();
    let _ = builder
        .node("Person", "person-1")
        .prop("name", "Alice")
        .prop("age", "42")
        .finish();
    let _ = builder
        .node("Person", "person-2")
        .prop("name", "Bob")
        .prop("age", "31")
        .finish();
    let _ = builder
        .node("Document", "doc-1")
        .prop("name", "Paper")
        .prop("age", "42")
        .finish();
    let _ = builder
        .edge("KNOWS", "person-1", "person-2")
        .id("edge-1")
        .prop("since", "2020")
        .finish();
    let _ = builder
        .edge("LIKES", "person-2", "person-1")
        .id("edge-2")
        .prop("since", "2021")
        .finish();
    let _ = builder
        .edge("KNOWS", "doc-1", "person-2")
        .id("edge-3")
        .prop("since", "2022")
        .finish();
    let graph = builder.build();

    store.put_graph(&graph).await.expect("put graph");

    let outgoing = query_string_rows(
        store
            .query_arrow_ipc(
                "MATCH (a:Person {age: '42'})-[e:KNOWS {since: '2020'}]->(b:Person) \
                 RETURN a.id, e.id, b.name \
                 ORDER BY b.name",
            )
            .await
            .expect("outgoing Cypher query"),
        3,
    );
    assert_eq!(outgoing, vec![vec!["person-1", "edge-1", "Bob"]]);

    let incoming = query_string_rows(
        store
            .query_arrow_ipc(
                "MATCH (a)<-[e]-(b) \
                 RETURN a.id, e.id, b.id \
                 ORDER BY e.id",
            )
            .await
            .expect("incoming Cypher query"),
        3,
    );
    assert_eq!(
        incoming,
        vec![
            vec!["person-2", "edge-1", "person-1"],
            vec!["person-1", "edge-2", "person-2"],
            vec!["person-2", "edge-3", "doc-1"],
        ]
    );

    let undirected = query_string_rows(
        store
            .query_arrow_ipc(
                "MATCH (a)-[e]-(b) \
                 RETURN a.id, e.id, b.id \
                 ORDER BY e.id, a.id",
            )
            .await
            .expect("undirected Cypher query"),
        3,
    );
    assert_eq!(
        undirected,
        vec![
            vec!["person-1", "edge-1", "person-2"],
            vec!["person-2", "edge-1", "person-1"],
            vec!["person-1", "edge-2", "person-2"],
            vec!["person-2", "edge-2", "person-1"],
            vec!["doc-1", "edge-3", "person-2"],
            vec!["person-2", "edge-3", "doc-1"],
        ]
    );

    let limit_all = query_string_rows(
        store
            .query_arrow_ipc(
                "MATCH (a:Person)-->(b:Person) \
                 RETURN b.name \
                 ORDER BY b.name \
                 SKIP 1 \
                 LIMIT ALL",
            )
            .await
            .expect("LIMIT ALL Cypher query"),
        1,
    );
    assert_eq!(limit_all, vec![vec!["Bob"]]);
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_get_edges() {
    let store = store().await;

    let graph = sample_graph();
    store.put_graph(&graph).await.expect("put_graph");

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::new("person-1")),
            ..Default::default()
        })
        .await
        .expect("get_edges");
    assert!(!edges.is_empty());
    assert_eq!(edges[0].label.as_str(), "presents");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_idempotent_put_node() {
    let store = store().await;

    let node = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada v1"));
        p
    });
    store.put_node(&node).await.expect("first put");

    let updated = Node::new("Person", "person-1", {
        let mut p = Props::new();
        p.insert("name".into(), Value::from("Ada v2"));
        p
    });
    store.put_node(&updated).await.expect("second put");

    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node")
        .expect("node missing");
    let name = fetched
        .props
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(name, "Ada v2", "second put should overwrite props");
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_delete_node_and_edge() {
    let store = store().await;

    let graph = sample_graph();
    store.put_graph(&graph).await.expect("put_graph");

    store
        .delete_edge(
            &NodeId::new("person-1"),
            &Label::new("presents"),
            &NodeId::new("talk-1"),
        )
        .await
        .expect("delete_edge");
    let edges = store
        .get_edges(EdgeQuery::default())
        .await
        .expect("get_edges");
    assert!(edges.is_empty(), "edge should be deleted");

    store
        .delete_node(&NodeId::new("person-1"))
        .await
        .expect("delete_node");
    let fetched = store
        .get_node(&NodeId::new("person-1"))
        .await
        .expect("get_node");
    assert!(fetched.is_none(), "node should be deleted");
}
