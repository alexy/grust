use std::io::Cursor;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::RwLock;

use arrow::array::Array as _;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use grust_core::prelude::*;
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
        }
    );
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
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
    assert_eq!(
        bounded.into_mutations(),
        vec![GraphMutation::DeleteMatchingNodes {
            label: Some(Label::new("Person")),
            props: Props::from([("active".to_string(), Value::Bool(false))]),
        }]
    );

    let unbounded = sail_cypher_mutation_plan("MATCH (n) DELETE n").unwrap();
    assert_eq!(
        unbounded.operations,
        vec![GraphMutationPlanOp::DeleteMatchingNodes {
            label: None,
            props: Props::new(),
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
fn cypher_match_merge_rejects_unresolved_or_broad_forms() {
    for cypher in [
        "MATCH (:Person {id: 'person-1'}), (b:Person {id: 'person-2'}) MERGE (:Person {id: 'person-1'})-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) MERGE (a)-[:KNOWS]->(b)",
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) MERGE (a)-[:KNOWS]->(b)",
        "MATCH (a:Person {id: 'person-1'}) MERGE (:Person {id: 'person-3'})",
        "MATCH (a:Person {id: 'person-1'}) CREATE (a)-[:KNOWS]->(:Person {id: 'person-2'})",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported MATCH MERGE must fail");
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
}

#[test]
fn cypher_match_set_rejects_deferred_patch_forms() {
    for cypher in [
        "MATCH (n:Person {id: 'person-1'}) SET m += {name: 'Ada'}",
        "MATCH (n:Person {id: 'person-1'}) SET n.name = 'Ada'",
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS {since: 2020}]->(:Person {id: 'person-2'}) SET e += {since: 2026}",
        "MATCH (n:Person {id: 'person-1'}) REMOVE n.name",
    ] {
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
        "MATCH (:Person {id: 'a'})-[e:KNOWS {since: 2020}]->(:Person {id: 'b'}) SET e += {since: 2026}",
    )
    .expect_err("edge patch cardinality");
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
        .execute_cypher_mutation("MATCH (n:Person) SET n += {age: 37}")
        .await
        .expect("broad patch typed Person age");
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
async fn test_execute_cypher_mutation_strict_create() {
    let store = store().await;
    let strict = CypherMutationOptions {
        create_mode: CypherCreateMode::ErrorIfExists,
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
            strict,
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
            strict,
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
