use std::collections::HashMap;
use std::io::Cursor;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::RwLock;

use arrow::array::Array as _;
use arrow::array::{Int64Array, StringArray};
use arrow::datatypes::{DataType, Field as ArrowField, Schema as ArrowSchema};
use arrow::ipc::reader::StreamReader;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
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

fn ipc_bytes(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut data, &batch.schema())
            .map_err(|e| GrustError::Backend(format!("IPC writer: {e}")))?;
        writer
            .write(batch)
            .map_err(|e| GrustError::Backend(format!("write IPC batch: {e}")))?;
        writer
            .finish()
            .map_err(|e| GrustError::Backend(format!("finish IPC stream: {e}")))?;
    }
    Ok(data)
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
fn edge_key_delete_sql_targets_persisted_edge_identity() {
    let sql = delete_edge_keys_from_view_sql("grust_edges").unwrap();

    assert!(sql.contains("MERGE INTO `grust_edges`"));
    assert!(sql.contains("ON t.edge_key = s.edge_key"));
    assert!(!sql.contains("t.src_id = s.src_id"));
    assert!(!sql.contains("t.dst_id = s.dst_id"));
}

#[test]
fn cypher_mutation_options_default_to_upsert_compatible_create() {
    assert_eq!(
        CypherMutationOptions::default(),
        CypherMutationOptions {
            create_mode: CypherCreateMode::UpsertCompatible,
            node_id_policy: CypherNodeIdPolicy::ExplicitOnly,
            relationship_id_policy: CypherRelationshipIdPolicy::ExplicitOnly,
            collect_written_node_identities: false,
            collect_written_edge_identities: false,
            null_assignment: CypherNullAssignment::StoreNull,
            parameters: CypherParameters::new(),
        }
    );
}

#[tokio::test]
async fn sail_reports_validate_before_write_for_required_and_unique_constraints() {
    let store = request_store();
    let required = GraphConstraint::NodePropertyRequired {
        label: Label::new("Person"),
        key: "email".to_string(),
    };
    let unique = GraphConstraint::NodePropertyUnique {
        label: Label::new("Person"),
        key: "email".to_string(),
    };
    let edge_unique = GraphConstraint::EdgePropertyUnique {
        label: Label::new("RATED"),
        key: "token".to_string(),
    };

    assert_eq!(
        store.constraint_capability(&required),
        GraphConstraintCapability::ValidateBeforeWrite
    );
    assert_eq!(
        store.constraint_capability(&unique),
        GraphConstraintCapability::ValidateBeforeWrite
    );
    assert_eq!(
        store.constraint_capability(&edge_unique),
        GraphConstraintCapability::ValidateBeforeWrite
    );
    assert_eq!(
        store.native_constraint_capability(&required),
        GraphNativeConstraintCapability::Unsupported
    );

    let native_error = store
        .apply_native_constraint(GraphNativeConstraintRequest {
            constraint: unique,
            if_not_exists: true,
        })
        .await
        .expect_err("Sail validates constraints but does not emit native DDL");
    assert!(
        matches!(native_error, GrustError::Unsupported(message) if message.contains("backend-native DDL"))
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
fn strict_create_plan_conflicts_reject_duplicate_concrete_create_targets() {
    let duplicate_nodes = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "ada", Props::new()),
        },
        GraphMutationPlanOp::UpsertNode {
            kind: GraphMutationPlanKind::Create,
            node: Node::new("Person", "ada", Props::new()),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_nodes)
        .expect_err("duplicate CREATE node should fail");
    assert!(error.to_string().contains("duplicate node 'ada'"));

    let duplicate_structural_edges = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-1"),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-2"),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_structural_edges)
        .expect_err("duplicate CREATE structural edge should fail");
    assert!(error.to_string().contains("duplicate edge 'edge-2'"));

    let duplicate_explicit_edges = GraphMutationPlan::new(vec![
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("KNOWS", "ada", "bob", Props::new()).with_id("edge-1"),
        },
        GraphMutationPlanOp::UpsertEdge {
            kind: GraphMutationPlanKind::Create,
            edge: Edge::new("LIKES", "ada", "carol", Props::new()).with_id("edge-1"),
        },
    ]);
    let error = check_strict_create_plan_conflicts(&duplicate_explicit_edges)
        .expect_err("duplicate CREATE explicit edge id should fail");
    assert!(error.to_string().contains("duplicate edge 'edge-1'"));
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

#[test]
fn cypher_constraint_registry_sql_escapes_values() {
    let create = create_cypher_constraint_registry_table_sql();
    assert!(create.contains(CYPHER_CONSTRAINT_REGISTRY_TABLE));
    assert!(create.contains("registry_json STRING NOT NULL"));

    let upsert =
        upsert_cypher_constraint_registry_sql("default's", r#"{"name":"person's"}"#).unwrap();
    assert!(upsert.contains("MERGE INTO grust_cypher_constraint_registry AS t"));
    assert!(upsert.contains("'default''s' AS name"));
    assert!(upsert.contains(r#"'{"name":"person''s"}' AS registry_json"#));

    let select = select_cypher_constraint_registry_sql("default's").unwrap();
    assert_eq!(
        select,
        "SELECT registry_json FROM grust_cypher_constraint_registry WHERE name = 'default''s' LIMIT 1"
    );

    assert!(upsert_cypher_constraint_registry_sql(" ", "{}").is_err());
    assert!(select_cypher_constraint_registry_sql("").is_err());
}

#[test]
fn parse_optional_single_string_from_arrow_reads_one_value() {
    let schema = Arc::new(ArrowSchema::new(vec![ArrowField::new(
        "registry_json",
        DataType::Utf8,
        false,
    )]));
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from_iter_values([r#"{"named":{}}"#]))],
    )
    .expect("registry batch");

    let value = parse_optional_single_string_from_arrow(
        &[ipc_bytes(&batch).expect("IPC bytes")],
        "registry_json",
        "Cypher constraint registry",
    )
    .expect("parse registry result");
    assert_eq!(value, Some(r#"{"named":{}}"#.to_string()));
    let empty =
        parse_optional_single_string_from_arrow(&[], "registry_json", "Cypher constraint registry")
            .expect("parse empty registry result");
    assert_eq!(empty, None);
}

#[test]
fn parse_optional_single_string_from_arrow_rejects_multiple_rows() {
    let schema = Arc::new(ArrowSchema::new(vec![ArrowField::new(
        "registry_json",
        DataType::Utf8,
        false,
    )]));
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(StringArray::from_iter_values(["{}", "{}"]))],
    )
    .expect("registry batch");

    let error = parse_optional_single_string_from_arrow(
        &[ipc_bytes(&batch).expect("IPC bytes")],
        "registry_json",
        "Cypher constraint registry",
    )
    .expect_err("multiple rows should fail");
    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert!(error.to_string().contains("more than one row"));
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
fn cypher_match_delete_lowers_multiple_relationship_pattern_targets() {
    let plan = sail_cypher_mutation_plan(
        "MATCH (a:Person {id: 'person-1'})-[e:KNOWS]->(b:Person {id: 'person-2'}) DELETE e, a",
    )
    .unwrap();
    assert_eq!(
        plan.report(),
        GraphMutationReport {
            deletes: 2,
            changed_nodes: 1,
            changed_edges: 1,
            node_deletes: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        plan.into_mutations(),
        vec![
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
        "MATCH (a:Person {active: true})-[e:KNOWS]->(b:Person {id: 'person-2'}) DELETE e, a",
        "MATCH (:Person {id: 'person-1'})-[e:KNOWS]->(:Person {id: 'person-2'}) DELETE e,",
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
            op: GraphPredicateOp::IsNotNull,
            value: Value::Null,
        },
        GraphPropertyPredicate {
            key: "retired".to_string(),
            op: GraphPredicateOp::IsNull,
            value: Value::Null,
        },
        GraphPropertyPredicate {
            key: "name".to_string(),
            op: GraphPredicateOp::StartsWith,
            value: Value::from("Ad"),
        },
        GraphPropertyPredicate {
            key: "team".to_string(),
            op: GraphPredicateOp::In,
            value: Value::Json(serde_json::json!(["eng", "data"])),
        },
    ];
    let (sql, args) =
        matching_nodes_sql(Some(&Label::new("Person")), &Props::new(), &predicates).unwrap();

    assert!(sql.contains("label = ?"));
    assert!(sql.contains("GET_JSON_OBJECT(props, '$.status') = ?"));
    assert!(sql.contains("CAST(GET_JSON_OBJECT(props, '$.score') AS BIGINT) >= ?"));
    assert!(sql.contains("GET_JSON_OBJECT(props, '$.nickname') IS NOT NULL"));
    assert!(sql.contains("GET_JSON_OBJECT(props, '$.retired') IS NULL"));
    assert!(sql.contains("STARTSWITH(GET_JSON_OBJECT(props, '$.name'), ?)"));
    assert!(sql.contains(
        "(GET_JSON_OBJECT(props, '$.team') = ? OR GET_JSON_OBJECT(props, '$.team') = ?)"
    ));
    assert_eq!(args.len(), 6);
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
        "MATCH (n:Person) WHERE (n.status = 'inactive' AND n.score >= $min) AND NOT (n.active = true) AND (n.nickname IS NOT NULL) AND n.name STARTS WITH 'Ad' AND n.team IN $teams SET n.archived = true",
        CypherMutationOptions {
            parameters: CypherParameters::from([
                ("min".to_string(), Value::Int(10)),
                (
                    "teams".to_string(),
                    Value::from(vec!["eng".to_string(), "data".to_string()]),
                ),
            ]),
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
                GraphPropertyPredicate {
                    key: "active".to_string(),
                    op: GraphPredicateOp::NotEqual,
                    value: Value::Bool(true),
                },
                GraphPropertyPredicate {
                    key: "nickname".to_string(),
                    op: GraphPredicateOp::IsNotNull,
                    value: Value::Null,
                },
                GraphPropertyPredicate {
                    key: "name".to_string(),
                    op: GraphPredicateOp::StartsWith,
                    value: Value::from("Ad"),
                },
                GraphPropertyPredicate {
                    key: "team".to_string(),
                    op: GraphPredicateOp::In,
                    value: Value::from(vec!["eng".to_string(), "data".to_string()]),
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
        "MATCH (a:Person {id: 'a'})-[e:KNOWS]->(b:Person) WHERE (e.since >= 2020 AND e.source IS NOT NULL AND e.note CONTAINS 'work') AND NOT (b.status ENDS WITH 'blocked') AND NOT b.team IN ['ops'] SET e.seen = true",
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
                    predicates: vec![
                        GraphPropertyPredicate {
                            key: "status".to_string(),
                            op: GraphPredicateOp::NotEndsWith,
                            value: Value::from("blocked"),
                        },
                        GraphPropertyPredicate {
                            key: "team".to_string(),
                            op: GraphPredicateOp::NotIn,
                            value: Value::Json(serde_json::json!(["ops"])),
                        },
                    ],
                },
                id: None,
                props: Props::new(),
                predicates: vec![
                    GraphPropertyPredicate {
                        key: "since".to_string(),
                        op: GraphPredicateOp::GreaterThanOrEqual,
                        value: Value::Int(2020),
                    },
                    GraphPropertyPredicate {
                        key: "source".to_string(),
                        op: GraphPredicateOp::IsNotNull,
                        value: Value::Null,
                    },
                    GraphPropertyPredicate {
                        key: "note".to_string(),
                        op: GraphPredicateOp::Contains,
                        value: Value::from("work"),
                    },
                ],
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
        "MATCH (n:Person) WHERE (n.status = 'inactive' OR n.score >= 10) SET n.archived = true",
        "MATCH (n:Person) WHERE NOT NOT n.active = true SET n.archived = true",
        "MATCH (n:Person) WHERE (n.status = 'inactive' SET n.archived = true",
        "MATCH (n:Person) WHERE size(n.tags) = 2 SET n.archived = true",
        "MATCH (n:Person) WHERE n.active > true SET n.archived = true",
        "MATCH (n:Person) WHERE n.name STARTS WITH 1 SET n.archived = true",
        "MATCH (n:Person) WHERE n.team IN null SET n.archived = true",
        "MATCH (n:Person) WHERE n.team IN [null] SET n.archived = true",
        "MATCH (n:Person) WHERE n.team IN [['eng']] SET n.archived = true",
        "MATCH (n:Person) WHERE m.status = 'inactive' SET n.archived = true",
    ] {
        let error =
            sail_cypher_mutation_plan(cypher).expect_err("unsupported WHERE predicate should fail");
        assert!(is_cypher_planning_error(&error) || matches!(error, GrustError::CypherSyntax(_)));
    }
}

#[test]
fn sail_cypher_match_where_in_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-in-ada', team: 'eng', status: 'active'});
            CREATE (:Person {id: 'where-in-bob', team: 'ops', status: 'active'});
            CREATE (:Person {id: 'where-in-cara', team: 'data', status: 'blocked'});
            CREATE (:Person {id: 'where-in-missing', status: 'active'});
            MATCH (n:Person)
            WHERE n.team IN ['eng', 'data'] AND NOT n.status IN ['blocked']
            SET n.selected = true
            RETURN n.id AS id, n.selected AS selected
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted IN WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-in-ada"), Value::Bool(true)]]
    );
}

#[test]
fn sail_cypher_match_where_parenthesized_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-paren-ada', status: 'inactive', score: 12, active: false});
            CREATE (:Person {id: 'where-paren-bob', status: 'inactive', score: 5, active: false});
            CREATE (:Person {id: 'where-paren-cara', status: 'inactive', score: 14, active: true});
            MATCH (n:Person) WHERE (n.status = 'inactive' AND n.score >= 10) AND NOT (n.active = true)
            SET n.archived = true
            RETURN n.id AS id, n.archived AS archived
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("parenthesized WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-paren-ada"), Value::Bool(true)]]
    );
}

#[test]
fn sail_cypher_match_where_string_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-string-ada', name: 'Ada Lovelace', status: 'active'});
            CREATE (:Person {id: 'where-string-grace', name: 'Grace Hopper', status: 'inactive'});
            CREATE (:Person {id: 'where-string-alan', name: 'Alan Turing', status: 'active'});
            CREATE (:Person {id: 'where-string-missing', status: 'active'});
            MATCH (n:Person)
            WHERE n.name STARTS WITH 'A' AND n.name CONTAINS 'a' AND NOT n.name ENDS WITH 'ing'
            SET n.selected = true
            RETURN n.id AS id, n.selected AS selected
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted string WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-string-ada"), Value::Bool(true)]]
    );
}

#[test]
fn sail_cypher_match_where_not_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-not-ada', active: true});
            CREATE (:Person {id: 'where-not-bob', active: false});
            CREATE (:Person {id: 'where-not-cara'});
            MATCH (n:Person) WHERE NOT n.active = true SET n.archived = true
            RETURN n.id AS id, n.archived AS archived
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted NOT WHERE should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-not-bob"), Value::Bool(true)]]
    );
}

#[test]
fn sail_cypher_match_where_is_null_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-null-ada', nickname: 'Ada'});
            CREATE (:Person {id: 'where-null-bob', nickname: null});
            CREATE (:Person {id: 'where-null-cara'});
            MATCH (n:Person) WHERE n.nickname IS NULL SET n.unset = true
            RETURN n.id AS id, n.unset AS unset
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted IS NULL WHERE should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-null-bob"), Value::Bool(true)],
            vec![Value::from("where-null-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn sail_cypher_match_where_negated_null_checks_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-not-null-negated-ada', nickname: 'Ada'});
            CREATE (:Person {id: 'where-not-null-negated-bob', nickname: null});
            CREATE (:Person {id: 'where-not-null-negated-cara'});
            MATCH (n:Person) WHERE NOT n.nickname IS NOT NULL SET n.unset = true
            RETURN n.id AS id, n.unset AS unset
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("negated restricted IS NOT NULL WHERE should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-null-negated-bob"), Value::Bool(true),],
            vec![
                Value::from("where-not-null-negated-cara"),
                Value::Bool(true),
            ],
        ]
    );
}

#[test]
fn sail_cypher_match_where_is_not_null_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'where-not-null-ada', nickname: 'Ada'});
            CREATE (:Person {id: 'where-not-null-bob', nickname: null});
            CREATE (:Person {id: 'where-not-null-cara'});
            MATCH (n:Person) WHERE n.nickname IS NOT NULL SET n.seen = true
            RETURN n.id AS id, n.seen AS seen
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted IS NOT NULL WHERE should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-not-null-ada"), Value::Bool(true)]]
    );
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
            edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
}

#[test]
fn cypher_match_create_lowers_row_producing_edge_variable() {
    let planned = sail_cypher_mutation_plan_with_return_options(
        "
        MATCH (a:Person {status: 'active'}), (b:Team {id: 'team-1'})
        CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
        RETURN e.label;
        ",
        CypherMutationOptions::default(),
    )
    .unwrap();

    assert_eq!(
        planned.plan.operations,
        vec![GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
            kind: GraphMutationPlanKind::Create,
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("active"))]),
                predicates: Vec::new(),
            },
            to: GraphNodeMatch {
                label: Some(Label::new("Team")),
                props: Props::from([("id".to_string(), Value::from("team-1"))]),
                predicates: Vec::new(),
            },
            label: Label::new("MEMBER_OF"),
            props: Props::from([("source".to_string(), Value::from("cypher"))]),
            edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
            cardinality: GraphMutationCardinality::BoundedMany,
        }]
    );
    assert_eq!(
        planned.row_edge_bindings.get("e"),
        Some(&CypherRowProducedEdgeBinding {
            kind: GraphMutationPlanKind::Create,
            from_variable: "a".to_string(),
            from: GraphNodeMatch {
                label: Some(Label::new("Person")),
                props: Props::from([("status".to_string(), Value::from("active"))]),
                predicates: Vec::new(),
            },
            to_variable: "b".to_string(),
            to: GraphNodeMatch {
                label: Some(Label::new("Team")),
                props: Props::from([("id".to_string(), Value::from("team-1"))]),
                predicates: Vec::new(),
            },
            label: Label::new("MEMBER_OF"),
            props: Props::from([("source".to_string(), Value::from("cypher"))]),
            edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
        })
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
            edge_id_policy: GraphRowEdgeIdPolicy::ExplicitOnly,
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
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) MERGE (a)-[:KNOWS {id: 1}]->(b)",
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
        "MATCH (a:Person {name: 'Ada'}), (b:Person {id: 'person-2'}) CREATE (a)-[:KNOWS {id: 1}]->(b)",
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
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(n:Person {id: 'b'}) SET e.weight = n.weight + 1, e.note = 'x'",
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
fn cypher_match_set_numeric_expression_lowers_edge_updates() {
    let resolved = sail_cypher_mutation_plan(
        "MATCH (:Person {id: 'a'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'b'})
         SET e.weight = e.weight + 1",
    )
    .unwrap();
    let relationship = GraphRelationshipMatch {
        from: GraphNodeMatch {
            label: None,
            props: Props::from([("id".to_string(), Value::from("a"))]),
            predicates: Vec::new(),
        },
        label: Label::new("KNOWS"),
        to: GraphNodeMatch {
            label: None,
            props: Props::from([("id".to_string(), Value::from("b"))]),
            predicates: Vec::new(),
        },
        id: Some(EdgeId::new("edge-1")),
        props: Props::new(),
        predicates: Vec::new(),
    };
    assert_eq!(
        resolved.report(),
        GraphMutationReport {
            patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        resolved.operations,
        vec![GraphMutationPlanOp::UpdateMatchingEdgeProperty {
            relationship: relationship.clone(),
            target_key: "weight".to_string(),
            source_key: "weight".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
            cardinality: GraphMutationCardinality::SingleIdentity,
        }]
    );
    assert_eq!(
        resolved.into_mutations(),
        vec![GraphMutation::UpdateMatchingEdgeProperty {
            relationship,
            target_key: "weight".to_string(),
            source_key: "weight".to_string(),
            op: GraphNumericOp::Add,
            operand: Value::Int(1),
        }]
    );

    let broad = sail_cypher_mutation_plan_with_options(
        "MATCH (:Person {status: 'active'})-[e:KNOWS {active: true}]->(:Person {status: 'active'})
         SET e.weight = e.weight * $factor",
        CypherMutationOptions {
            parameters: CypherParameters::from([("factor".to_string(), Value::Int(2))]),
            ..CypherMutationOptions::default()
        },
    )
    .unwrap()
    .0;
    assert_eq!(
        broad.operations,
        vec![GraphMutationPlanOp::UpdateMatchingEdgeProperty {
            relationship: GraphRelationshipMatch {
                from: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: Vec::new(),
                },
                label: Label::new("KNOWS"),
                to: GraphNodeMatch {
                    label: Some(Label::new("Person")),
                    props: Props::from([("status".to_string(), Value::from("active"))]),
                    predicates: Vec::new(),
                },
                id: None,
                props: Props::from([("active".to_string(), Value::Bool(true))]),
                predicates: Vec::new(),
            },
            target_key: "weight".to_string(),
            source_key: "weight".to_string(),
            op: GraphNumericOp::Multiply,
            operand: Value::Int(2),
            cardinality: GraphMutationCardinality::BoundedMany,
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
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(:Person {id: 'b'}) SET e.weight = n.weight + 1",
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
            node_inserts: 3,
            edge_inserts: 1,
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
fn sail_cypher_multi_target_delete_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new("Person", "delete-a", Props::new()),
            Node::new("Person", "delete-b", Props::new()),
        ],
        vec![Edge::new("KNOWS", "delete-a", "delete-b", Props::new())],
    )))
    .unwrap();

    let plan = sail_cypher_mutation_plan(
        "
        MATCH (a:Person {id: 'delete-a'})-[e:KNOWS]->(b:Person {id: 'delete-b'})
        DELETE e, a;
        ",
    )
    .unwrap();
    let report = futures_executor::block_on(store.execute_cypher_mutation_plan(&plan)).unwrap();

    assert_eq!(
        report,
        GraphMutationReport {
            deletes: 2,
            changed_nodes: 1,
            changed_edges: 1,
            node_deletes: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("delete-a")))
            .unwrap()
            .is_none()
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("delete-b")))
            .unwrap()
            .is_some()
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery {
            from: Some(NodeId::new("delete-a")),
            to: Some(NodeId::new("delete-b")),
            label: Some(Label::new("KNOWS")),
        }))
        .unwrap()
        .is_empty()
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
            node_inserts: 1,
            ..GraphMutationReport::default()
        }
    );
    let node = futures_executor::block_on(store.get_node(&NodeId::new("c1")))
        .unwrap()
        .expect("counter node");
    assert_eq!(node.props.get("count"), Some(&Value::Int(4)));
}

#[test]
fn sail_cypher_returning_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', name: 'Ada', order: 'first', limit: 3});
            MATCH (n:Person {id: 'ada'})
            SET n.seen = true, n.count = 1
            RETURN n.id, n.label, n.seen AS seen, n.order, n.limit, n.missing;
            ",
            CypherMutationOptions {
                collect_written_node_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .unwrap();

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            creates: 1,
            patches: 2,
            changed_nodes: 3,
            node_upserts: 1,
            node_patches: 2,
            node_inserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.mutation.written_node_identities,
        vec![CypherWrittenNodeIdentity {
            kind: GraphMutationPlanKind::Create,
            label: Label::new("Person"),
            id: NodeId::new("ada"),
        }]
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "n.id".to_string(),
                "n.label".to_string(),
                "seen".to_string(),
                "n.order".to_string(),
                "n.limit".to_string(),
                "n.missing".to_string()
            ],
            rows: vec![vec![
                Value::from("ada"),
                Value::from("Person"),
                Value::Bool(true),
                Value::from("first"),
                Value::Int(3),
                Value::Null,
            ]],
        }
    );
}

#[test]
fn sail_cypher_returning_projects_bound_edge_properties_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada'});
            CREATE (:Person {id: 'bob'});
            CREATE (:Person {id: 'ada'})-[:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'});
            MATCH (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1'}]->(:Person {id: 'bob'})
            SET e.weight = 2
            RETURN e.id, e.label, e.weight, e.missing;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            creates: 3,
            patches: 1,
            changed_nodes: 2,
            changed_edges: 2,
            node_upserts: 2,
            edge_upserts: 1,
            edge_patches: 1,
            node_inserts: 2,
            edge_inserts: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "e.id".to_string(),
                "e.label".to_string(),
                "e.weight".to_string(),
                "e.missing".to_string()
            ],
            rows: vec![vec![
                Value::from("edge-1"),
                Value::from("KNOWS"),
                Value::Int(2),
                Value::Null
            ]],
        }
    );
}

#[test]
fn sail_cypher_numeric_edge_updates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let resolved =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'edge-num-a'});
            CREATE (b:Person {id: 'edge-num-b'});
            CREATE (a)-[e:KNOWS {id: 'edge-num-1', weight: 2}]->(b);
            MATCH (a:Person {id: 'edge-num-a'})-[e:KNOWS {id: 'edge-num-1'}]->(b:Person {id: 'edge-num-b'})
            SET e.weight = e.weight + 3
            RETURN e.weight AS weight;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("resolved edge numeric update");
    assert_eq!(
        resolved.table,
        CypherResultTable {
            columns: vec!["weight".to_string()],
            rows: vec![vec![Value::Int(5)]],
        }
    );
    assert_eq!(resolved.mutation.report.edge_patches, 1);
    assert_eq!(resolved.mutation.report.changed_edges, 2);

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'edge-num-c', status: 'edge-num'});
            CREATE (:Person {id: 'edge-num-d', status: 'edge-num'});
            CREATE (:Person {id: 'edge-num-e', status: 'edge-num'});
            CREATE (:Person {id: 'edge-num-c'})-[:LIKES {active: true, weight: 2}]->(:Person {id: 'edge-num-e'});
            CREATE (:Person {id: 'edge-num-d'})-[:LIKES {active: true, weight: 4}]->(:Person {id: 'edge-num-e'});
            MATCH (n:Person {status: 'edge-num'})-[e:LIKES {active: true}]->(t:Person {id: 'edge-num-e'})
            SET e.weight = e.weight * $factor
            RETURN e.weight AS weight
            ORDER BY weight;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("factor".to_string(), Value::Int(2))]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad edge numeric update");
    assert_eq!(
        broad.table,
        CypherResultTable {
            columns: vec!["weight".to_string()],
            rows: vec![vec![Value::Int(4)], vec![Value::Int(8)]],
        }
    );
    assert_eq!(broad.mutation.report.matched_rows, 2);
    assert_eq!(broad.mutation.report.edge_patches, 2);
    assert_eq!(broad.mutation.report.changed_edges, 4);
}

#[test]
fn sail_cypher_returning_projects_new_concrete_edge_properties_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let top_level =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'ada'});
            CREATE (b:Person {id: 'bob'});
            CREATE (a)-[e:KNOWS {id: 'edge-1', since: 2026}]->(b)
            RETURN e.id, e.since;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        top_level.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.since".to_string()],
            rows: vec![vec![Value::from("edge-1"), Value::Int(2026)]],
        }
    );

    let match_create =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (a:Person {id: 'ada'}), (b:Person {id: 'bob'})
            CREATE (a)-[e:WORKS_WITH {id: 'edge-2', weight: 4}]->(b)
            RETURN e.id, e.weight;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        match_create.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.weight".to_string()],
            rows: vec![vec![Value::from("edge-2"), Value::Int(4)]],
        }
    );
}

#[test]
fn sail_cypher_returning_projects_bound_elements_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'ada', name: 'Ada'});
            CREATE (b:Person {id: 'bob'});
            CREATE (a)-[e:KNOWS {id: 'edge-1', since: 2026}]->(b)
            RETURN a AS node, e AS relationship;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["node".to_string(), "relationship".to_string()],
            rows: vec![vec![
                Value::from(serde_json::json!({
                    "id": "ada",
                    "label": "Person",
                    "props": {
                        "id": {"type": "string", "value": "ada"},
                        "name": {"type": "string", "value": "Ada"}
                    }
                })),
                Value::from(serde_json::json!({
                    "id": "edge-1",
                    "from": "ada",
                    "to": "bob",
                    "label": "KNOWS",
                    "props": {
                        "id": {"type": "string", "value": "edge-1"},
                        "since": {"type": "int", "value": 2026}
                    }
                }))
            ]],
        }
    );
}

#[test]
fn sail_cypher_returning_projects_star_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'star-ada', name: 'Ada'});
            CREATE (b:Person {id: 'star-bob'});
            CREATE (a)-[e:KNOWS {id: 'star-edge', since: 2026}]->(b)
            RETURN *;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete RETURN *");
    assert_eq!(
        concrete.table.columns,
        vec!["a".to_string(), "b".to_string(), "e".to_string()]
    );
    assert_eq!(concrete.table.rows.len(), 1);
    assert_eq!(concrete.table.rows[0].len(), 3);
    let Value::Json(a) = &concrete.table.rows[0][0] else {
        panic!("RETURN * should project concrete node a");
    };
    let Value::Json(b) = &concrete.table.rows[0][1] else {
        panic!("RETURN * should project concrete node b");
    };
    let Value::Json(e) = &concrete.table.rows[0][2] else {
        panic!("RETURN * should project concrete relationship e");
    };
    assert_eq!(a["id"], serde_json::json!("star-ada"));
    assert_eq!(b["id"], serde_json::json!("star-bob"));
    assert_eq!(e["id"], serde_json::json!("star-edge"));

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'star-cara', status: 'active'});
            CREATE (:Person {id: 'star-dana', status: 'active'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN *, n.id AS id ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad node RETURN *");
    assert_eq!(broad.table.columns, vec!["n".to_string(), "id".to_string()]);
    assert_eq!(broad.table.rows.len(), 2);
    assert_eq!(broad.table.rows[0][1], Value::from("star-cara"));
    assert_eq!(broad.table.rows[1][1], Value::from("star-dana"));
    let Value::Json(n) = &broad.table.rows[0][0] else {
        panic!("RETURN * should project broad node n");
    };
    assert_eq!(n["id"], serde_json::json!("star-cara"));

    let row_edge =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'star-team'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'star-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'star'}]->(t)
            RETURN *, r.source AS source;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship RETURN *");
    assert_eq!(
        row_edge.table.columns,
        vec![
            "n".to_string(),
            "r".to_string(),
            "t".to_string(),
            "source".to_string()
        ]
    );
    assert_eq!(row_edge.table.rows.len(), 2);
    for row in &row_edge.table.rows {
        let Value::Json(person) = &row[0] else {
            panic!("RETURN * should project matched source n");
        };
        let Value::Json(edge) = &row[1] else {
            panic!("RETURN * should project row-producing relationship r");
        };
        let Value::Json(team) = &row[2] else {
            panic!("RETURN * should project matched endpoint t");
        };
        assert!(["star-cara", "star-dana"].contains(&person["id"].as_str().expect("person id")));
        assert_eq!(edge["label"], serde_json::json!("MEMBER_OF"));
        assert_eq!(team["id"], serde_json::json!("star-team"));
        assert_eq!(row[3], Value::from("star"));
    }

    let explicit_source =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'star-team'})
            MERGE (n)-[r:WORKS_ON {source: 'explicit'}]->(t)
            RETURN n.id AS person, r.source AS source ORDER BY person;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship source endpoint RETURN");
    assert_eq!(
        explicit_source.table,
        CypherResultTable {
            columns: vec!["person".to_string(), "source".to_string()],
            rows: vec![
                vec![Value::from("star-cara"), Value::from("explicit")],
                vec![Value::from("star-dana"), Value::from("explicit")]
            ],
        }
    );
}

#[test]
fn sail_cypher_returning_row_model_preserves_alignment_and_star_order() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'row-model-ada', status: 'row-model'});
            CREATE (:Person {id: 'row-model-bob', status: 'row-model'});
            CREATE (:Team {id: 'row-model-eng', kind: 'row-model'});
            CREATE (:Team {id: 'row-model-ops', kind: 'row-model'});
            MATCH (n:Person {status: 'row-model'}), (t:Team {kind: 'row-model'})
            CREATE (n)-[r:ASSIGNED {source: 'row-model'}]->(t)
            RETURN *, n.id AS person, t.id AS team
            ORDER BY person, team;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing RETURN should preserve row alignment");

    assert_eq!(
        result.table.columns,
        vec![
            "n".to_string(),
            "r".to_string(),
            "t".to_string(),
            "person".to_string(),
            "team".to_string()
        ]
    );
    assert_eq!(result.table.rows.len(), 4);
    for row in &result.table.rows {
        let Value::Json(person) = &row[0] else {
            panic!("RETURN * should include source endpoint node");
        };
        let Value::Json(edge) = &row[1] else {
            panic!("RETURN * should include produced relationship");
        };
        let Value::Json(team) = &row[2] else {
            panic!("RETURN * should include target endpoint node");
        };
        assert_eq!(person["id"], row[3].to_json());
        assert_eq!(team["id"], row[4].to_json());
        assert_eq!(edge["from"], row[3].to_json());
        assert_eq!(edge["to"], row[4].to_json());
    }

    let collected =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'row-model'}), (t:Team {kind: 'row-model'})
            MERGE (n)-[r:ASSIGNED_AGAIN {source: 'row-model'}]->(t)
            RETURN collect(*) AS rows;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("collect(*) should use the same row model");
    let Value::Json(rows) = &collected.table.rows[0][0] else {
        panic!("collect(*) should return JSON rows");
    };
    let rows = rows.as_array().expect("collect(*) array");
    assert_eq!(rows.len(), 4);
    let first = rows[0].as_object().expect("row object");
    assert_eq!(
        first.keys().cloned().collect::<Vec<_>>(),
        vec!["n".to_string(), "r".to_string(), "t".to_string()]
    );
}

#[test]
fn sail_cypher_returning_projects_row_producing_paths_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'path-ada', status: 'path'});
            CREATE (:Person {id: 'path-bob', status: 'path'});
            CREATE (:Team {id: 'path-team'});
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[r:MEMBER_OF {source: 'path'}]->(t)
            RETURN p,
                   length(p) AS hops,
                   nodes(p) AS path_nodes,
                   relationships(p) AS path_relationships,
                   n.id AS person,
                   r.source AS source
            ORDER BY person;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing path RETURN");

    assert_eq!(
        result.table.columns,
        vec![
            "p".to_string(),
            "hops".to_string(),
            "path_nodes".to_string(),
            "path_relationships".to_string(),
            "person".to_string(),
            "source".to_string()
        ]
    );
    assert_eq!(result.table.rows.len(), 2);
    assert_eq!(result.table.rows[0][4], Value::from("path-ada"));
    assert_eq!(result.table.rows[1][4], Value::from("path-bob"));
    for row in &result.table.rows {
        let Value::Json(path) = &row[0] else {
            panic!("RETURN p should project a JSON path");
        };
        assert_eq!(row[1], Value::Int(1));
        assert_eq!(path["nodes"][0]["id"], row[4].to_json());
        assert_eq!(path["nodes"][1]["id"], serde_json::json!("path-team"));
        assert_eq!(path["relationships"][0]["from"], row[4].to_json());
        assert_eq!(
            path["relationships"][0]["to"],
            serde_json::json!("path-team")
        );
        assert_eq!(
            path["relationships"][0]["label"],
            serde_json::json!("MEMBER_OF")
        );
        assert_eq!(row[2].to_json(), path["nodes"]);
        assert_eq!(row[3].to_json(), path["relationships"]);
        assert_eq!(row[5], Value::from("path"));
    }

    let star = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            MERGE q = (n)-[r:WORKS_ON {source: 'path-star'}]->(t)
            RETURN *
            LIMIT 1;
            ",
        CypherMutationOptions::default(),
    ))
    .expect("row-producing path RETURN *");
    assert_eq!(
        star.table.columns,
        vec![
            "n".to_string(),
            "q".to_string(),
            "r".to_string(),
            "t".to_string()
        ]
    );
    let Value::Json(path) = &star.table.rows[0][1] else {
        panic!("RETURN * should include the path variable");
    };
    assert_eq!(
        path["relationships"][0]["label"],
        serde_json::json!("WORKS_ON")
    );

    let resolved_path =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'path-resolved-a'});
            CREATE (b:Person {id: 'path-resolved-b'});
            MATCH (a:Person {id: 'path-resolved-a'}), (b:Person {id: 'path-resolved-b'})
            CREATE p = (a)-[r:KNOWS {id: 'path-resolved-r'}]->(b)
            RETURN p,
                   length(p) AS hops,
                   nodes(p) AS path_nodes,
                   relationships(p) AS path_relationships,
                   count(p) AS path_count,
                   collect(p) AS paths;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("resolved relationship path variables should project");
    assert_eq!(
        resolved_path.table.columns,
        vec![
            "p".to_string(),
            "hops".to_string(),
            "path_nodes".to_string(),
            "path_relationships".to_string(),
            "path_count".to_string(),
            "paths".to_string()
        ]
    );
    assert_eq!(resolved_path.table.rows.len(), 1);
    assert_eq!(resolved_path.table.rows[0][1], Value::Int(1));
    assert_eq!(resolved_path.table.rows[0][4], Value::Int(1));
    let Value::Json(path) = &resolved_path.table.rows[0][0] else {
        panic!("resolved RETURN p should project a JSON path");
    };
    assert_eq!(path["nodes"][0]["id"], serde_json::json!("path-resolved-a"));
    assert_eq!(path["nodes"][1]["id"], serde_json::json!("path-resolved-b"));
    assert_eq!(
        path["relationships"][0]["id"],
        serde_json::json!("path-resolved-r")
    );
    assert_eq!(resolved_path.table.rows[0][2].to_json(), path["nodes"]);
    assert_eq!(
        resolved_path.table.rows[0][3].to_json(),
        path["relationships"]
    );
    let Value::Json(paths) = &resolved_path.table.rows[0][5] else {
        panic!("resolved collect(p) should return JSON paths");
    };
    assert_eq!(paths.as_array().expect("resolved paths array").len(), 1);

    let path_function_on_node =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}) SET n.path_function_checked = true
            RETURN nodes(n);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("path functions should require path variables");
    assert!(
        matches!(
            path_function_on_node,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{path_function_on_node:?}"
    );

    let missing_relationship_variable =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[:MISSING_VAR]->(t)
            RETURN p;
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("path variables should require a relationship variable");
    assert!(
        matches!(missing_relationship_variable, GrustError::CypherSyntax(_)),
        "{missing_relationship_variable:?}"
    );

    let path_property =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[r:PATH_PROPERTY]->(t)
            RETURN p.id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("path properties should stay deferred");
    assert!(
        matches!(path_property, GrustError::CypherUnsupportedCardinality(_)),
        "{path_property:?}"
    );

    let path_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[r:PATH_AGGREGATE]->(t)
            RETURN count(p) AS path_count,
                   count(DISTINCT p) AS distinct_path_count,
                   collect(p) AS paths;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted path aggregates should be supported");
    assert_eq!(
        path_aggregates.table.columns,
        vec![
            "path_count".to_string(),
            "distinct_path_count".to_string(),
            "paths".to_string()
        ]
    );
    assert_eq!(path_aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(path_aggregates.table.rows[0][1], Value::Int(2));
    let Value::Json(paths) = &path_aggregates.table.rows[0][2] else {
        panic!("collect(p) should return JSON paths");
    };
    let paths = paths.as_array().expect("path collection");
    assert_eq!(paths.len(), 2);
    assert_eq!(
        paths[0]["relationships"][0]["label"],
        serde_json::json!("PATH_AGGREGATE")
    );

    let path_function_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[r:PATH_FUNCTION_AGGREGATE]->(t)
            RETURN sum(length(p)) AS total_hops,
                   avg(length(p)) AS average_hops,
                   count(DISTINCT length(p)) AS distinct_lengths,
                   collect(nodes(p)) AS node_paths,
                   collect(relationships(p)) AS relationship_paths;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted path function aggregates should be supported");
    assert_eq!(
        path_function_aggregates.table.columns,
        vec![
            "total_hops".to_string(),
            "average_hops".to_string(),
            "distinct_lengths".to_string(),
            "node_paths".to_string(),
            "relationship_paths".to_string()
        ]
    );
    assert_eq!(path_function_aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(path_function_aggregates.table.rows[0][1], Value::Float(1.0));
    assert_eq!(path_function_aggregates.table.rows[0][2], Value::Int(1));
    let Value::Json(node_paths) = &path_function_aggregates.table.rows[0][3] else {
        panic!("collect(nodes(p)) should return JSON arrays");
    };
    let node_paths = node_paths.as_array().expect("node path collection");
    assert_eq!(node_paths.len(), 2);
    assert_eq!(node_paths[0].as_array().expect("node array").len(), 2);
    let Value::Json(relationship_paths) = &path_function_aggregates.table.rows[0][4] else {
        panic!("collect(relationships(p)) should return JSON arrays");
    };
    let relationship_paths = relationship_paths
        .as_array()
        .expect("relationship path collection");
    assert_eq!(relationship_paths.len(), 2);
    assert_eq!(
        relationship_paths[0][0]["label"],
        serde_json::json!("PATH_FUNCTION_AGGREGATE")
    );

    let path_function_aggregate_on_node =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}) SET n.path_function_aggregate_checked = true
            RETURN count(length(n));
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("path function aggregates should require path variables");
    assert!(
        matches!(
            path_function_aggregate_on_node,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{path_function_aggregate_on_node:?}"
    );

    let grouped_path_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'path'}), (t:Team {id: 'path-team'})
            CREATE p = (n)-[r:PATH_GROUP]->(t)
            RETURN n.id AS person, count(p) AS path_count, collect(p) AS paths
            ORDER BY person;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("grouped path aggregates should be supported");
    assert_eq!(
        grouped_path_aggregates.table.columns,
        vec![
            "person".to_string(),
            "path_count".to_string(),
            "paths".to_string()
        ]
    );
    assert_eq!(grouped_path_aggregates.table.rows.len(), 2);
    for row in &grouped_path_aggregates.table.rows {
        assert_eq!(row[1], Value::Int(1));
        let Value::Json(paths) = &row[2] else {
            panic!("grouped collect(p) should return JSON paths");
        };
        let paths = paths.as_array().expect("grouped path collection");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0]["nodes"][0]["id"], row[0].to_json());
        assert_eq!(
            paths[0]["relationships"][0]["label"],
            serde_json::json!("PATH_GROUP")
        );
    }
}

#[test]
fn sail_cypher_returning_projects_restricted_maps_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'map-ada', name: 'Ada'})
            RETURN n { .id, .label, display: n.name, marker: 'seen', rank: 1, active: true, fallback: $fallback, empty: null, .missing } AS person;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "fallback".to_string(),
                    Value::from("provided"),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete map projection");
    assert_eq!(concrete.table.columns, vec!["person"]);
    assert_eq!(
        concrete.table.rows,
        vec![vec![Value::Json(serde_json::json!({
            "id": "map-ada",
            "label": "Person",
            "display": "Ada",
            "marker": "seen",
            "rank": 1,
            "active": true,
            "fallback": "provided",
            "empty": null,
            "missing": null
        }))]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'map-bob', status: 'active', team: 'eng'});
            CREATE (:Person {id: 'map-cara', status: 'active', team: 'ops'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN n.id AS id, n { .id, kind: 'person', team: n.team, .seen } AS person ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad row map projection");
    assert_eq!(broad.table.columns, vec!["id", "person"]);
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("map-bob"),
                Value::Json(serde_json::json!({
                    "id": "map-bob",
                    "kind": "person",
                    "team": "eng",
                    "seen": true
                }))
            ],
            vec![
                Value::from("map-cara"),
                Value::Json(serde_json::json!({
                    "id": "map-cara",
                    "kind": "person",
                    "team": "ops",
                    "seen": true
                }))
            ]
        ]
    );

    let row_edge =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'map-team'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'map-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'map'}]->(t)
            RETURN n.id AS id,
                   n { .id, kind: 'person', team: n.team } AS person,
                   r { .label, source: r.source, static: 'map-entry' } AS membership
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing endpoint and relationship map projections");
    assert_eq!(
        row_edge.table.columns,
        vec![
            "id".to_string(),
            "person".to_string(),
            "membership".to_string()
        ]
    );
    assert_eq!(row_edge.table.rows.len(), 2);
    assert_eq!(
        row_edge.table.rows[0],
        vec![
            Value::from("map-bob"),
            Value::Json(serde_json::json!({"id": "map-bob", "kind": "person", "team": "eng"})),
            Value::Json(
                serde_json::json!({"label": "MEMBER_OF", "source": "map", "static": "map-entry"})
            )
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'active'}) SET n.map_aggregated = true
            RETURN count(n { team: n.team, marker: 'seen' }) AS mapped_rows,
                   count(DISTINCT n { team: n.team, marker: 'seen' }) AS distinct_maps,
                   collect(n { .id, kind: 'person', team: n.team }) AS people;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("map projection aggregates");
    assert_eq!(
        aggregates.table.columns,
        vec![
            "mapped_rows".to_string(),
            "distinct_maps".to_string(),
            "people".to_string()
        ]
    );
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
    let Value::Json(people) = &aggregates.table.rows[0][2] else {
        panic!("collect(map projection) should return JSON array");
    };
    assert_eq!(people.as_array().expect("people maps").len(), 2);

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'map-invalid'}) RETURN n { id };",
            CypherMutationOptions::default(),
        ))
        .expect_err("map projection expressions should stay restricted");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let cross_variable =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'map-cross-a'});
            CREATE (b:Person {id: 'map-cross-b'})
            RETURN a { other: b.id };
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("map projection entries should reject cross-variable properties");
    assert!(
        matches!(cross_variable, GrustError::CypherUnsupportedCardinality(_)),
        "{cross_variable:?}"
    );

    let duplicate_key =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'map-duplicate', team: 'eng'}) RETURN n { .team, team: 'dup' };",
            CypherMutationOptions::default(),
        ))
        .expect_err("map projection entries should reject duplicate output keys");
    assert!(
        matches!(duplicate_key, GrustError::CypherUnsupportedCardinality(_)),
        "{duplicate_key:?}"
    );

    let nested =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'map-nested'}) RETURN n { nested: {value: 1} };",
            CypherMutationOptions::default(),
        ))
        .expect_err("map projection entries should reject nested maps");
    assert!(
        matches!(nested, GrustError::CypherUnsupportedCardinality(_)),
        "{nested:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_lists_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'list-ada', name: 'Ada'})
            RETURN [n.id, n.label, n.name, 'seen', 1, true, null, $marker, n.missing] AS person;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("marker".to_string(), Value::from("param"))]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list projection");
    assert_eq!(concrete.table.columns, vec!["person"]);
    assert_eq!(
        concrete.table.rows,
        vec![vec![Value::Json(serde_json::json!([
            "list-ada", "Person", "Ada", "seen", 1, true, null, "param", null
        ]))]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'list-bob', status: 'active', team: 'eng'});
            CREATE (:Person {id: 'list-cara', status: 'active', team: 'ops'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN n.id AS id, [n.id, 'team', n.team, n.seen] AS person ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad row list projection");
    assert_eq!(broad.table.columns, vec!["id", "person"]);
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("list-bob"),
                Value::Json(serde_json::json!(["list-bob", "team", "eng", true]))
            ],
            vec![
                Value::from("list-cara"),
                Value::Json(serde_json::json!(["list-cara", "team", "ops", true]))
            ]
        ]
    );

    let row_edge =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'list-team'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'list-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'list'}]->(t)
            RETURN n.id AS id, [n.id, 'team', n.team] AS person, [r.label, 'source', r.source] AS membership
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing endpoint and relationship list projections");
    assert_eq!(
        row_edge.table.columns,
        vec![
            "id".to_string(),
            "person".to_string(),
            "membership".to_string()
        ]
    );
    assert_eq!(row_edge.table.rows.len(), 2);
    assert_eq!(
        row_edge.table.rows[0],
        vec![
            Value::from("list-bob"),
            Value::Json(serde_json::json!(["list-bob", "team", "eng"])),
            Value::Json(serde_json::json!(["MEMBER_OF", "source", "list"]))
        ]
    );

    let literal_only =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'list-literal-only'})
            RETURN ['literal', 1, false, null] AS values;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("literal-only list projection");
    assert_eq!(
        literal_only.table.rows,
        vec![vec![Value::Json(serde_json::json!([
            "literal", 1, false, null
        ]))]]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'active'}) SET n.list_aggregated = true
            RETURN count([n.team, 'seen']) AS listed_rows,
                   count(DISTINCT [n.team, 'seen']) AS distinct_lists,
                   collect([n.id, 'team', n.team]) AS people;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list projection aggregates");
    assert_eq!(
        aggregates.table.columns,
        vec![
            "listed_rows".to_string(),
            "distinct_lists".to_string(),
            "people".to_string()
        ]
    );
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
    let Value::Json(people) = &aggregates.table.rows[0][2] else {
        panic!("collect(list projection) should return JSON array");
    };
    assert_eq!(people.as_array().expect("people lists").len(), 2);

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'list-a'});
            CREATE (b:Person {id: 'list-b'})
            RETURN [a.id, b.id];
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("cross-variable list projections should stay restricted");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let nested =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'list-nested'})
            RETURN [n.id, [1, 2]];
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested list projections should stay restricted");
    assert!(
        matches!(nested, GrustError::CypherUnsupportedCardinality(_)),
        "{nested:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_literals_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'literal-ada', team: 'eng'})
            RETURN 'created' AS status, 1 AS one, true AS ok, null AS empty, n.id AS id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete literal projections");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec![
                "status".to_string(),
                "one".to_string(),
                "ok".to_string(),
                "empty".to_string(),
                "id".to_string(),
            ],
            rows: vec![vec![
                Value::from("created"),
                Value::Int(1),
                Value::Bool(true),
                Value::Null,
                Value::from("literal-ada"),
            ]],
        }
    );

    let parameterized =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'literal-param', team: 'ops'})
            RETURN $status AS status, $score AS score, n.team AS team;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("status".to_string(), Value::from("accepted")),
                    ("score".to_string(), Value::Int(7)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("parameterized literal projections");
    assert_eq!(
        parameterized.table.rows,
        vec![vec![
            Value::from("accepted"),
            Value::Int(7),
            Value::from("ops")
        ]]
    );

    let ranges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:RangeProbe {id: 'literal-range'})
            RETURN range(1, 4) AS ascending,
                   range($start, $end, $step) AS descending,
                   range(4, 1) AS empty_range;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("start".to_string(), Value::Int(5)),
                    ("end".to_string(), Value::Int(1)),
                    ("step".to_string(), Value::Int(-2)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("range literal projections");
    assert_eq!(
        ranges.table.rows,
        vec![vec![
            Value::IntArray(vec![1, 2, 3, 4]),
            Value::IntArray(vec![5, 3, 1]),
            Value::IntArray(vec![]),
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person) SET n.literal_seen = true
            RETURN n.team AS team, 'seen' AS status, count(1) AS rows
            ORDER BY team;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("grouped literal projection");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("eng"), Value::from("seen"), Value::Int(1)],
            vec![Value::from("ops"), Value::from("seen"), Value::Int(1)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person) SET n.literal_counted = true
            RETURN count(1) AS counted,
                   count(DISTINCT 'x') AS distinct_literal,
                   count(null) AS non_null,
                   sum(1) AS summed,
                   avg(2) AS averaged,
                   collect('x') AS collected,
                   count(range(1, 2)) AS range_count,
                   collect(range(1, 2)) AS ranges;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("literal aggregate projections");
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
    assert_eq!(aggregates.table.rows[0][2], Value::Int(0));
    assert_eq!(aggregates.table.rows[0][3], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][4], Value::Float(2.0));
    assert_eq!(
        aggregates.table.rows[0][5],
        Value::Json(serde_json::json!(["x", "x"]))
    );
    assert_eq!(aggregates.table.rows[0][6], Value::Int(2));
    assert_eq!(
        aggregates.table.rows[0][7],
        Value::Json(serde_json::json!([[1, 2], [1, 2]]))
    );

    let zero_step =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:RangeProbe {id: 'literal-range-zero'}) RETURN range(1, 3, 0);",
            CypherMutationOptions::default(),
        ))
        .expect_err("range zero step should stay rejected");
    assert!(
        matches!(zero_step, GrustError::CypherUnsupportedCardinality(_)),
        "{zero_step:?}"
    );

    let non_integer =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:RangeProbe {id: 'literal-range-float'}) RETURN range(1.5, 3);",
            CypherMutationOptions::default(),
        ))
        .expect_err("range float arguments should stay rejected");
    assert!(
        matches!(non_integer, GrustError::CypherUnsupportedCardinality(_)),
        "{non_integer:?}"
    );

    let numeric_range_aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person) SET n.literal_range_sum = true RETURN sum(range(1, 2));",
            CypherMutationOptions::default(),
        ))
        .expect_err("numeric aggregates over range arrays should stay rejected");
    assert!(
        matches!(
            numeric_range_aggregate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_range_aggregate:?}"
    );

    let missing_parameter =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'literal-missing'}) RETURN $missing;",
            CypherMutationOptions::default(),
        ))
        .expect_err("missing RETURN parameter should fail");
    assert!(
        matches!(missing_parameter, GrustError::CypherUnresolvedIdentity(_)),
        "{missing_parameter:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_coalesce_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'coalesce-ada', name: 'Ada'})
            RETURN coalesce(n.nickname, n.name, 'unknown') AS display,
                   coalesce(null, $fallback) AS fallback;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "fallback".to_string(),
                    Value::from("provided"),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete coalesce projection");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec!["display".to_string(), "fallback".to_string()],
            rows: vec![vec![Value::from("Ada"), Value::from("provided")]],
        }
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'coalesce-bob', status: 'coalesce', name: 'Bob'});
            CREATE (:Person {id: 'coalesce-cara', status: 'coalesce', nickname: 'C'});
            MATCH (n:Person {status: 'coalesce'}) SET n.seen = true
            RETURN n.id AS id, coalesce(n.nickname, n.name, 'unknown') AS display
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad coalesce projection");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("coalesce-bob"), Value::from("Bob")],
            vec![Value::from("coalesce-cara"), Value::from("C")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'coalesce'}) SET n.coalesced = true
            RETURN count(coalesce(n.nickname, n.name)) AS named,
                   count(DISTINCT coalesce(n.nickname, n.name)) AS distinct_names,
                   min(coalesce(n.nickname, n.name)) AS first_name,
                   max(coalesce(n.nickname, n.name)) AS last_name,
                   collect(coalesce(n.nickname, n.name)) AS names;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("coalesce aggregate projections");
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][2], Value::from("Bob"));
    assert_eq!(aggregates.table.rows[0][3], Value::from("C"));
    assert_eq!(
        aggregates.table.rows[0][4],
        Value::Json(serde_json::json!(["Bob", "C"]))
    );

    let cross_variable =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'coalesce-a'});
            CREATE (b:Person {id: 'coalesce-b'})
            RETURN coalesce(a.name, b.name, 'unknown');
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("cross-variable coalesce should stay restricted");
    assert!(
        matches!(cross_variable, GrustError::CypherUnsupportedCardinality(_)),
        "{cross_variable:?}"
    );

    let nested =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'coalesce-nested'}) RETURN coalesce(length(n), 'unknown');",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested coalesce arguments should stay restricted");
    assert!(
        matches!(nested, GrustError::CypherUnsupportedCardinality(_)),
        "{nested:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_element_functions_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'function-ada'});
            CREATE (b:Person {id: 'function-bob'});
            MATCH (a:Person {id: 'function-ada'}), (b:Person {id: 'function-bob'})
            CREATE (a)-[e:KNOWS {id: 'function-knows'}]->(b)
            RETURN labels(a) AS node_labels, type(e) AS relationship_type;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete element function projections");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec!["node_labels".to_string(), "relationship_type".to_string()],
            rows: vec![vec![
                Value::Json(serde_json::json!(["Person"])),
                Value::from("KNOWS"),
            ]],
        }
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'function-cara', status: 'element-functions'});
            CREATE (:Person {id: 'function-dan', status: 'element-functions'});
            MATCH (n:Person {status: 'element-functions'}) SET n.seen = true
            RETURN n.id AS id, labels(n) AS labels
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad node labels projection");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("function-cara"),
                Value::Json(serde_json::json!(["Person"]))
            ],
            vec![
                Value::from("function-dan"),
                Value::Json(serde_json::json!(["Person"]))
            ],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'function-team'});
            MATCH (n:Person {status: 'element-functions'}), (t:Team {id: 'function-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'function'}]->(t)
            RETURN n.id AS id, type(r) AS relationship_type
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship type projection");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("function-cara"), Value::from("MEMBER_OF")],
            vec![Value::from("function-dan"), Value::from("MEMBER_OF")],
        ]
    );

    let node_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'element-functions'}) SET n.label_counted = true
            RETURN count(labels(n)) AS labelled_nodes,
                   collect(labels(n)) AS node_labels;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("node labels aggregate projections");
    assert_eq!(node_aggregates.table.rows[0][0], Value::Int(2));

    let relationship_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (:Person {status: 'element-functions'})-[r:MEMBER_OF]->(:Team {id: 'function-team'})
            SET r.checked = true
            RETURN
                   count(DISTINCT type(r)) AS relationship_types,
                   collect(type(r)) AS relationships;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("relationship type aggregate projections");
    assert_eq!(relationship_aggregates.table.rows[0][0], Value::Int(1));
    assert_eq!(
        relationship_aggregates.table.rows[0][1],
        Value::Json(serde_json::json!(["MEMBER_OF", "MEMBER_OF"]))
    );

    let labels_on_edge =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'element-functions'}), (t:Team {id: 'function-team'})
            CREATE (n)-[r:REJECTED_FUNCTION_TARGET]->(t)
            RETURN labels(r);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("labels on relationship variables should stay rejected");
    assert!(
        matches!(labels_on_edge, GrustError::CypherUnsupportedCardinality(_)),
        "{labels_on_edge:?}"
    );

    let type_on_node =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'element-functions'}) SET n.rejected = true
            RETURN type(n);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("type on node variables should stay rejected");
    assert!(
        matches!(type_on_node, GrustError::CypherUnsupportedCardinality(_)),
        "{type_on_node:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_properties_and_keys_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'props-ada', name: 'Ada', team: 'eng'});
            CREATE (b:Person {id: 'props-bob'});
            MATCH (a:Person {id: 'props-ada'}), (b:Person {id: 'props-bob'})
            CREATE (a)-[e:KNOWS {id: 'props-knows', since: 2026}]->(b)
            RETURN properties(a) AS node_props,
                   keys(a) AS node_keys,
                   properties(e) AS edge_props,
                   keys(e) AS edge_keys;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete properties/keys projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Json(serde_json::json!({
                "id": "props-ada",
                "name": "Ada",
                "team": "eng"
            })),
            Value::Json(serde_json::json!(["id", "name", "team"])),
            Value::Json(serde_json::json!({
                "id": "props-knows",
                "since": 2026
            })),
            Value::Json(serde_json::json!(["id", "since"])),
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'props-cara', status: 'props', team: 'ops'});
            CREATE (:Person {id: 'props-dan', status: 'props', team: 'eng'});
            MATCH (n:Person {status: 'props'}) SET n.seen = true
            RETURN n.id AS id, properties(n) AS props, keys(n) AS keys
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad properties/keys projections");
    assert_eq!(broad.table.rows.len(), 2);
    assert_eq!(broad.table.rows[0][0], Value::from("props-cara"));
    assert_eq!(
        broad.table.rows[0][2],
        Value::Json(serde_json::json!(["id", "seen", "status", "team"]))
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'props-team'});
            MATCH (n:Person {status: 'props'}), (t:Team {id: 'props-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'props'}]->(t)
            RETURN n.id AS id, properties(r) AS props, keys(r) AS keys
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship properties/keys projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![
                Value::from("props-cara"),
                Value::Json(serde_json::json!({"source": "props"})),
                Value::Json(serde_json::json!(["source"]))
            ],
            vec![
                Value::from("props-dan"),
                Value::Json(serde_json::json!({"source": "props"})),
                Value::Json(serde_json::json!(["source"]))
            ],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'props'}) SET n.props_counted = true
            RETURN count(properties(n)) AS prop_rows,
                   count(DISTINCT keys(n)) AS distinct_key_sets,
                   collect(keys(n)) AS key_sets;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("properties/keys aggregate projections");
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
    let Value::Json(key_sets) = &aggregates.table.rows[0][2] else {
        panic!("collect(keys(n)) should return JSON arrays");
    };
    assert_eq!(key_sets.as_array().expect("key sets").len(), 2);

    let properties_on_path =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'props'}), (t:Team {id: 'props-team'})
            CREATE p = (n)-[r:REJECTED_PROPS_PATH]->(t)
            RETURN properties(p);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("properties on path variables should stay rejected");
    assert!(
        matches!(
            properties_on_path,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{properties_on_path:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_relationship_endpoints_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'endpoint-ada', name: 'Ada'});
            CREATE (b:Person {id: 'endpoint-bob', name: 'Bob'});
            MATCH (a:Person {id: 'endpoint-ada'}), (b:Person {id: 'endpoint-bob'})
            CREATE (a)-[e:KNOWS {id: 'endpoint-knows'}]->(b)
            RETURN startNode(e) AS source, endNode(e) AS target;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete relationship endpoint projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from(serde_json::json!({
                "id": "endpoint-ada",
                "label": "Person",
                "props": {
                    "id": {"type": "string", "value": "endpoint-ada"},
                    "name": {"type": "string", "value": "Ada"}
                }
            })),
            Value::from(serde_json::json!({
                "id": "endpoint-bob",
                "label": "Person",
                "props": {
                    "id": {"type": "string", "value": "endpoint-bob"},
                    "name": {"type": "string", "value": "Bob"}
                }
            })),
        ]]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'endpoint-team'});
            MATCH (n:Person), (t:Team {id: 'endpoint-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'endpoint'}]->(t)
            RETURN n.id AS id, startNode(r) AS source, endNode(r) AS target
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship endpoint projections");
    assert_eq!(row_edges.table.rows.len(), 2);
    assert_eq!(row_edges.table.rows[0][0], Value::from("endpoint-ada"));
    let Value::Json(source) = &row_edges.table.rows[0][1] else {
        panic!("startNode(r) should return a JSON node");
    };
    assert_eq!(source["id"], serde_json::json!("endpoint-ada"));
    let Value::Json(target) = &row_edges.table.rows[0][2] else {
        panic!("endNode(r) should return a JSON node");
    };
    assert_eq!(target["id"], serde_json::json!("endpoint-team"));

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (:Person)-[r:MEMBER_OF {source: 'endpoint'}]->(:Team {id: 'endpoint-team'})
            SET r.endpoint_checked = true
            RETURN count(startNode(r)) AS sources,
                   count(DISTINCT endNode(r)) AS target_count,
                   collect(endNode(r)) AS targets;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("relationship endpoint aggregate projections");
    assert_eq!(aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
    let Value::Json(targets) = &aggregates.table.rows[0][2] else {
        panic!("collect(endNode(r)) should return JSON nodes");
    };
    assert_eq!(targets.as_array().expect("target nodes").len(), 2);

    let start_node_on_node =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person) SET n.endpoint_rejected = true
            RETURN startNode(n);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("startNode on node variables should stay rejected");
    assert!(
        matches!(
            start_node_on_node,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{start_node_on_node:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_identity_functions_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'identity-ada'});
            CREATE (b:Person {id: 'identity-bob'});
            MATCH (a:Person {id: 'identity-ada'}), (b:Person {id: 'identity-bob'})
            CREATE (a)-[e:KNOWS {id: 'identity-knows'}]->(b)
            RETURN id(a) AS node_id,
                   elementId(a) AS node_element_id,
                   id(e) AS edge_id,
                   elementId(e) AS edge_element_id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete identity function projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("identity-ada"),
            Value::from("identity-ada"),
            Value::from("identity-knows"),
            Value::from("identity-knows"),
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'identity-cara', status: 'identity'});
            CREATE (:Person {id: 'identity-dan', status: 'identity'});
            MATCH (n:Person {status: 'identity'}) SET n.seen = true
            RETURN n.id AS raw, id(n) AS id, elementId(n) AS element_id
            ORDER BY raw;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad node identity function projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("identity-cara"),
                Value::from("identity-cara"),
                Value::from("identity-cara")
            ],
            vec![
                Value::from("identity-dan"),
                Value::from("identity-dan"),
                Value::from("identity-dan")
            ],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'identity-team'});
            MATCH (n:Person {status: 'identity'}), (t:Team {id: 'identity-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'identity'}]->(t)
            RETURN n.id AS id, id(r) AS relationship_id
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship identity function projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("identity-cara"), Value::Null],
            vec![Value::from("identity-dan"), Value::Null],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'identity'}) SET n.identity_counted = true
            RETURN count(id(n)) AS ids,
                   count(DISTINCT elementId(n)) AS distinct_ids,
                   collect(id(n)) AS collected_ids;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("identity function aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["identity-cara", "identity-dan"])),
        ]]
    );

    let relationship_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (:Person {status: 'identity'})-[r:MEMBER_OF {source: 'identity'}]->(:Team {id: 'identity-team'})
            SET r.identity_checked = true
            RETURN count(id(r)) AS relationship_ids,
                   collect(elementId(r)) AS collected_relationship_ids;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("relationship identity function aggregate projections");
    assert_eq!(
        relationship_aggregates.table.rows,
        vec![vec![Value::Int(0), Value::Json(serde_json::json!([]))]]
    );

    let identity_on_path =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'identity'}), (t:Team {id: 'identity-team'})
            CREATE p = (n)-[r:REJECTED_ID_PATH]->(t)
            RETURN id(p);
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("identity functions on path variables should stay rejected");
    assert!(
        matches!(
            identity_on_path,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{identity_on_path:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_exists_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'exists-ada', name: 'Ada'});
            CREATE (b:Person {id: 'exists-bob'});
            MATCH (a:Person {id: 'exists-ada'}), (b:Person {id: 'exists-bob'})
            CREATE (a)-[e:KNOWS {id: 'exists-knows', since: 2026}]->(b)
            RETURN exists(a.name) AS has_name,
                   exists(a.nickname) AS has_nickname,
                   exists(e.since) AS has_since,
                   exists(e.weight) AS has_weight;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete exists projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(false),
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'exists-cara', status: 'exists', nickname: 'C'});
            CREATE (:Person {id: 'exists-dan', status: 'exists'});
            MATCH (n:Person {status: 'exists'}) SET n.seen = true
            RETURN n.id AS id, exists(n.nickname) AS has_nickname
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad exists projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("exists-cara"), Value::Bool(true)],
            vec![Value::from("exists-dan"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'exists-team'});
            MATCH (n:Person {status: 'exists'}), (t:Team {id: 'exists-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'exists'}]->(t)
            RETURN n.id AS id, exists(r.source) AS has_source, exists(r.id) AS has_id
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship exists projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![
                Value::from("exists-cara"),
                Value::Bool(true),
                Value::Bool(false)
            ],
            vec![
                Value::from("exists-dan"),
                Value::Bool(true),
                Value::Bool(false)
            ],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'exists'}) SET n.exists_counted = true
            RETURN count(exists(n.nickname)) AS rows,
                   count(DISTINCT exists(n.nickname)) AS distinct_states,
                   collect(exists(n.nickname)) AS states;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("exists aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let non_property =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'exists-rejected'}) RETURN exists(n);",
            CypherMutationOptions::default(),
        ))
        .expect_err("exists over whole elements should stay rejected");
    assert!(
        matches!(non_property, GrustError::CypherUnsupportedCardinality(_)),
        "{non_property:?}"
    );

    let traversal_exists =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'exists'}), (t:Team {id: 'exists-team'})
            CREATE (n)-[r:REJECTED_EXISTS_PATH]->(t)
            RETURN exists((n)-[:REJECTED_EXISTS_PATH]->(t));
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("exists traversal predicates should stay rejected");
    assert!(
        matches!(
            traversal_exists,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{traversal_exists:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_size_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'size-ada', name: 'Ada', tags: $tags})
            RETURN size(n.name) AS name_size,
                   size(n.tags) AS tag_count,
                   size(n.nickname) AS missing_size;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete size projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![Value::Int(3), Value::Int(2), Value::Null]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'size-bob', status: 'size', nickname: 'B'});
            CREATE (:Person {id: 'size-cara', status: 'size', nickname: 'Cara'});
            MATCH (n:Person {status: 'size'}) SET n.seen = true
            RETURN n.id AS id, size(n.nickname) AS nickname_size
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad size projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("size-bob"), Value::Int(1)],
            vec![Value::from("size-cara"), Value::Int(4)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'size-team'});
            MATCH (n:Person {status: 'size'}), (t:Team {id: 'size-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'size'}]->(t)
            RETURN n.id AS id, size(r.source) AS source_size
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship size projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("size-bob"), Value::Int(4)],
            vec![Value::from("size-cara"), Value::Int(4)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'size'}) SET n.size_counted = true
            RETURN count(size(n.nickname)) AS rows,
                   sum(size(n.nickname)) AS total_size,
                   collect(size(n.nickname)) AS sizes;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("size aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(5),
            Value::Json(serde_json::json!([1, 4])),
        ]]
    );

    let numeric_size =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'size-number', score: 3}) RETURN size(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("size over numeric values should stay rejected");
    assert!(
        matches!(numeric_size, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_size:?}"
    );

    let traversal_size =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'size'}), (t:Team {id: 'size-team'})
            CREATE (n)-[r:REJECTED_SIZE_PATH]->(t)
            RETURN size((n)-[:REJECTED_SIZE_PATH]->(t));
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("size traversal expressions should stay rejected");
    assert!(
        matches!(traversal_size, GrustError::CypherUnsupportedCardinality(_)),
        "{traversal_size:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_slices_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'slice-ada', tags: $tags, scores: $scores});
            CREATE (b:Person {id: 'slice-bob'});
            MATCH (a:Person {id: 'slice-ada'}), (b:Person {id: 'slice-bob'})
            CREATE (a)-[e:KNOWS {id: 'slice-knows', weights: $weights}]->(b)
            RETURN a.tags[0..2] AS first_tags,
                   a.scores[$start..$end] AS middle_scores,
                   e.weights[1..] AS trailing_weights,
                   a.tags[..1] AS leading_tag,
                   a.tags[9..12] AS empty_tags,
                   a.nickname[0..1] AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec![
                            "engineer".to_string(),
                            "speaker".to_string(),
                            "writer".to_string(),
                        ]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![5, 7, 11, 13])),
                    (
                        "weights".to_string(),
                        Value::FloatArray(vec![2.5, 4.5, 6.5]),
                    ),
                    ("start".to_string(), Value::Int(1)),
                    ("end".to_string(), Value::Int(3)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list slice projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
            Value::IntArray(vec![7, 11]),
            Value::FloatArray(vec![4.5, 6.5]),
            Value::StringArray(vec!["engineer".to_string()]),
            Value::StringArray(vec![]),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'slice-cara', status: 'slice', scores: $scores_a});
            CREATE (:Person {id: 'slice-dan', status: 'slice', scores: $scores_b});
            MATCH (n:Person {status: 'slice'}) SET n.sliced = true
            RETURN n.id AS id, n.scores[1..3] AS scores
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores_a".to_string(), Value::IntArray(vec![3, 5, 8])),
                    ("scores_b".to_string(), Value::IntArray(vec![7, 9, 13])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list slice projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("slice-cara"), Value::IntArray(vec![5, 8])],
            vec![Value::from("slice-dan"), Value::IntArray(vec![9, 13])],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'slice-team'});
            MATCH (n:Person {status: 'slice'}), (t:Team {id: 'slice-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, r.rankings[..2] AS ranks
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2, 3]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list slice projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("slice-cara"), Value::IntArray(vec![1, 2])],
            vec![Value::from("slice-dan"), Value::IntArray(vec![1, 2])],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'slice'}) SET n.slice_counted = true
            RETURN count(n.scores[1..3]) AS rows,
                   collect(n.scores[1..3]) AS score_slices;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list slice aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([[5, 8], [9, 13]])),
        ]]
    );

    let numeric_slice_aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'slice'}) SET n.slice_summed = true RETURN sum(n.scores[1..3]);",
            CypherMutationOptions::default(),
        ))
        .expect_err("numeric aggregates over list slices should stay rejected");
    assert!(
        matches!(
            numeric_slice_aggregate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_slice_aggregate:?}"
    );

    let non_array =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-string', name: 'Ada'}) RETURN n.name[0..1];",
            CypherMutationOptions::default(),
        ))
        .expect_err("list slices over strings should stay rejected");
    assert!(
        matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
        "{non_array:?}"
    );

    let negative_bound =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-negative', scores: $scores}) RETURN n.scores[-1..2];",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![1, 2]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("negative list slice bounds should stay rejected");
    assert!(
        matches!(negative_bound, GrustError::CypherUnsupportedCardinality(_)),
        "{negative_bound:?}"
    );

    let nested_bound =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-nested', scores: $scores}) RETURN n.scores[0..head(n.scores)];",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![1, 2]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("nested list slice bounds should stay rejected");
    assert!(
        matches!(nested_bound, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_bound:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_membership_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'membership-ada', tags: $tags, scores: $scores});
            CREATE (b:Person {id: 'membership-bob'});
            MATCH (a:Person {id: 'membership-ada'}), (b:Person {id: 'membership-bob'})
            CREATE (a)-[e:KNOWS {id: 'membership-knows', weights: $weights}]->(b)
            RETURN 'speaker' IN a.tags AS has_speaker,
                   $needle_score IN a.scores AS has_score,
                   4.5 IN e.weights AS has_weight,
                   'missing' IN a.tags AS missing_tag,
                   null IN a.tags AS null_needle,
                   'speaker' IN a.nickname AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ("needle_score".to_string(), Value::Int(11)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list membership projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
            Value::Null,
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'membership-cara', status: 'membership', tags: $tags_a});
            CREATE (:Person {id: 'membership-dan', status: 'membership', tags: $tags_b});
            MATCH (n:Person {status: 'membership'}) SET n.membership_checked = true
            RETURN n.id AS id, 'speaker' IN n.tags AS has_speaker
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags_a".to_string(),
                        Value::StringArray(vec!["speaker".to_string(), "mentor".to_string()]),
                    ),
                    (
                        "tags_b".to_string(),
                        Value::StringArray(vec!["writer".to_string(), "mentor".to_string()]),
                    ),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list membership projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("membership-cara"), Value::Bool(true)],
            vec![Value::from("membership-dan"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'membership-team'});
            MATCH (n:Person {status: 'membership'}), (t:Team {id: 'membership-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, 2 IN r.rankings AS has_rank
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2, 3]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list membership projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("membership-cara"), Value::Bool(true)],
            vec![Value::from("membership-dan"), Value::Bool(true)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'membership'}) SET n.membership_counted = true
            RETURN count('speaker' IN n.tags) AS rows,
                   count(DISTINCT 'speaker' IN n.tags) AS states,
                   collect('speaker' IN n.tags) AS memberships;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list membership aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let numeric_membership_aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'membership'}) SET n.membership_summed = true RETURN sum('speaker' IN n.tags);",
            CypherMutationOptions::default(),
        ))
        .expect_err("numeric aggregates over membership booleans should stay rejected");
    assert!(
        matches!(
            numeric_membership_aggregate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_membership_aggregate:?}"
    );

    let type_mismatch =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'membership-type', scores: $scores}) RETURN '11' IN n.scores;",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![11]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("type mismatched list membership should evaluate false");
    assert_eq!(type_mismatch.table.rows, vec![vec![Value::Bool(false)]]);

    let non_array =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'membership-string', name: 'Ada'}) RETURN 'A' IN n.name;",
            CypherMutationOptions::default(),
        ))
        .expect_err("list membership over strings should stay rejected");
    assert!(
        matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
        "{non_array:?}"
    );

    let computed_needle =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'membership-computed', tags: $tags}) RETURN toLower('SPEAKER') IN n.tags;",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["speaker".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("computed list membership needles should stay rejected");
    assert!(
        matches!(computed_needle, GrustError::CypherUnsupportedCardinality(_)),
        "{computed_needle:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_predicates_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'predicate-list-ada', tags: $tags, scores: $scores});
            CREATE (b:Person {id: 'predicate-list-bob'});
            MATCH (a:Person {id: 'predicate-list-ada'}), (b:Person {id: 'predicate-list-bob'})
            CREATE (a)-[e:KNOWS {id: 'predicate-list-knows', weights: $weights}]->(b)
            RETURN any(t IN a.tags WHERE t = 'speaker') AS any_speaker,
                   all(t IN a.tags WHERE t = 'speaker') AS all_speaker,
                   none(t IN a.tags WHERE t = 'missing') AS none_missing,
                   single(s IN a.scores WHERE s = $needle_score) AS single_score,
                   any(w IN e.weights WHERE w = 4.5) AS any_weight,
                   any(t IN a.nickname WHERE t = 'speaker') AS missing_name,
                   any(t IN a.tags WHERE t = null) AS null_needle;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec![
                            "engineer".to_string(),
                            "speaker".to_string(),
                            "speaker".to_string(),
                        ]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ("needle_score".to_string(), Value::Int(11)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list predicate projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Null,
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'predicate-list-cara', status: 'list-predicate', tags: $tags_a});
            CREATE (:Person {id: 'predicate-list-dan', status: 'list-predicate', tags: $tags_b});
            MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_checked = true
            RETURN n.id AS id, any(t IN n.tags WHERE t = 'speaker') AS any_speaker
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags_a".to_string(),
                        Value::StringArray(vec!["speaker".to_string(), "mentor".to_string()]),
                    ),
                    (
                        "tags_b".to_string(),
                        Value::StringArray(vec!["writer".to_string(), "mentor".to_string()]),
                    ),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list predicate projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("predicate-list-cara"), Value::Bool(true)],
            vec![Value::from("predicate-list-dan"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'predicate-list-team'});
            MATCH (n:Person {status: 'list-predicate'}), (t:Team {id: 'predicate-list-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, single(rank IN r.rankings WHERE rank = 2) AS single_rank
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2, 3]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list predicate projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("predicate-list-cara"), Value::Bool(true)],
            vec![Value::from("predicate-list-dan"), Value::Bool(true)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_counted = true
            RETURN count(any(t IN n.tags WHERE t = 'speaker')) AS rows,
                   count(DISTINCT any(t IN n.tags WHERE t = 'speaker')) AS states,
                   collect(any(t IN n.tags WHERE t = 'speaker')) AS predicates;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list predicate aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let empty =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'predicate-list-empty', tags: $tags})
            RETURN any(t IN n.tags WHERE t = 'speaker') AS any_speaker,
                   all(t IN n.tags WHERE t = 'speaker') AS all_speaker,
                   none(t IN n.tags WHERE t = 'speaker') AS none_speaker,
                   single(t IN n.tags WHERE t = 'speaker') AS single_speaker;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(Vec::new()),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("empty list predicate projections");
    assert_eq!(
        empty.table.rows,
        vec![vec![
            Value::Bool(false),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
        ]]
    );

    let numeric_predicate_aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_summed = true RETURN sum(any(t IN n.tags WHERE t = 'speaker'));",
            CypherMutationOptions::default(),
        ))
        .expect_err("numeric aggregates over list predicate booleans should stay rejected");
    assert!(
        matches!(
            numeric_predicate_aggregate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_predicate_aggregate:?}"
    );

    let type_mismatch =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-list-type', scores: $scores}) RETURN any(s IN n.scores WHERE s = '11');",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![11]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("type mismatched list predicates should evaluate false");
    assert_eq!(type_mismatch.table.rows, vec![vec![Value::Bool(false)]]);

    let non_array =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-list-string', name: 'Ada'}) RETURN any(ch IN n.name WHERE ch = 'A');",
            CypherMutationOptions::default(),
        ))
        .expect_err("list predicates over strings should stay rejected");
    assert!(
        matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
        "{non_array:?}"
    );

    let wrong_item =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-list-wrong-item', tags: $tags}) RETURN any(t IN n.tags WHERE other = 'speaker');",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["speaker".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("list predicates should require the same WHERE item variable");
    assert!(
        matches!(wrong_item, GrustError::CypherUnsupportedCardinality(_)),
        "{wrong_item:?}"
    );

    let computed_predicate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-list-computed', tags: $tags}) RETURN any(t IN n.tags WHERE toLower(t) = 'speaker');",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["speaker".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("computed list predicate expressions should stay rejected");
    assert!(
        matches!(
            computed_predicate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{computed_predicate:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_indexes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'index-ada', tags: $tags, scores: $scores});
            CREATE (b:Person {id: 'index-bob'});
            MATCH (a:Person {id: 'index-ada'}), (b:Person {id: 'index-bob'})
            CREATE (a)-[e:KNOWS {id: 'index-knows', weights: $weights}]->(b)
            RETURN a.tags[0] AS first_tag,
                   a.scores[$score_index] AS second_score,
                   e.weights[1] AS second_weight,
                   a.tags[9] AS missing_tag,
                   a.nickname[0] AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                    ("score_index".to_string(), Value::Int(1)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list index projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("engineer"),
            Value::Int(11),
            Value::Float(4.5),
            Value::Null,
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'index-cara', status: 'index', scores: $scores_a});
            CREATE (:Person {id: 'index-dan', status: 'index', scores: $scores_b});
            MATCH (n:Person {status: 'index'}) SET n.indexed = true
            RETURN n.id AS id, n.scores[0] AS score
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                    ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list index projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("index-cara"), Value::Int(3)],
            vec![Value::from("index-dan"), Value::Int(7)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'index-team'});
            MATCH (n:Person {status: 'index'}), (t:Team {id: 'index-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, r.rankings[1] AS rank
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list index projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("index-cara"), Value::Int(2)],
            vec![Value::from("index-dan"), Value::Int(2)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'index'}) SET n.index_counted = true
            RETURN count(n.scores[0]) AS rows,
                   sum(n.scores[0]) AS total_scores,
                   collect(n.scores[0]) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list index aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(10),
            Value::Json(serde_json::json!([3, 7])),
        ]]
    );

    let non_array =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'index-string', name: 'Ada'}) RETURN n.name[0];",
            CypherMutationOptions::default(),
        ))
        .expect_err("list indexes over strings should stay rejected");
    assert!(
        matches!(non_array, GrustError::CypherUnsupportedCardinality(_)),
        "{non_array:?}"
    );

    let negative_index =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'index-negative', scores: $scores}) RETURN n.scores[-1];",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![1]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("negative list indexes should stay rejected");
    assert!(
        matches!(negative_index, GrustError::CypherUnsupportedCardinality(_)),
        "{negative_index:?}"
    );

    let nested_index =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'index-nested', scores: $scores}) RETURN n.scores[head(n.scores)];",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::IntArray(vec![1]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("nested list index expressions should stay rejected");
    assert!(
        matches!(nested_index, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_index:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_elements_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'list-ada', tags: $tags, scores: $scores, empty: $empty});
            CREATE (b:Person {id: 'list-bob'});
            MATCH (a:Person {id: 'list-ada'}), (b:Person {id: 'list-bob'})
            CREATE (a)-[e:KNOWS {id: 'list-knows', weights: $weights}]->(b)
            RETURN head(a.tags) AS first_tag,
                   last(a.scores) AS last_score,
                   head(a.empty) AS empty_head,
                   last(e.weights) AS last_weight,
                   head(a.nickname) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("empty".to_string(), Value::StringArray(Vec::new())),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list element projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("engineer"),
            Value::Int(11),
            Value::Null,
            Value::Float(4.5),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'list-cara', status: 'list', scores: $scores_a});
            CREATE (:Person {id: 'list-dan', status: 'list', scores: $scores_b});
            MATCH (n:Person {status: 'list'}) SET n.seen = true
            RETURN n.id AS id, head(n.scores) AS score
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                    ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list element projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("list-cara"), Value::Int(3)],
            vec![Value::from("list-dan"), Value::Int(7)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'list-team'});
            MATCH (n:Person {status: 'list'}), (t:Team {id: 'list-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, last(r.rankings) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list element projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("list-cara"), Value::Int(2)],
            vec![Value::from("list-dan"), Value::Int(2)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'list'}) SET n.list_counted = true
            RETURN count(head(n.scores)) AS rows,
                   sum(head(n.scores)) AS total_scores,
                   collect(head(n.scores)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list element aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(10),
            Value::Json(serde_json::json!([3, 7])),
        ]]
    );

    let string_head =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'list-string', name: 'Ada'}) RETURN head(n.name);",
            CypherMutationOptions::default(),
        ))
        .expect_err("head over string values should stay rejected");
    assert!(
        matches!(string_head, GrustError::CypherUnsupportedCardinality(_)),
        "{string_head:?}"
    );

    let nested_head =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'list-nested', path: 'a/b'}) RETURN head(split(n.path, '/'));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested head arguments should stay rejected");
    assert!(
        matches!(nested_head, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_head:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_tail_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'tail-ada', tags: $tags, scores: $scores, empty: $empty});
            CREATE (b:Person {id: 'tail-bob'});
            MATCH (a:Person {id: 'tail-ada'}), (b:Person {id: 'tail-bob'})
            CREATE (a)-[e:KNOWS {id: 'tail-knows', weights: $weights}]->(b)
            RETURN tail(a.tags) AS tag_tail,
                   tail(a.scores) AS score_tail,
                   tail(a.empty) AS empty_tail,
                   tail(e.weights) AS weight_tail,
                   tail(a.nickname) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("empty".to_string(), Value::StringArray(Vec::new())),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list tail projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::StringArray(vec!["speaker".to_string()]),
            Value::IntArray(vec![11]),
            Value::StringArray(Vec::new()),
            Value::FloatArray(vec![4.5]),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'tail-cara', status: 'tail', scores: $scores_a});
            CREATE (:Person {id: 'tail-dan', status: 'tail', scores: $scores_b});
            MATCH (n:Person {status: 'tail'}) SET n.seen = true
            RETURN n.id AS id, tail(n.scores) AS scores
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                    ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list tail projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("tail-cara"), Value::IntArray(vec![5])],
            vec![Value::from("tail-dan"), Value::IntArray(vec![9])],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'tail-team'});
            MATCH (n:Person {status: 'tail'}), (t:Team {id: 'tail-team'})
            CREATE (n)-[r:MEMBER_OF {rankings: $rankings}]->(t)
            RETURN n.id AS id, tail(r.rankings) AS ranks
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "rankings".to_string(),
                    Value::IntArray(vec![1, 2]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing relationship list tail projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("tail-cara"), Value::IntArray(vec![2])],
            vec![Value::from("tail-dan"), Value::IntArray(vec![2])],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'tail'}) SET n.tail_counted = true
            RETURN count(tail(n.scores)) AS rows,
                   collect(tail(n.scores)) AS score_tails;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list tail aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([[5], [9]])),
        ]]
    );

    let string_tail =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'tail-string', name: 'Ada'}) RETURN tail(n.name);",
            CypherMutationOptions::default(),
        ))
        .expect_err("tail over string values should stay rejected");
    assert!(
        matches!(string_tail, GrustError::CypherUnsupportedCardinality(_)),
        "{string_tail:?}"
    );

    let nested_tail =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'tail-nested', path: 'a/b'}) RETURN tail(split(n.path, '/'));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested tail arguments should stay rejected");
    assert!(
        matches!(nested_tail, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_tail:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_is_empty_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'empty-ada', name: '', tags: $tags, codes: $codes})
            RETURN isEmpty(n.name) AS empty_name,
                   isEmpty(n.tags) AS empty_tags,
                   isEmpty(n.codes) AS empty_codes,
                   isEmpty(n.nickname) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("tags".to_string(), Value::StringArray(Vec::new())),
                    (
                        "codes".to_string(),
                        Value::StringArray(vec!["A".to_string()]),
                    ),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete isEmpty projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'empty-bob', status: 'empty', nickname: ''});
            CREATE (:Person {id: 'empty-cara', status: 'empty', nickname: 'Cara'});
            MATCH (n:Person {status: 'empty'}) SET n.seen = true
            RETURN n.id AS id, isEmpty(n.nickname) AS empty_nickname
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad isEmpty projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("empty-bob"), Value::Bool(true)],
            vec![Value::from("empty-cara"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'empty-team'});
            MATCH (n:Person {status: 'empty'}), (t:Team {id: 'empty-team'})
            CREATE (n)-[r:MEMBER_OF {source: ''}]->(t)
            RETURN n.id AS id, isEmpty(r.source) AS empty_source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship isEmpty projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("empty-bob"), Value::Bool(true)],
            vec![Value::from("empty-cara"), Value::Bool(true)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'empty'}) SET n.empty_counted = true
            RETURN count(isEmpty(n.nickname)) AS rows,
                   count(DISTINCT isEmpty(n.nickname)) AS distinct_states,
                   collect(isEmpty(n.nickname)) AS states;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("isEmpty aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let numeric_empty =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'empty-number', score: 3}) RETURN isEmpty(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("isEmpty over numeric values should stay rejected");
    assert!(
        matches!(numeric_empty, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_empty:?}"
    );

    let nested_empty =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'empty-nested', name: ''}) RETURN isEmpty(toLower(n.name));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested isEmpty arguments should stay rejected");
    assert!(
        matches!(nested_empty, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_empty:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_to_string_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'to-string-ada', name: 'Ada', score: 3, active: true});
            CREATE (b:Person {id: 'to-string-bob'});
            MATCH (a:Person {id: 'to-string-ada'}), (b:Person {id: 'to-string-bob'})
            CREATE (a)-[e:KNOWS {id: 'to-string-knows', weight: 2.5}]->(b)
            RETURN toString(a.name) AS name,
                   toString(a.score) AS score,
                   toString(a.active) AS active,
                   toString(e.weight) AS weight,
                   toString(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete toString projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("Ada"),
            Value::from("3"),
            Value::from("true"),
            Value::from("2.5"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'to-string-cara', status: 'to-string', score: 7});
            CREATE (:Person {id: 'to-string-dan', status: 'to-string', score: 11});
            MATCH (n:Person {status: 'to-string'}) SET n.seen = true
            RETURN n.id AS id, toString(n.score) AS score
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad toString projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("to-string-cara"), Value::from("7")],
            vec![Value::from("to-string-dan"), Value::from("11")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'to-string-team'});
            MATCH (n:Person {status: 'to-string'}), (t:Team {id: 'to-string-team'})
            CREATE (n)-[r:MEMBER_OF {rank: 5}]->(t)
            RETURN n.id AS id, toString(r.rank) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship toString projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("to-string-cara"), Value::from("5")],
            vec![Value::from("to-string-dan"), Value::from("5")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'to-string'}) SET n.to_string_counted = true
            RETURN count(toString(n.score)) AS rows,
                   count(DISTINCT toString(n.score)) AS distinct_scores,
                   collect(toString(n.score)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("toString aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["7", "11"])),
        ]]
    );

    let array_to_string =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'to-string-array', tags: $tags})
            RETURN toString(n.tags);
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["a".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("toString over arrays should stay rejected");
    assert!(
        matches!(array_to_string, GrustError::CypherUnsupportedCardinality(_)),
        "{array_to_string:?}"
    );

    let nested_to_string =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'to-string-nested', name: 'Ada'}) RETURN toString(toLower(n.name));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested toString arguments should stay rejected");
    assert!(
        matches!(
            nested_to_string,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{nested_to_string:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_abs_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'abs-ada', debt: -3, ratio: -2.5});
            CREATE (b:Person {id: 'abs-bob'});
            MATCH (a:Person {id: 'abs-ada'}), (b:Person {id: 'abs-bob'})
            CREATE (a)-[e:KNOWS {id: 'abs-knows', weight: -4}]->(b)
            RETURN abs(a.debt) AS debt,
                   abs(a.ratio) AS ratio,
                   abs(e.weight) AS weight,
                   abs(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete abs projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Int(3),
            Value::Float(2.5),
            Value::Int(4),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'abs-cara', status: 'abs', score: -7});
            CREATE (:Person {id: 'abs-dan', status: 'abs', score: -11});
            MATCH (n:Person {status: 'abs'}) SET n.seen = true
            RETURN n.id AS id, abs(n.score) AS score
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad abs projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("abs-cara"), Value::Int(7)],
            vec![Value::from("abs-dan"), Value::Int(11)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'abs-team'});
            MATCH (n:Person {status: 'abs'}), (t:Team {id: 'abs-team'})
            CREATE (n)-[r:MEMBER_OF {rank: -5}]->(t)
            RETURN n.id AS id, abs(r.rank) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship abs projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("abs-cara"), Value::Int(5)],
            vec![Value::from("abs-dan"), Value::Int(5)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'abs'}) SET n.abs_counted = true
            RETURN count(abs(n.score)) AS rows,
                   sum(abs(n.score)) AS total_scores,
                   collect(abs(n.score)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("abs aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(18),
            Value::Json(serde_json::json!([7, 11])),
        ]]
    );

    let string_abs =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'abs-string', score: '3'}) RETURN abs(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("abs over string values should stay rejected");
    assert!(
        matches!(string_abs, GrustError::CypherUnsupportedCardinality(_)),
        "{string_abs:?}"
    );

    let nested_abs =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'abs-nested', score: -3}) RETURN abs(size(n.score));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested abs arguments should stay rejected");
    assert!(
        matches!(nested_abs, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_abs:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_numeric_rounds_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'round-ada', debt: -3.2, ratio: 2.1});
            CREATE (b:Person {id: 'round-bob'});
            MATCH (a:Person {id: 'round-ada'}), (b:Person {id: 'round-bob'})
            CREATE (a)-[e:KNOWS {id: 'round-knows', weight: -4.8}]->(b)
            RETURN ceil(a.debt) AS debt_ceiling,
                   floor(a.ratio) AS ratio_floor,
                   floor(e.weight) AS weight_floor,
                   ceil(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete numeric rounding projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Float(-3.0),
            Value::Float(2.0),
            Value::Float(-5.0),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'round-cara', status: 'round', score: 7.2});
            CREATE (:Person {id: 'round-dan', status: 'round', score: 11.8});
            MATCH (n:Person {status: 'round'}) SET n.seen = true
            RETURN n.id AS id, ceil(n.score) AS score
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad numeric rounding projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("round-cara"), Value::Float(8.0)],
            vec![Value::from("round-dan"), Value::Float(12.0)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'round-team'});
            MATCH (n:Person {status: 'round'}), (t:Team {id: 'round-team'})
            CREATE (n)-[r:MEMBER_OF {rank: -5.3}]->(t)
            RETURN n.id AS id, floor(r.rank) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship numeric rounding projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("round-cara"), Value::Float(-6.0)],
            vec![Value::from("round-dan"), Value::Float(-6.0)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'round'}) SET n.round_counted = true
            RETURN count(ceil(n.score)) AS rows,
                   sum(ceil(n.score)) AS total_scores,
                   collect(ceil(n.score)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("numeric rounding aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Float(20.0),
            Value::Json(serde_json::json!([8.0, 12.0])),
        ]]
    );

    let string_round =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'round-string', score: '3'}) RETURN ceil(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("ceil over string values should stay rejected");
    assert!(
        matches!(string_round, GrustError::CypherUnsupportedCardinality(_)),
        "{string_round:?}"
    );

    let nested_round =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'round-nested', score: -3.2}) RETURN ceil(abs(n.score));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested ceil arguments should stay rejected");
    assert!(
        matches!(nested_round, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_round:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_numeric_sign_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'sign-ada', debt: -3, ratio: 2.1, zero: 0});
            CREATE (b:Person {id: 'sign-bob'});
            MATCH (a:Person {id: 'sign-ada'}), (b:Person {id: 'sign-bob'})
            CREATE (a)-[e:KNOWS {id: 'sign-knows', weight: -4.8}]->(b)
            RETURN sign(a.debt) AS debt_sign,
                   sign(a.ratio) AS ratio_sign,
                   sign(a.zero) AS zero_sign,
                   sign(e.weight) AS weight_sign,
                   sign(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete numeric sign projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Int(-1),
            Value::Float(1.0),
            Value::Int(0),
            Value::Float(-1.0),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'sign-cara', status: 'sign', score: -7});
            CREATE (:Person {id: 'sign-dan', status: 'sign', score: 11});
            MATCH (n:Person {status: 'sign'}) SET n.seen = true
            RETURN n.id AS id, sign(n.score) AS score
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad numeric sign projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("sign-cara"), Value::Int(-1)],
            vec![Value::from("sign-dan"), Value::Int(1)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'sign-team'});
            MATCH (n:Person {status: 'sign'}), (t:Team {id: 'sign-team'})
            CREATE (n)-[r:MEMBER_OF {rank: -5.3}]->(t)
            RETURN n.id AS id, sign(r.rank) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship numeric sign projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("sign-cara"), Value::Float(-1.0)],
            vec![Value::from("sign-dan"), Value::Float(-1.0)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'sign'}) SET n.sign_counted = true
            RETURN count(sign(n.score)) AS rows,
                   sum(sign(n.score)) AS total_scores,
                   collect(sign(n.score)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("numeric sign aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(0),
            Value::Json(serde_json::json!([-1, 1])),
        ]]
    );

    let string_sign =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'sign-string', score: '3'}) RETURN sign(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("sign over string values should stay rejected");
    assert!(
        matches!(string_sign, GrustError::CypherUnsupportedCardinality(_)),
        "{string_sign:?}"
    );

    let nested_sign =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'sign-nested', score: -3}) RETURN sign(abs(n.score));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested sign arguments should stay rejected");
    assert!(
        matches!(nested_sign, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_sign:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_numeric_casts_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'cast-ada', score: 7, ratio: 2.9, text_score: '42'});
            CREATE (b:Person {id: 'cast-bob'});
            MATCH (a:Person {id: 'cast-ada'}), (b:Person {id: 'cast-bob'})
            CREATE (a)-[e:KNOWS {id: 'cast-knows', weight: '4.5'}]->(b)
            RETURN toFloat(a.score) AS score_float,
                   toInteger(a.ratio) AS ratio_int,
                   toInteger(a.text_score) AS text_score_int,
                   toFloat(e.weight) AS weight_float,
                   toInteger(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete numeric cast projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Float(7.0),
            Value::Int(2),
            Value::Int(42),
            Value::Float(4.5),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'cast-cara', status: 'cast', score: 7.2});
            CREATE (:Person {id: 'cast-dan', status: 'cast', score: 11.8});
            MATCH (n:Person {status: 'cast'}) SET n.seen = true
            RETURN n.id AS id, toInteger(n.score) AS score
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad numeric cast projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("cast-cara"), Value::Int(7)],
            vec![Value::from("cast-dan"), Value::Int(11)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'cast-team'});
            MATCH (n:Person {status: 'cast'}), (t:Team {id: 'cast-team'})
            CREATE (n)-[r:MEMBER_OF {rank: 5}]->(t)
            RETURN n.id AS id, toFloat(r.rank) AS rank
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship numeric cast projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("cast-cara"), Value::Float(5.0)],
            vec![Value::from("cast-dan"), Value::Float(5.0)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'cast'}) SET n.cast_counted = true
            RETURN count(toInteger(n.score)) AS rows,
                   sum(toInteger(n.score)) AS total_scores,
                   collect(toInteger(n.score)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("numeric cast aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(18),
            Value::Json(serde_json::json!([7, 11])),
        ]]
    );

    let boolean_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'cast-bool', score: true}) RETURN toInteger(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("toInteger over boolean values should stay rejected");
    assert!(
        matches!(boolean_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{boolean_cast:?}"
    );

    let non_integer_string =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'cast-string', score: '3.5'}) RETURN toInteger(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("toInteger over non-integer strings should stay rejected");
    assert!(
        matches!(
            non_integer_string,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{non_integer_string:?}"
    );

    let nested_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'cast-nested', score: 3}) RETURN toFloat(abs(n.score));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested toFloat arguments should stay rejected");
    assert!(
        matches!(nested_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_cast:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_boolean_cast_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'bool-ada', active: true, enabled: 'FALSE'});
            CREATE (b:Person {id: 'bool-bob'});
            MATCH (a:Person {id: 'bool-ada'}), (b:Person {id: 'bool-bob'})
            CREATE (a)-[e:KNOWS {id: 'bool-knows', trusted: 'true'}]->(b)
            RETURN toBoolean(a.active) AS active,
                   toBoolean(a.enabled) AS enabled,
                   toBoolean(e.trusted) AS trusted,
                   toBoolean(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete boolean cast projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'bool-cara', status: 'bool', active: 'true'});
            CREATE (:Person {id: 'bool-dan', status: 'bool', active: 'false'});
            MATCH (n:Person {status: 'bool'}) SET n.seen = true
            RETURN n.id AS id, toBoolean(n.active) AS active
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad boolean cast projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("bool-cara"), Value::Bool(true)],
            vec![Value::from("bool-dan"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'bool-team'});
            MATCH (n:Person {status: 'bool'}), (t:Team {id: 'bool-team'})
            CREATE (n)-[r:MEMBER_OF {trusted: false}]->(t)
            RETURN n.id AS id, toBoolean(r.trusted) AS trusted
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship boolean cast projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("bool-cara"), Value::Bool(false)],
            vec![Value::from("bool-dan"), Value::Bool(false)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'bool'}) SET n.bool_counted = true
            RETURN count(toBoolean(n.active)) AS rows,
                   count(DISTINCT toBoolean(n.active)) AS distinct_states,
                   collect(toBoolean(n.active)) AS states;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("boolean cast aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let numeric_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'bool-number', active: 1}) RETURN toBoolean(n.active);",
            CypherMutationOptions::default(),
        ))
        .expect_err("toBoolean over numeric values should stay rejected");
    assert!(
        matches!(numeric_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_cast:?}"
    );

    let invalid_string =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'bool-string', active: 'yes'}) RETURN toBoolean(n.active);",
            CypherMutationOptions::default(),
        ))
        .expect_err("toBoolean over non-boolean strings should stay rejected");
    assert!(
        matches!(invalid_string, GrustError::CypherUnsupportedCardinality(_)),
        "{invalid_string:?}"
    );

    let nested_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'bool-nested', active: true}) RETURN toBoolean(toString(n.active));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested toBoolean arguments should stay rejected");
    assert!(
        matches!(nested_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_cast:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_list_casts_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {
                id: 'list-cast-ada',
                scores: $scores,
                text_scores: $text_scores,
                ratios: $ratios,
                flags: $flags,
                json_numbers: $json_numbers
            });
            CREATE (b:Person {id: 'list-cast-bob'});
            MATCH (a:Person {id: 'list-cast-ada'}), (b:Person {id: 'list-cast-bob'})
            CREATE (a)-[e:KNOWS {id: 'list-cast-knows', ranks: $ranks}]->(b)
            RETURN toStringList(a.scores) AS score_strings,
                   toIntegerList(a.text_scores) AS score_ints,
                   toFloatList(a.ratios) AS ratio_floats,
                   toBooleanList(a.flags) AS flag_bools,
                   toIntegerList(a.json_numbers) AS json_ints,
                   toIntegerList(e.ranks) AS edge_ranks,
                   toStringList(a.nickname) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    (
                        "text_scores".to_string(),
                        Value::StringArray(vec!["3".to_string(), "5".to_string()]),
                    ),
                    ("ratios".to_string(), Value::FloatArray(vec![2.5, 4.0])),
                    (
                        "flags".to_string(),
                        Value::StringArray(vec!["true".to_string(), "FALSE".to_string()]),
                    ),
                    (
                        "json_numbers".to_string(),
                        Value::Json(serde_json::json!(["8", 13])),
                    ),
                    (
                        "ranks".to_string(),
                        Value::StringArray(vec!["1".to_string(), "2".to_string()]),
                    ),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete list cast projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::StringArray(vec!["7".to_string(), "11".to_string()]),
            Value::IntArray(vec![3, 5]),
            Value::FloatArray(vec![2.5, 4.0]),
            Value::Json(serde_json::json!([true, false])),
            Value::IntArray(vec![8, 13]),
            Value::IntArray(vec![1, 2]),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'list-cast-cara', status: 'list-cast', scores: $scores_a});
            CREATE (:Person {id: 'list-cast-dan', status: 'list-cast', scores: $scores_b});
            MATCH (n:Person {status: 'list-cast'}) SET n.cast_seen = true
            RETURN n.id AS id, toFloatList(n.scores) AS scores
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                    ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("broad list cast projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("list-cast-cara"),
                Value::FloatArray(vec![3.0, 5.0]),
            ],
            vec![
                Value::from("list-cast-dan"),
                Value::FloatArray(vec![7.0, 9.0]),
            ],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'list-cast'}) SET n.cast_counted = true
            RETURN count(toStringList(n.scores)) AS rows,
                   collect(toStringList(n.scores)) AS scores;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("list cast aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([["3", "5"], ["7", "9"]])),
        ]]
    );

    let scalar_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'list-cast-scalar', score: 3}) RETURN toStringList(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("list casts over scalar values should stay rejected");
    assert!(
        matches!(scalar_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{scalar_cast:?}"
    );

    let invalid_element =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'list-cast-invalid', scores: $scores}) RETURN toIntegerList(n.scores);",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "scores".to_string(),
                    Value::StringArray(vec!["3.5".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("invalid list cast elements should stay rejected");
    assert!(
        matches!(invalid_element, GrustError::CypherUnsupportedCardinality(_)),
        "{invalid_element:?}"
    );

    let nested_cast =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'list-cast-nested', tags: $tags}) RETURN toStringList(tail(n.tags));",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(vec!["a".to_string(), "b".to_string()]),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("nested list cast arguments should stay rejected");
    assert!(
        matches!(nested_cast, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_cast:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_transforms_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'string-ada', name: 'Ada Lovelace'});
            CREATE (b:Person {id: 'string-bob'});
            MATCH (a:Person {id: 'string-ada'}), (b:Person {id: 'string-bob'})
            CREATE (a)-[e:KNOWS {id: 'string-knows', source: 'Conference'}]->(b)
            RETURN toLower(a.name) AS lower_name,
                   toUpper(a.name) AS upper_name,
                   toLower(e.source) AS lower_source,
                   toUpper(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete string transform projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("ada lovelace"),
            Value::from("ADA LOVELACE"),
            Value::from("conference"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'string-cara', status: 'string', team: 'Eng'});
            CREATE (:Person {id: 'string-dan', status: 'string', team: 'Ops'});
            MATCH (n:Person {status: 'string'}) SET n.seen = true
            RETURN n.id AS id, toLower(n.team) AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string transform projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("string-cara"), Value::from("eng")],
            vec![Value::from("string-dan"), Value::from("ops")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'string-team'});
            MATCH (n:Person {status: 'string'}), (t:Team {id: 'string-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'StringSlice'}]->(t)
            RETURN n.id AS id, toUpper(r.source) AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string transform projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("string-cara"), Value::from("STRINGSLICE")],
            vec![Value::from("string-dan"), Value::from("STRINGSLICE")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'string'}) SET n.string_counted = true
            RETURN count(toLower(n.team)) AS rows,
                   count(DISTINCT toLower(n.team)) AS distinct_teams,
                   collect(toUpper(n.team)) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string transform aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["ENG", "OPS"])),
        ]]
    );

    let numeric_transform =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'string-number', score: 3}) RETURN toLower(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("string transforms over numeric values should stay rejected");
    assert!(
        matches!(
            numeric_transform,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_transform:?}"
    );

    let nested_transform =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'string-nested', name: 'Ada'}) RETURN toLower(coalesce(n.name, 'unknown'));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string transforms should stay rejected");
    assert!(
        matches!(
            nested_transform,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{nested_transform:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_trims_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'trim-ada', name: '  Ada  '});
            CREATE (b:Person {id: 'trim-bob'});
            MATCH (a:Person {id: 'trim-ada'}), (b:Person {id: 'trim-bob'})
            CREATE (a)-[e:KNOWS {id: 'trim-knows', source: '  Conference  '}]->(b)
            RETURN trim(a.name) AS trimmed_name,
                   lTrim(a.name) AS left_trimmed_name,
                   rTrim(e.source) AS right_trimmed_source,
                   trim(a.nickname) AS missing_name;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete string trim projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("Ada"),
            Value::from("Ada  "),
            Value::from("  Conference"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'trim-cara', status: 'trim', team: ' Eng '});
            CREATE (:Person {id: 'trim-dan', status: 'trim', team: ' Ops '});
            MATCH (n:Person {status: 'trim'}) SET n.seen = true
            RETURN n.id AS id, trim(n.team) AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string trim projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("trim-cara"), Value::from("Eng")],
            vec![Value::from("trim-dan"), Value::from("Ops")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'trim-team'});
            MATCH (n:Person {status: 'trim'}), (t:Team {id: 'trim-team'})
            CREATE (n)-[r:MEMBER_OF {source: ' TrimSlice '}]->(t)
            RETURN n.id AS id, lTrim(r.source) AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string trim projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("trim-cara"), Value::from("TrimSlice ")],
            vec![Value::from("trim-dan"), Value::from("TrimSlice ")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'trim'}) SET n.trim_counted = true
            RETURN count(trim(n.team)) AS rows,
                   count(DISTINCT trim(n.team)) AS distinct_teams,
                   collect(trim(n.team)) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string trim aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["Eng", "Ops"])),
        ]]
    );

    let numeric_trim =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'trim-number', score: 3}) RETURN trim(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("string trims over numeric values should stay rejected");
    assert!(
        matches!(numeric_trim, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_trim:?}"
    );

    let nested_trim =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'trim-nested', name: ' Ada '}) RETURN trim(toLower(n.name));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string trims should stay rejected");
    assert!(
        matches!(nested_trim, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_trim:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_substring_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'substring-ada', name: 'Ada Lovelace'});
            CREATE (b:Person {id: 'substring-bob'});
            MATCH (a:Person {id: 'substring-ada'}), (b:Person {id: 'substring-bob'})
            CREATE (a)-[e:KNOWS {id: 'substring-knows', source: 'Conference'}]->(b)
            RETURN substring(a.name, 0, 3) AS first_name,
                   substring(a.name, 4) AS last_name,
                   substring(e.source, $start, $length) AS source_part,
                   substring(a.nickname, 0, 2) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("start".to_string(), Value::Int(3)),
                    ("length".to_string(), Value::Int(4)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete substring projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("Ada"),
            Value::from("Lovelace"),
            Value::from("fere"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'substring-cara', status: 'substring', team: 'Engineering'});
            CREATE (:Person {id: 'substring-dan', status: 'substring', team: 'Operations'});
            MATCH (n:Person {status: 'substring'}) SET n.seen = true
            RETURN n.id AS id, substring(n.team, 0, 3) AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad substring projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("substring-cara"), Value::from("Eng")],
            vec![Value::from("substring-dan"), Value::from("Ope")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'substring-team'});
            MATCH (n:Person {status: 'substring'}), (t:Team {id: 'substring-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'SubstringSlice'}]->(t)
            RETURN n.id AS id, substring(r.source, 9, 5) AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship substring projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("substring-cara"), Value::from("Slice")],
            vec![Value::from("substring-dan"), Value::from("Slice")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'substring'}) SET n.substring_counted = true
            RETURN count(substring(n.team, 0, 3)) AS rows,
                   count(DISTINCT substring(n.team, 0, 3)) AS distinct_prefixes,
                   collect(substring(n.team, 0, 3)) AS prefixes;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("substring aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["Eng", "Ope"])),
        ]]
    );

    let numeric_substring =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'substring-number', score: 3}) RETURN substring(n.score, 0, 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("substring over numeric values should stay rejected");
    assert!(
        matches!(
            numeric_substring,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_substring:?}"
    );

    let negative_start =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'substring-negative', name: 'Ada'}) RETURN substring(n.name, -1, 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("negative substring offsets should stay rejected");
    assert!(
        matches!(negative_start, GrustError::CypherUnsupportedCardinality(_)),
        "{negative_start:?}"
    );

    let nested_substring =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'substring-nested', name: 'Ada'}) RETURN substring(toLower(n.name), 0, 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested substring arguments should stay rejected");
    assert!(
        matches!(
            nested_substring,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{nested_substring:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_replace_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'replace-ada', name: 'Ada Lovelace'});
            CREATE (b:Person {id: 'replace-bob'});
            MATCH (a:Person {id: 'replace-ada'}), (b:Person {id: 'replace-bob'})
            CREATE (a)-[e:KNOWS {id: 'replace-knows', source: 'Conference'}]->(b)
            RETURN replace(a.name, 'Ada', 'Augusta') AS renamed,
                   replace(e.source, $search, $replacement) AS source,
                   replace(a.nickname, 'x', 'y') AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("search".to_string(), Value::from("ference")),
                    ("replacement".to_string(), Value::from("gress")),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete replace projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("Augusta Lovelace"),
            Value::from("Congress"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'replace-cara', status: 'replace', team: 'eng-team'});
            CREATE (:Person {id: 'replace-dan', status: 'replace', team: 'ops-team'});
            MATCH (n:Person {status: 'replace'}) SET n.seen = true
            RETURN n.id AS id, replace(n.team, '-team', '') AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad replace projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("replace-cara"), Value::from("eng")],
            vec![Value::from("replace-dan"), Value::from("ops")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'replace-team'});
            MATCH (n:Person {status: 'replace'}), (t:Team {id: 'replace-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'ReplaceSlice'}]->(t)
            RETURN n.id AS id, replace(r.source, 'Replace', 'Row') AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship replace projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("replace-cara"), Value::from("RowSlice")],
            vec![Value::from("replace-dan"), Value::from("RowSlice")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'replace'}) SET n.replace_counted = true
            RETURN count(replace(n.team, '-team', '')) AS rows,
                   count(DISTINCT replace(n.team, '-team', '')) AS distinct_teams,
                   collect(replace(n.team, '-team', '')) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("replace aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["eng", "ops"])),
        ]]
    );

    let numeric_replace =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'replace-number', score: 3}) RETURN replace(n.score, '3', '4');",
            CypherMutationOptions::default(),
        ))
        .expect_err("replace over numeric values should stay rejected");
    assert!(
        matches!(numeric_replace, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_replace:?}"
    );

    let non_string_search =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'replace-search', name: 'Ada'}) RETURN replace(n.name, 1, 'A');",
            CypherMutationOptions::default(),
        ))
        .expect_err("replace search argument should stay string-only");
    assert!(
        matches!(
            non_string_search,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{non_string_search:?}"
    );

    let nested_replace =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'replace-nested', name: 'Ada'}) RETURN replace(toLower(n.name), 'a', 'A');",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested replace arguments should stay rejected");
    assert!(
        matches!(nested_replace, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_replace:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_predicates_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'predicate-ada', name: 'Ada Lovelace'});
            CREATE (b:Person {id: 'predicate-bob'});
            MATCH (a:Person {id: 'predicate-ada'}), (b:Person {id: 'predicate-bob'})
            CREATE (a)-[e:KNOWS {id: 'predicate-knows', source: 'Conference'}]->(b)
            RETURN startsWith(a.name, 'Ada') AS starts,
                   endsWith(a.name, $suffix) AS ends,
                   contains(e.source, 'fer') AS contains_source,
                   contains(a.nickname, 'x') AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("suffix".to_string(), Value::from("lace"))]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete string predicate projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'predicate-cara', status: 'predicate', team: 'engineering'});
            CREATE (:Person {id: 'predicate-dan', status: 'predicate', team: 'operations'});
            MATCH (n:Person {status: 'predicate'}) SET n.seen = true
            RETURN n.id AS id, startsWith(n.team, 'eng') AS engineering
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string predicate projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("predicate-cara"), Value::Bool(true)],
            vec![Value::from("predicate-dan"), Value::Bool(false)],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'predicate-team'});
            MATCH (n:Person {status: 'predicate'}), (t:Team {id: 'predicate-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'PredicateSlice'}]->(t)
            RETURN n.id AS id, endsWith(r.source, 'Slice') AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string predicate projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("predicate-cara"), Value::Bool(true)],
            vec![Value::from("predicate-dan"), Value::Bool(true)],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'predicate'}) SET n.predicate_counted = true
            RETURN count(startsWith(n.team, 'eng')) AS rows,
                   count(DISTINCT startsWith(n.team, 'eng')) AS distinct_states,
                   collect(startsWith(n.team, 'eng')) AS states;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string predicate aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );

    let numeric_predicate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-number', score: 3}) RETURN contains(n.score, '3');",
            CypherMutationOptions::default(),
        ))
        .expect_err("string predicates over numeric values should stay rejected");
    assert!(
        matches!(
            numeric_predicate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{numeric_predicate:?}"
    );

    let non_string_needle =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-needle', name: 'Ada'}) RETURN contains(n.name, 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("string predicate needle should stay string-only");
    assert!(
        matches!(
            non_string_needle,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{non_string_needle:?}"
    );

    let nested_predicate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'predicate-nested', name: 'Ada'}) RETURN contains(toLower(n.name), 'a');",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string predicate arguments should stay rejected");
    assert!(
        matches!(
            nested_predicate,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{nested_predicate:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_slices_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'slice-ada', name: 'Ada Lovelace'});
            CREATE (b:Person {id: 'slice-bob'});
            MATCH (a:Person {id: 'slice-ada'}), (b:Person {id: 'slice-bob'})
            CREATE (a)-[e:KNOWS {id: 'slice-knows', source: 'Conference'}]->(b)
            RETURN left(a.name, 3) AS first,
                   right(a.name, 8) AS last,
                   left(e.source, $len) AS source_prefix,
                   right(a.nickname, 2) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("len".to_string(), Value::Int(4))]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete string slice projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("Ada"),
            Value::from("Lovelace"),
            Value::from("Conf"),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'slice-cara', status: 'slice', team: 'engineering'});
            CREATE (:Person {id: 'slice-dan', status: 'slice', team: 'operations'});
            MATCH (n:Person {status: 'slice'}) SET n.seen = true
            RETURN n.id AS id, left(n.team, 3) AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string slice projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("slice-cara"), Value::from("eng")],
            vec![Value::from("slice-dan"), Value::from("ope")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'slice-team'});
            MATCH (n:Person {status: 'slice'}), (t:Team {id: 'slice-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'SliceSource'}]->(t)
            RETURN n.id AS id, right(r.source, 6) AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string slice projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("slice-cara"), Value::from("Source")],
            vec![Value::from("slice-dan"), Value::from("Source")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'slice'}) SET n.slice_counted = true
            RETURN count(left(n.team, 3)) AS rows,
                   count(DISTINCT left(n.team, 3)) AS distinct_teams,
                   collect(left(n.team, 3)) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string slice aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["eng", "ope"])),
        ]]
    );

    let numeric_slice =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-number', score: 3}) RETURN left(n.score, 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("string slices over numeric values should stay rejected");
    assert!(
        matches!(numeric_slice, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_slice:?}"
    );

    let negative_length =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-negative', name: 'Ada'}) RETURN left(n.name, -1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("string slice length should stay non-negative");
    assert!(
        matches!(negative_length, GrustError::CypherUnsupportedCardinality(_)),
        "{negative_length:?}"
    );

    let nested_slice =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'slice-nested', name: 'Ada'}) RETURN left(toLower(n.name), 1);",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string slice arguments should stay rejected");
    assert!(
        matches!(nested_slice, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_slice:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_reverse_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'reverse-ada', name: 'Ada Lovelace', tags: $tags, scores: $scores});
            CREATE (b:Person {id: 'reverse-bob'});
            MATCH (a:Person {id: 'reverse-ada'}), (b:Person {id: 'reverse-bob'})
            CREATE (a)-[e:KNOWS {id: 'reverse-knows', source: 'Conference', weights: $weights}]->(b)
            RETURN reverse(a.name) AS reversed_name,
                   reverse(e.source) AS reversed_source,
                   reverse(a.tags) AS reversed_tags,
                   reverse(a.scores) AS reversed_scores,
                   reverse(e.weights) AS reversed_weights,
                   reverse(a.nickname) AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    (
                        "tags".to_string(),
                        Value::StringArray(vec!["engineer".to_string(), "speaker".to_string()]),
                    ),
                    ("scores".to_string(), Value::IntArray(vec![7, 11])),
                    ("weights".to_string(), Value::FloatArray(vec![2.5, 4.5])),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete string and array reverse projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::from("ecalevoL adA"),
            Value::from("ecnerefnoC"),
            Value::StringArray(vec!["speaker".to_string(), "engineer".to_string()]),
            Value::IntArray(vec![11, 7]),
            Value::FloatArray(vec![4.5, 2.5]),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'reverse-cara', status: 'reverse', team: 'engineering'});
            CREATE (:Person {id: 'reverse-dan', status: 'reverse', team: 'operations'});
            MATCH (n:Person {status: 'reverse'}) SET n.seen = true
            RETURN n.id AS id, reverse(n.team) AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string reverse projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("reverse-cara"), Value::from("gnireenigne")],
            vec![Value::from("reverse-dan"), Value::from("snoitarepo")],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'reverse-team'});
            MATCH (n:Person {status: 'reverse'}), (t:Team {id: 'reverse-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'ReverseSource'}]->(t)
            RETURN n.id AS id, reverse(r.source) AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string reverse projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![Value::from("reverse-cara"), Value::from("ecruoSesreveR")],
            vec![Value::from("reverse-dan"), Value::from("ecruoSesreveR")],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'reverse'}) SET n.reverse_counted = true
            RETURN count(reverse(n.team)) AS rows,
                   count(DISTINCT reverse(n.team)) AS distinct_teams,
                   collect(reverse(n.team)) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string reverse aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!(["gnireenigne", "snoitarepo"])),
        ]]
    );

    let numeric_reverse =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'reverse-number', score: 3}) RETURN reverse(n.score);",
            CypherMutationOptions::default(),
        ))
        .expect_err("reverse over numeric values should stay rejected");
    assert!(
        matches!(numeric_reverse, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_reverse:?}"
    );

    let nested_reverse =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'reverse-nested', name: 'Ada'}) RETURN reverse(toLower(n.name));",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string reverse arguments should stay rejected");
    assert!(
        matches!(nested_reverse, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_reverse:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_string_split_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'split-ada', path: 'people/ada/lovelace'});
            CREATE (b:Person {id: 'split-bob'});
            MATCH (a:Person {id: 'split-ada'}), (b:Person {id: 'split-bob'})
            CREATE (a)-[e:KNOWS {id: 'split-knows', source: 'Conference:Talk'}]->(b)
            RETURN split(a.path, '/') AS path_parts,
                   split(e.source, $delimiter) AS source_parts,
                   split(a.nickname, '/') AS missing_name;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([("delimiter".to_string(), Value::from(":"))]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("concrete string split projections");
    assert_eq!(
        concrete.table.rows,
        vec![vec![
            Value::Json(serde_json::json!(["people", "ada", "lovelace"])),
            Value::Json(serde_json::json!(["Conference", "Talk"])),
            Value::Null,
        ]]
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'split-cara', status: 'split', team: 'engineering/platform'});
            CREATE (:Person {id: 'split-dan', status: 'split', team: 'operations/support'});
            MATCH (n:Person {status: 'split'}) SET n.seen = true
            RETURN n.id AS id, split(n.team, '/') AS team
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad string split projections");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![
                Value::from("split-cara"),
                Value::Json(serde_json::json!(["engineering", "platform"])),
            ],
            vec![
                Value::from("split-dan"),
                Value::Json(serde_json::json!(["operations", "support"])),
            ],
        ]
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'split-team'});
            MATCH (n:Person {status: 'split'}), (t:Team {id: 'split-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'Split|Source'}]->(t)
            RETURN n.id AS id, split(r.source, '|') AS source
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing relationship string split projections");
    assert_eq!(
        row_edges.table.rows,
        vec![
            vec![
                Value::from("split-cara"),
                Value::Json(serde_json::json!(["Split", "Source"])),
            ],
            vec![
                Value::from("split-dan"),
                Value::Json(serde_json::json!(["Split", "Source"])),
            ],
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'split'}) SET n.split_counted = true
            RETURN count(split(n.team, '/')) AS rows,
                   count(DISTINCT split(n.team, '/')) AS distinct_teams,
                   collect(split(n.team, '/')) AS teams;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("string split aggregate projections");
    assert_eq!(
        aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([
                ["engineering", "platform"],
                ["operations", "support"]
            ])),
        ]]
    );

    let numeric_split =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'split-number', score: 3}) RETURN split(n.score, '/');",
            CypherMutationOptions::default(),
        ))
        .expect_err("string split over numeric values should stay rejected");
    assert!(
        matches!(numeric_split, GrustError::CypherUnsupportedCardinality(_)),
        "{numeric_split:?}"
    );

    let empty_delimiter =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'split-empty', path: 'abc'}) RETURN split(n.path, '');",
            CypherMutationOptions::default(),
        ))
        .expect_err("string split delimiter should stay non-empty");
    assert!(
        matches!(empty_delimiter, GrustError::CypherUnsupportedCardinality(_)),
        "{empty_delimiter:?}"
    );

    let nested_split =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'split-nested', path: 'a/b'}) RETURN split(toLower(n.path), '/');",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested string split arguments should stay rejected");
    assert!(
        matches!(nested_split, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_split:?}"
    );
}

#[test]
fn sail_cypher_returning_projects_restricted_case_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'case-ada', team: 'eng'})
            RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("concrete CASE projection");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec!["group".to_string()],
            rows: vec![vec![Value::from("engineering")]],
        }
    );

    let broad =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'case-bob', status: 'case', team: 'eng'});
            CREATE (:Person {id: 'case-cara', status: 'case', team: 'ops'});
            MATCH (n:Person {status: 'case'}) SET n.seen = true
            RETURN n.id AS id,
                   CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad row CASE projection");
    assert_eq!(
        broad.table.rows,
        vec![
            vec![Value::from("case-bob"), Value::from("engineering")],
            vec![Value::from("case-cara"), Value::from("other")]
        ]
    );

    let row_edge =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'case-team'});
            MATCH (n:Person {status: 'case'}), (t:Team {id: 'case-team'})
            CREATE (n)-[r:MEMBER_OF {source: 'case'}]->(t)
            RETURN n.id AS id,
                   CASE WHEN r.source = 'case' THEN 'matched' ELSE 'missed' END AS edge_case,
                   CASE WHEN t.id = 'case-team' THEN true ELSE false END AS endpoint_case
            ORDER BY id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("row-producing CASE projection");
    assert_eq!(
        row_edge.table.rows,
        vec![
            vec![
                Value::from("case-bob"),
                Value::from("matched"),
                Value::Bool(true)
            ],
            vec![
                Value::from("case-cara"),
                Value::from("matched"),
                Value::Bool(true)
            ],
        ]
    );

    let grouped =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'case'}) SET n.counted = true
            RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group,
                   count(*) AS people
            ORDER BY group;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("grouped CASE projection");
    assert_eq!(
        grouped.table.rows,
        vec![
            vec![Value::from("engineering"), Value::Int(1)],
            vec![Value::from("other"), Value::Int(1)]
        ]
    );

    let parameterized =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'case'}) SET n.parameterized = true
            RETURN n.id AS id,
                   CASE WHEN n.team = $team THEN $matched ELSE $unmatched END AS group
            ORDER BY id;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("team".to_string(), Value::from("eng")),
                    ("matched".to_string(), Value::from("engineering")),
                    ("unmatched".to_string(), Value::from("other")),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("parameterized CASE projection");
    assert_eq!(
        parameterized.table.rows,
        vec![
            vec![Value::from("case-bob"), Value::from("engineering")],
            vec![Value::from("case-cara"), Value::from("other")]
        ]
    );

    let aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'case'}) SET n.aggregated = true
            RETURN count(CASE WHEN n.team = 'eng' THEN 1 ELSE null END) AS eng_count,
                   count(DISTINCT CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_teams,
                   sum(CASE WHEN n.team = 'eng' THEN 1 ELSE 0 END) AS eng_sum,
                   avg(CASE WHEN n.team = 'eng' THEN 10 ELSE 2 END) AS score_avg,
                   min(CASE WHEN n.team = 'eng' THEN 'a' ELSE 'z' END) AS first_bucket,
                   max(CASE WHEN n.team = 'eng' THEN 'a' ELSE 'z' END) AS last_bucket,
                   collect(CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_ids;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted CASE aggregate projections");
    assert_eq!(
        aggregates.table.columns,
        vec![
            "eng_count".to_string(),
            "eng_teams".to_string(),
            "eng_sum".to_string(),
            "score_avg".to_string(),
            "first_bucket".to_string(),
            "last_bucket".to_string(),
            "eng_ids".to_string(),
        ]
    );
    assert_eq!(aggregates.table.rows[0][0], Value::Int(1));
    assert_eq!(aggregates.table.rows[0][1], Value::Int(1));
    assert_eq!(aggregates.table.rows[0][2], Value::Int(1));
    assert_eq!(aggregates.table.rows[0][3], Value::Float(6.0));
    assert_eq!(aggregates.table.rows[0][4], Value::from("a"));
    assert_eq!(aggregates.table.rows[0][5], Value::from("z"));
    assert_eq!(
        aggregates.table.rows[0][6],
        Value::Json(serde_json::json!(["eng"]))
    );

    let grouped_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'case'}) SET n.group_aggregated = true
            RETURN CASE WHEN n.team = 'eng' THEN 'engineering' ELSE 'other' END AS group,
                   sum(CASE WHEN n.team = 'eng' THEN 1 ELSE 0 END) AS eng_sum,
                   collect(CASE WHEN n.team = 'eng' THEN 'eng' ELSE null END) AS eng_ids
            ORDER BY group;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("grouped restricted CASE aggregate projections");
    assert_eq!(
        grouped_aggregates.table.rows,
        vec![
            vec![
                Value::from("engineering"),
                Value::Int(1),
                Value::Json(serde_json::json!(["eng"]))
            ],
            vec![
                Value::from("other"),
                Value::Int(0),
                Value::Json(serde_json::json!([]))
            ]
        ]
    );

    let parameterized_aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'case'}) SET n.parameterized_aggregate = true
            RETURN sum(CASE WHEN n.team = $team THEN $matched ELSE $unmatched END) AS score;
            ",
            CypherMutationOptions {
                parameters: CypherParameters::from([
                    ("team".to_string(), Value::from("eng")),
                    ("matched".to_string(), Value::Int(3)),
                    ("unmatched".to_string(), Value::Int(1)),
                ]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("parameterized CASE aggregate projection");
    assert_eq!(
        parameterized_aggregate.table.rows,
        vec![vec![Value::Int(4)]]
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'case'}) SET n.flag = true
             RETURN sum(CASE WHEN lower(n.team) = 'eng' THEN 1 ELSE 0 END);",
            CypherMutationOptions::default(),
        ))
        .expect_err("aggregate CASE functions should stay rejected");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'case'}) SET n.flag = true
             RETURN CASE WHEN n.team = $missing THEN 'match' ELSE 'miss' END;",
            CypherMutationOptions::default(),
        ))
        .expect_err("missing CASE parameter should be rejected");
    assert!(
        matches!(error, GrustError::CypherUnresolvedIdentity(_)),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'case'}) SET n.flag = true
             RETURN CASE WHEN n.team > 'eng' THEN 'other' ELSE 'engineering' END;",
            CypherMutationOptions::default(),
        ))
        .expect_err("unsupported CASE predicate operator should be rejected");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {status: 'case'}) SET n.flag = true
             RETURN CASE WHEN n.team = 'eng' THEN n.id ELSE 'other' END;",
            CypherMutationOptions::default(),
        ))
        .expect_err("CASE branches should stay literal-only");
    assert!(matches!(error, GrustError::Unsupported(_)), "{error:?}");
}

#[test]
fn sail_cypher_returning_projects_broad_node_rows_on_memory_facade() {
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new(
                "Person",
                "ada",
                Props::from([
                    ("name".to_string(), Value::from("Ada")),
                    ("status".to_string(), Value::from("active")),
                    ("nickname".to_string(), Value::from("ada")),
                ]),
            ),
            Node::new(
                "Person",
                "bob",
                Props::from([
                    ("name".to_string(), Value::from("Bob")),
                    ("status".to_string(), Value::from("active")),
                    ("nickname".to_string(), Value::from("bob")),
                ]),
            ),
            Node::new(
                "Person",
                "eve",
                Props::from([
                    ("name".to_string(), Value::from("Eve")),
                    ("status".to_string(), Value::from("inactive")),
                ]),
            ),
        ],
        vec![],
    )))
    .unwrap();

    let set_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'active'})
            SET n.seen = true
            RETURN n.id, n.name, n.seen, n.label;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        set_result.mutation.report,
        GraphMutationReport {
            patches: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_patches: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        set_result.table,
        CypherResultTable {
            columns: vec![
                "n.id".to_string(),
                "n.name".to_string(),
                "n.seen".to_string(),
                "n.label".to_string()
            ],
            rows: vec![
                vec![
                    Value::from("ada"),
                    Value::from("Ada"),
                    Value::Bool(true),
                    Value::from("Person")
                ],
                vec![
                    Value::from("bob"),
                    Value::from("Bob"),
                    Value::Bool(true),
                    Value::from("Person")
                ],
            ],
        }
    );

    let remove_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'active'})
            REMOVE n.nickname
            RETURN n.id, n.nickname;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        remove_result.table,
        CypherResultTable {
            columns: vec!["n.id".to_string(), "n.nickname".to_string()],
            rows: vec![
                vec![Value::from("ada"), Value::Null],
                vec![Value::from("bob"), Value::Null],
            ],
        }
    );

    let ordered_store = MemoryGraphStore::new();
    futures_executor::block_on(ordered_store.put_node(&Node::new(
        "Person",
        "grace",
        Props::from([("status".to_string(), Value::from("inactive"))]),
    )))
    .unwrap();
    let ordered_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &ordered_store,
            "
            MATCH (m:Person {status: 'inactive'})
            SET m.status = 'active';
            MATCH (n:Person {status: 'active'})
            SET n.seen = true
            RETURN n.id, n.status, n.seen;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        ordered_result.table,
        CypherResultTable {
            columns: vec![
                "n.id".to_string(),
                "n.status".to_string(),
                "n.seen".to_string()
            ],
            rows: vec![vec![
                Value::from("grace"),
                Value::from("active"),
                Value::Bool(true)
            ]],
        }
    );
}

#[test]
fn sail_cypher_returning_projects_deleted_broad_node_rows_on_memory_facade() {
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new(
                "Person",
                "ada",
                Props::from([
                    ("name".to_string(), Value::from("Ada")),
                    ("status".to_string(), Value::from("inactive")),
                ]),
            ),
            Node::new(
                "Person",
                "bob",
                Props::from([
                    ("name".to_string(), Value::from("Bob")),
                    ("status".to_string(), Value::from("inactive")),
                ]),
            ),
            Node::new(
                "Person",
                "cara",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
        ],
        vec![],
    )))
    .unwrap();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (n:Person {status: 'inactive'})
            DELETE n
            RETURN n.id, n.name ORDER BY n.id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad node delete can return deleted matched rows");

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_deletes: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["n.id".to_string(), "n.name".to_string()],
            rows: vec![
                vec![Value::from("ada"), Value::from("Ada")],
                vec![Value::from("bob"), Value::from("Bob")],
            ],
        }
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("ada")))
            .unwrap()
            .is_none()
    );
}

#[test]
fn sail_cypher_returning_projects_broad_edge_rows_on_memory_facade() {
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new(
                "Person",
                "ada",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
            Node::new(
                "Person",
                "bob",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
            Node::new(
                "Person",
                "eve",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        ],
        vec![
            Edge::new(
                "KNOWS",
                "ada",
                "bob",
                Props::from([("weight".to_string(), Value::Int(3))]),
            )
            .with_id("edge-1"),
            Edge::new(
                "KNOWS",
                "ada",
                "eve",
                Props::from([("weight".to_string(), Value::Int(7))]),
            )
            .with_id("edge-2"),
        ],
    )))
    .unwrap();

    let set_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
            SET e.seen = true
            RETURN e.id, e.label, e.weight, e.seen;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        set_result.mutation.report,
        GraphMutationReport {
            patches: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_patches: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        set_result.table,
        CypherResultTable {
            columns: vec![
                "e.id".to_string(),
                "e.label".to_string(),
                "e.weight".to_string(),
                "e.seen".to_string()
            ],
            rows: vec![vec![
                Value::from("edge-1"),
                Value::from("KNOWS"),
                Value::Int(3),
                Value::Bool(true)
            ]],
        }
    );

    let remove_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
            REMOVE e.weight
            RETURN e.id, e.weight;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        remove_result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.weight".to_string()],
            rows: vec![vec![Value::from("edge-1"), Value::Null]],
        }
    );

    let ordered_store = MemoryGraphStore::new();
    futures_executor::block_on(ordered_store.put_graph(&Graph::new(
        vec![
            Node::new("Person", "ada", Props::new()),
            Node::new("Person", "bob", Props::new()),
        ],
        vec![
            Edge::new(
                "KNOWS",
                "ada",
                "bob",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            )
            .with_id("edge-3"),
        ],
    )))
    .unwrap();
    let ordered_result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &ordered_store,
            "
            MATCH (:Person {id: 'ada'})-[f:KNOWS {status: 'inactive'}]->(:Person {id: 'bob'})
            SET f.status = 'active';
            MATCH (:Person {id: 'ada'})-[e:KNOWS {status: 'active'}]->(:Person {id: 'bob'})
            SET e.seen = true
            RETURN e.id, e.status, e.seen;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        ordered_result.table,
        CypherResultTable {
            columns: vec![
                "e.id".to_string(),
                "e.status".to_string(),
                "e.seen".to_string()
            ],
            rows: vec![vec![
                Value::from("edge-3"),
                Value::from("active"),
                Value::Bool(true)
            ]],
        }
    );
}

#[test]
fn sail_cypher_returning_projects_deleted_broad_edge_rows_on_memory_facade() {
    let store = MemoryGraphStore::new();

    futures_executor::block_on(store.put_graph(&Graph::new(
        vec![
            Node::new(
                "Person",
                "ada",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
            Node::new(
                "Person",
                "bob",
                Props::from([("status".to_string(), Value::from("active"))]),
            ),
            Node::new(
                "Person",
                "eve",
                Props::from([("status".to_string(), Value::from("inactive"))]),
            ),
        ],
        vec![
            Edge::new(
                "KNOWS",
                "ada",
                "bob",
                Props::from([("weight".to_string(), Value::Int(3))]),
            )
            .with_id("edge-1"),
            Edge::new(
                "KNOWS",
                "ada",
                "eve",
                Props::from([("weight".to_string(), Value::Int(7))]),
            )
            .with_id("edge-2"),
        ],
    )))
    .unwrap();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
            DELETE e
            RETURN e.id, e.label, e.weight;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("broad edge delete can return deleted matched rows");

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "e.id".to_string(),
                "e.label".to_string(),
                "e.weight".to_string()
            ],
            rows: vec![vec![
                Value::from("edge-1"),
                Value::from("KNOWS"),
                Value::Int(3)
            ]],
        }
    );
    assert_eq!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .unwrap()
            .into_iter()
            .map(|edge| edge.id.map(|id| id.as_str().to_string()))
            .collect::<Vec<_>>(),
        vec![Some("edge-2".to_string())]
    );
}

#[test]
fn sail_cypher_returning_evaluates_row_produced_edge_values() {
    let planned = sail_cypher_mutation_plan_with_return_options(
        "
        MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
        CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
        RETURN e.label, e.source, e.id;
        ",
        CypherMutationOptions::default(),
    )
    .unwrap();
    let mut row_edge_values = HashMap::new();
    row_edge_values.insert(
        "e".to_string(),
        vec![
            Edge::new(
                "MEMBER_OF",
                "ada",
                "eng",
                Props::from([("source".to_string(), Value::from("cypher"))]),
            ),
            Edge::new(
                "MEMBER_OF",
                "bob",
                "eng",
                Props::from([("source".to_string(), Value::from("cypher"))]),
            ),
        ],
    );

    let table = futures_executor::block_on(evaluate_cypher_return_table(
        &MemoryGraphStore::new(),
        &planned.node_bindings,
        &planned.edge_bindings,
        &HashMap::new(),
        &row_edge_values,
        &planned.row_path_bindings,
        &planned.return_clause,
    ))
    .unwrap();

    assert_eq!(
        table,
        CypherResultTable {
            columns: vec![
                "e.label".to_string(),
                "e.source".to_string(),
                "e.id".to_string()
            ],
            rows: vec![
                vec![Value::from("MEMBER_OF"), Value::from("cypher"), Value::Null],
                vec![Value::from("MEMBER_OF"), Value::from("cypher"), Value::Null]
            ],
        }
    );
}

#[test]
fn sail_cypher_returning_allows_control_words_as_aliases() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'ada', name: 'Ada'})
            RETURN n.id AS limit, n.name AS skip;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["limit".to_string(), "skip".to_string()],
            rows: vec![vec![Value::from("ada"), Value::from("Ada")]],
        }
    );
}

#[test]
fn sail_cypher_returning_generic_strict_create_checks_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'ada', name: 'Ada'}) RETURN n.id, n.name;",
            CypherMutationOptions {
                create_mode: CypherCreateMode::ErrorIfExists,
                ..CypherMutationOptions::default()
            },
        ))
        .unwrap();
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["n.id".to_string(), "n.name".to_string()],
            rows: vec![vec![Value::from("ada"), Value::from("Ada")]],
        }
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'ada', name: 'Ada again'}) RETURN n.id;",
            CypherMutationOptions {
                create_mode: CypherCreateMode::ErrorIfExists,
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("strict CREATE should reject existing node");
    assert!(matches!(error, GrustError::CypherExecution(_)));
    assert!(error.to_string().contains("would overwrite existing node"));

    let fresh = MemoryGraphStore::new();
    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &fresh,
            "
            CREATE (n:Person {id: 'ada', name: 'Ada'});
            CREATE (n:Person {id: 'ada', name: 'Ada again'})
            RETURN n.id;
            ",
            CypherMutationOptions {
                create_mode: CypherCreateMode::ErrorIfExists,
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("strict CREATE should reject duplicate node target in the same batch");
    assert!(matches!(error, GrustError::CypherExecution(_)));
    assert!(error.to_string().contains("duplicate node 'ada'"));
    assert!(
        futures_executor::block_on(fresh.get_node(&NodeId::new("ada")))
            .unwrap()
            .is_none(),
        "failed strict preflight must not partially write the first CREATE"
    );

    futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
        CREATE (b:Person {id: 'bob'});
        CREATE (a:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1'}]->(b)
        RETURN e.id;
        ",
        CypherMutationOptions::default(),
    ))
    .unwrap();
    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (a:Person {id: 'ada'})-[e:LIKES {id: 'edge-1'}]->(b:Person {id: 'bob'})
            RETURN e.id;
            ",
            CypherMutationOptions {
                create_mode: CypherCreateMode::ErrorIfExists,
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("strict CREATE should reject reused explicit edge id");
    assert!(matches!(error, GrustError::CypherExecution(_)));
    assert!(error.to_string().contains("would overwrite existing edge"));

    let fresh = MemoryGraphStore::new();
    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &fresh,
            "
            CREATE (a:Person {id: 'ada'});
            CREATE (b:Person {id: 'bob'});
            CREATE (a)-[:KNOWS {id: 'edge-1'}]->(b);
            CREATE (a)-[e:LIKES {id: 'edge-1'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions {
                create_mode: CypherCreateMode::ErrorIfExists,
                ..CypherMutationOptions::default()
            },
        ))
        .expect_err("strict CREATE should reject duplicate edge id in the same batch");
    assert!(matches!(error, GrustError::CypherExecution(_)));
    assert!(error.to_string().contains("duplicate edge 'edge-1'"));
    assert!(
        futures_executor::block_on(fresh.get_edges(EdgeQuery::default()))
            .unwrap()
            .is_empty(),
        "failed strict preflight must not partially write earlier CREATE operations"
    );
}

#[test]
fn sail_cypher_returning_rejects_deferred_result_forms() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Person {id: 'ada'}) RETURN n.id;",
            CypherMutationOptions::default(),
        ))
        .expect_err("unbound variable should be rejected");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    // ORDER BY a column that was not returned is still rejected.
    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "MATCH (n:Person {id: 'ada'}) SET n.seen = true RETURN n.id ORDER BY n.missing;",
            CypherMutationOptions::default(),
        ))
        .expect_err("ORDER BY on a non-projected column should be rejected");
    assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));
}

#[test]
fn sail_cypher_returning_groups_mixed_aggregate_rows() {
    let store = MemoryGraphStore::new();

    let grouped =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active', team: 'eng', score: 10});
            CREATE (:Person {id: 'bob', status: 'active', team: 'eng', score: 20});
            CREATE (:Person {id: 'cara', status: 'active', team: 'ops', score: 7});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN n.team AS team,
                   count(*) AS people,
                   sum(n.score) AS total,
                   collect(n.id) AS ids,
                   collect(*) AS rows
            ORDER BY total DESC;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("mixed aggregate/scalar RETURN should group by scalar projections");

    assert_eq!(
        grouped.table.columns,
        vec![
            "team".to_string(),
            "people".to_string(),
            "total".to_string(),
            "ids".to_string(),
            "rows".to_string()
        ]
    );
    assert_eq!(grouped.table.rows.len(), 2);
    assert_eq!(
        &grouped.table.rows[0][..4],
        &[
            Value::from("eng"),
            Value::Int(2),
            Value::Int(30),
            Value::Json(serde_json::json!(["ada", "bob"]))
        ]
    );
    let Value::Json(eng_rows) = &grouped.table.rows[0][4] else {
        panic!("collect(*) should return JSON rows");
    };
    assert_eq!(eng_rows.as_array().expect("array").len(), 2);
    assert_eq!(eng_rows[0]["n"]["id"], serde_json::json!("ada"));
    assert_eq!(eng_rows[1]["n"]["id"], serde_json::json!("bob"));
    assert_eq!(
        &grouped.table.rows[1][..4],
        &[
            Value::from("ops"),
            Value::Int(1),
            Value::Int(7),
            Value::Json(serde_json::json!(["cara"]))
        ]
    );
    let Value::Json(ops_rows) = &grouped.table.rows[1][4] else {
        panic!("collect(*) should return JSON rows");
    };
    assert_eq!(ops_rows.as_array().expect("array").len(), 1);
    assert_eq!(ops_rows[0]["n"]["id"], serde_json::json!("cara"));

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Audit {id: 'grouped-concrete', kind: 'write'})
             RETURN n.kind, count(*) AS writes;",
            CypherMutationOptions::default(),
        ))
        .expect("concrete row can mix scalar and aggregate projections");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec!["n.kind".to_string(), "writes".to_string()],
            rows: vec![vec![Value::from("write"), Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_counts_materialized_rows_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'ada'}) RETURN count(n) AS writes;",
            CypherMutationOptions::default(),
        ))
        .expect("count concrete write row");
    assert_eq!(
        concrete.table,
        CypherResultTable {
            columns: vec!["writes".to_string()],
            rows: vec![vec![Value::Int(1)]],
        }
    );

    let concrete_props =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'dana', email: 'dana@example.test'})
             RETURN count(n.email) AS emails, count(n.missing) AS missing;",
            CypherMutationOptions::default(),
        ))
        .expect("count concrete properties");
    assert_eq!(
        concrete_props.table,
        CypherResultTable {
            columns: vec!["emails".to_string(), "missing".to_string()],
            rows: vec![vec![Value::Int(1), Value::Int(0)]],
        }
    );

    let row_producing =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Person {id: 'cara', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
            CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
            RETURN count(e) AS relationships, count(e.source) AS sourced, count(e.id) AS explicit_ids
            LIMIT ALL;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("count row-producing write rows");
    assert_eq!(
        row_producing.table,
        CypherResultTable {
            columns: vec![
                "relationships".to_string(),
                "sourced".to_string(),
                "explicit_ids".to_string()
            ],
            rows: vec![vec![Value::Int(2), Value::Int(2), Value::Int(0)]],
        }
    );

    let star = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "CREATE (:Audit {id: 'a1'}) RETURN COUNT ( * ) AS rows;",
        CypherMutationOptions::default(),
    ))
    .expect("count star with spaces");
    assert_eq!(
        star.table,
        CypherResultTable {
            columns: vec!["rows".to_string()],
            rows: vec![vec![Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_counts_distinct_materialized_values() {
    let store = MemoryGraphStore::new();

    let row_nodes =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
            CREATE (:Person {id: 'bob', status: 'active', department: 'eng'});
            CREATE (:Person {id: 'cara', status: 'active', department: 'ops'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN count(n.department) AS departments,
                   count(DISTINCT n.department) AS distinct_departments,
                   count(DISTINCT n.missing) AS missing;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("count distinct row node properties");
    assert_eq!(
        row_nodes.table,
        CypherResultTable {
            columns: vec![
                "departments".to_string(),
                "distinct_departments".to_string(),
                "missing".to_string()
            ],
            rows: vec![vec![Value::Int(3), Value::Int(2), Value::Int(0)]],
        }
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'eng'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
            CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
            RETURN count(e) AS relationships,
                   count(DISTINCT e.label) AS distinct_labels,
                   count(DISTINCT e.source) AS distinct_sources;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("count distinct row edge properties");
    assert_eq!(
        row_edges.table,
        CypherResultTable {
            columns: vec![
                "relationships".to_string(),
                "distinct_labels".to_string(),
                "distinct_sources".to_string()
            ],
            rows: vec![vec![Value::Int(3), Value::Int(1), Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_evaluates_restricted_numeric_aggregates() {
    let store = MemoryGraphStore::new();

    let row_nodes =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active', score: 10, team: 'eng'});
            CREATE (:Person {id: 'bob', status: 'active', score: 20, team: 'eng'});
            CREATE (:Person {id: 'cara', status: 'active', score: 20, team: 'ops'});
            CREATE (:Person {id: 'dana', status: 'active'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN sum(n.score) AS total,
                   avg(n.score) AS average,
                   min(n.score) AS low,
                   max(n.score) AS high,
                   sum(DISTINCT n.score) AS distinct_total;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted numeric aggregates over broad node rows");
    assert_eq!(
        row_nodes.table,
        CypherResultTable {
            columns: vec![
                "total".to_string(),
                "average".to_string(),
                "low".to_string(),
                "high".to_string(),
                "distinct_total".to_string()
            ],
            rows: vec![vec![
                Value::Int(50),
                Value::Float(50.0 / 3.0),
                Value::Int(10),
                Value::Int(20),
                Value::Int(30),
            ]],
        }
    );

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'eng'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
            CREATE (n)-[e:MEMBER_OF {weight: 1.5, source: 'cypher'}]->(t)
            RETURN sum(e.weight) AS total_weight,
                   avg(e.weight) AS average_weight,
                   min(e.source) AS first_source,
                   max(e.source) AS last_source;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted aggregates over row-producing edges");
    assert_eq!(
        row_edges.table,
        CypherResultTable {
            columns: vec![
                "total_weight".to_string(),
                "average_weight".to_string(),
                "first_source".to_string(),
                "last_source".to_string()
            ],
            rows: vec![vec![
                Value::Float(6.0),
                Value::Float(1.5),
                Value::from("cypher"),
                Value::from("cypher"),
            ]],
        }
    );

    let missing =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Audit {id: 'aggregate-missing'}) RETURN sum(a.missing) AS missing;",
            CypherMutationOptions::default(),
        ))
        .expect_err("unbound aggregate variable should fail");
    assert!(matches!(missing, GrustError::CypherUnresolvedIdentity(_)));
}

#[test]
fn sail_cypher_returning_rejects_unsupported_aggregate_forms() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'ada', name: 'Ada'}) RETURN sum(n.name);",
            CypherMutationOptions::default(),
        ))
        .expect_err("SUM over strings should fail");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'cara', score: 1}) RETURN avg(n);",
            CypherMutationOptions::default(),
        ))
        .expect_err("non-count aggregate over element should fail");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Person {id: 'dana', score: 1}) RETURN sum(*);",
            CypherMutationOptions::default(),
        ))
        .expect_err("non-count aggregate star should fail");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );
}

#[test]
fn sail_cypher_returning_collects_restricted_materialized_values() {
    let store = MemoryGraphStore::new();

    let row_nodes =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active', team: 'eng'});
            CREATE (:Person {id: 'bob', status: 'active', team: 'eng'});
            CREATE (:Person {id: 'cara', status: 'active', team: 'ops'});
            CREATE (:Person {id: 'dana', status: 'active'});
            MATCH (n:Person {status: 'active'}) SET n.seen = true
            RETURN collect(n.team) AS teams,
                   collect(DISTINCT n.team) AS distinct_teams,
                   collect(n.missing) AS missing,
                   collect(*) AS rows;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted collect over broad node rows");
    assert_eq!(
        row_nodes.table.columns,
        vec![
            "teams".to_string(),
            "distinct_teams".to_string(),
            "missing".to_string(),
            "rows".to_string(),
        ]
    );
    assert_eq!(
        &row_nodes.table.rows[0][..3],
        &[
            Value::Json(serde_json::json!(["eng", "eng", "ops"])),
            Value::Json(serde_json::json!(["eng", "ops"])),
            Value::Json(serde_json::json!([]))
        ]
    );
    let Value::Json(rows) = &row_nodes.table.rows[0][3] else {
        panic!("collect(*) should return JSON rows");
    };
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["n"]["id"], serde_json::json!("ada"));
    assert_eq!(rows[1]["n"]["id"], serde_json::json!("bob"));
    assert_eq!(rows[2]["n"]["id"], serde_json::json!("cara"));
    assert_eq!(rows[3]["n"]["id"], serde_json::json!("dana"));

    let row_edges =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Team {id: 'eng'});
            MATCH (n:Person {status: 'active'}), (t:Team {id: 'eng'})
            CREATE (n)-[e:MEMBER_OF {source: 'cypher'}]->(t)
            RETURN collect(e.source) AS sources,
                   collect(DISTINCT e.label) AS labels;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted collect over row-producing edge rows");
    assert_eq!(
        row_edges.table,
        CypherResultTable {
            columns: vec!["sources".to_string(), "labels".to_string()],
            rows: vec![vec![
                Value::Json(serde_json::json!(["cypher", "cypher", "cypher", "cypher"])),
                Value::Json(serde_json::json!(["MEMBER_OF"])),
            ]],
        }
    );
}

#[test]
fn sail_cypher_returning_collects_bound_elements() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (n:Person {id: 'ada', status: 'active'})
            RETURN collect(n) AS nodes, collect(DISTINCT n) AS distinct_nodes;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted collect over concrete node element");
    assert_eq!(result.table.columns, vec!["nodes", "distinct_nodes"]);
    assert_eq!(result.table.rows.len(), 1);
    let Value::Json(nodes) = &result.table.rows[0][0] else {
        panic!("collect(n) should return JSON array");
    };
    assert_eq!(nodes.as_array().expect("array").len(), 1);
    assert_eq!(nodes[0]["id"], serde_json::Value::String("ada".to_string()));
    let Value::Json(distinct_nodes) = &result.table.rows[0][1] else {
        panic!("collect(DISTINCT n) should return JSON array");
    };
    assert_eq!(distinct_nodes.as_array().expect("array").len(), 1);

    let star = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "CREATE (a:Audit {id: 'collect-star'}) RETURN collect(*) AS rows;",
        CypherMutationOptions::default(),
    ))
    .expect("collect star over concrete bound variable");
    assert_eq!(star.table.columns, vec!["rows"]);
    let Value::Json(rows) = &star.table.rows[0][0] else {
        panic!("collect(*) should return JSON rows");
    };
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["a"]["id"], serde_json::json!("collect-star"));
    assert_eq!(rows[0]["a"]["label"], serde_json::json!("Audit"));
}

#[test]
fn sail_cypher_returning_count_rejects_unbound_variables() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Person {id: 'ada'}) RETURN count(n);",
            CypherMutationOptions::default(),
        ))
        .expect_err("count over unbound variable should fail");
    assert!(matches!(error, GrustError::CypherUnresolvedIdentity(_)));

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Person {id: 'bob'}) RETURN count(DISTINCT *);",
            CypherMutationOptions::default(),
        ))
        .expect_err("COUNT DISTINCT star should stay deferred");
    assert!(matches!(error, GrustError::CypherUnsupportedCardinality(_)));
}

#[test]
fn sail_cypher_returning_accepts_limit_all_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'a', age: 30});
            CREATE (:Person {id: 'b', age: 20});
            MATCH (n:Person) SET n.seen = true
            RETURN n.id AS id ORDER BY id LIMIT ALL;
            ",
            CypherMutationOptions::default(),
        ))
        .expect("LIMIT ALL should preserve all rows");
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["id".to_string()],
            rows: vec![vec![Value::from("a")], vec![Value::from("b")]],
        }
    );
}

#[test]
fn sail_cypher_returning_accepts_offset_control() {
    let store = MemoryGraphStore::new();

    let rows = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
        CREATE (:Person {id: 'a', age: 30});
        CREATE (:Person {id: 'b', age: 20});
        CREATE (:Person {id: 'c', age: 40});
        MATCH (n:Person) SET n.seen = true
        RETURN n.id AS id, n.age AS age ORDER BY age DESC OFFSET 1 LIMIT 1;
        ",
        CypherMutationOptions::default(),
    ))
    .expect("OFFSET should behave like SKIP");
    assert_eq!(
        rows.table,
        CypherResultTable {
            columns: vec!["id".to_string(), "age".to_string()],
            rows: vec![vec![Value::from("a"), Value::Int(30)]],
        }
    );

    let aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Audit {id: 'offset-count'}) RETURN count(*) AS writes OFFSET 0 LIMIT ALL;",
            CypherMutationOptions::default(),
        ))
        .expect("OFFSET should work on aggregate table");
    assert_eq!(
        aggregate.table,
        CypherResultTable {
            columns: vec!["writes".to_string()],
            rows: vec![vec![Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_distinct_dedupes_materialized_rows() {
    let store = MemoryGraphStore::new();

    let rows = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
        CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
        CREATE (:Person {id: 'bob', status: 'active', department: 'eng'});
        CREATE (:Person {id: 'cara', status: 'active', department: 'ops'});
        MATCH (n:Person {status: 'active'}) SET n.seen = true
        RETURN DISTINCT n.department AS department ORDER BY department;
        ",
        CypherMutationOptions::default(),
    ))
    .expect("restricted RETURN DISTINCT over broad rows");

    assert_eq!(
        rows.table,
        CypherResultTable {
            columns: vec!["department".to_string()],
            rows: vec![vec![Value::from("eng")], vec![Value::from("ops")]],
        }
    );

    let aggregate =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Audit {id: 'distinct-row'}) RETURN DISTINCT count(*) AS rows;",
            CypherMutationOptions::default(),
        ))
        .expect("RETURN DISTINCT over aggregate result row");
    assert_eq!(
        aggregate.table,
        CypherResultTable {
            columns: vec!["rows".to_string()],
            rows: vec![vec![Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_distinct_requires_projection() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Person {id: 'ada'}) RETURN DISTINCT;",
            CypherMutationOptions::default(),
        ))
        .expect_err("RETURN DISTINCT without projection should fail");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn sail_cypher_returning_orders_by_projection_expression() {
    let store = MemoryGraphStore::new();

    let rows = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
        CREATE (:Person {id: 'ada', status: 'active', department: 'eng'});
        CREATE (:Person {id: 'bob', status: 'active', department: 'ops'});
        MATCH (n:Person {status: 'active'}) SET n.seen = true
        RETURN n.department AS department ORDER BY n.department DESC;
        ",
        CypherMutationOptions::default(),
    ))
    .expect("ORDER BY returned projection expression");

    assert_eq!(
        rows.table,
        CypherResultTable {
            columns: vec!["department".to_string()],
            rows: vec![vec![Value::from("ops")], vec![Value::from("eng")]],
        }
    );

    let count =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (:Audit {id: 'order-count'}) RETURN count(*) AS writes ORDER BY count(*);",
            CypherMutationOptions::default(),
        ))
        .expect("ORDER BY returned aggregate expression");
    assert_eq!(
        count.table,
        CypherResultTable {
            columns: vec!["writes".to_string()],
            rows: vec![vec![Value::Int(1)]],
        }
    );
}

#[test]
fn sail_cypher_returning_generic_row_producing_edges_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'generic'}]->(b)
            RETURN e.label, e.source, e.id;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            creates: 4,
            matched_rows: 2,
            changed_nodes: 3,
            changed_edges: 2,
            node_upserts: 3,
            edge_upserts: 2,
            node_inserts: 3,
            edge_inserts: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "e.label".to_string(),
                "e.source".to_string(),
                "e.id".to_string()
            ],
            rows: vec![
                vec![
                    Value::from("MEMBER_OF"),
                    Value::from("generic"),
                    Value::Null
                ],
                vec![
                    Value::from("MEMBER_OF"),
                    Value::from("generic"),
                    Value::Null
                ],
            ],
        }
    );
}

#[test]
fn sail_cypher_row_producing_edge_accepts_single_explicit_id() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {id: 'membership-1', source: 'cypher'}]->(b)
            RETURN e.id, e.source;
            ",
            CypherMutationOptions {
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("single row-producing edge can carry explicit id");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.source".to_string()],
            rows: vec![vec![Value::from("membership-1"), Value::from("cypher")]],
        }
    );
    assert_eq!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .expect("read edges")
            .into_iter()
            .map(|edge| edge.id.map(|id| id.as_str().to_string()))
            .collect::<Vec<_>>(),
        vec![Some("membership-1".to_string())]
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Create,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(EdgeId::new("membership-1")),
        }]
    );
}

#[test]
fn sail_cypher_row_producing_edge_collects_structural_identity() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
            RETURN e.id, e.source;
            ",
            CypherMutationOptions {
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("single row-producing edge can report structural identity");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string(), "e.source".to_string()],
            rows: vec![vec![Value::Null, Value::from("cypher")]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Create,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: None,
        }]
    );
}

#[test]
fn sail_cypher_row_producing_edge_generates_ids_for_create() {
    let store = MemoryGraphStore::new();
    let props = Props::from([("source".to_string(), Value::from("cypher"))]);
    let mut expected = vec![
        generated_row_edge_id(
            &NodeId::new("ada"),
            &Label::new("MEMBER_OF"),
            &NodeId::new("eng"),
            &props,
        ),
        generated_row_edge_id(
            &NodeId::new("bob"),
            &Label::new("MEMBER_OF"),
            &NodeId::new("eng"),
            &props,
        ),
    ];
    expected.sort();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'cypher'}]->(b)
            RETURN e.id ORDER BY e.id;
            ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreate,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing create can generate per-row edge ids");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: expected
                .iter()
                .map(|id| vec![Value::from(id.as_str())])
                .collect(),
        }
    );
    let mut written = result
        .mutation
        .written_edge_identities
        .iter()
        .map(|identity| identity.id.clone())
        .collect::<Vec<_>>();
    written.sort();
    assert_eq!(
        written,
        expected
            .iter()
            .cloned()
            .map(Some)
            .collect::<Vec<Option<EdgeId>>>()
    );
    let mut persisted = futures_executor::block_on(store.get_edges(EdgeQuery::default()))
        .expect("read generated edge ids")
        .into_iter()
        .filter_map(|edge| edge.id)
        .collect::<Vec<_>>();
    persisted.sort();
    assert_eq!(persisted, expected);
}

#[test]
fn sail_cypher_row_producing_edge_generates_ids_for_merge_when_requested() {
    let store = MemoryGraphStore::new();
    let props = Props::from([("source".to_string(), Value::from("merge"))]);
    let expected = generated_row_edge_id(
        &NodeId::new("ada"),
        &Label::new("MEMBER_OF"),
        &NodeId::new("eng"),
        &props,
    );

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            MERGE (a)-[e:MEMBER_OF {source: 'merge'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        ))
        .expect("row-producing merge can generate edge ids when explicitly requested");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: vec![vec![Value::from(expected.as_str())]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Merge,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(expected),
        }]
    );
}

#[test]
fn sail_cypher_row_producing_edge_rejects_multirow_explicit_id() {
    let store = MemoryGraphStore::new();

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Team {id: 'eng'});
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {id: 'membership-1'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions::default(),
        ))
        .expect_err("multi-row explicit relationship id should fail");

    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );
    assert!(
        futures_executor::block_on(store.get_edges(EdgeQuery::default()))
            .expect("read edges")
            .is_empty()
    );
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
            node_inserts: 3,
            edge_inserts: 1,
            edge_updates: 1,
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
        "MATCH (:Person {id: 'a'})-[e:KNOWS]->(n:Person {id: 'b'}) SET e.weight = n.weight + 1",
    )
    .expect_err("cross-variable edge expression cardinality");
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
            node_inserts: 2,
            edge_inserts: 1,
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
async fn test_execute_cypher_reports_resolved_insert_update_classification() {
    let store = store().await;

    let report = store
        .execute_cypher_mutation(
            "
            CREATE (a:Person {id: 'ada', name: 'Ada'});
            MERGE (a:Person {id: 'ada', name: 'Ada Updated'});
            CREATE (b:Person {id: 'bob', name: 'Bob'});
            CREATE (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1', weight: 1}]->(:Person {id: 'bob'});
            MERGE (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1', weight: 2}]->(:Person {id: 'bob'});
            ",
        )
        .await
        .expect("execute resolved upsert classification batch");

    assert_eq!(
        report,
        GraphMutationReport {
            creates: 3,
            merges: 2,
            changed_nodes: 3,
            changed_edges: 2,
            node_upserts: 3,
            edge_upserts: 2,
            node_inserts: 2,
            node_updates: 1,
            edge_inserts: 1,
            edge_updates: 1,
            ..GraphMutationReport::default()
        }
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
async fn test_execute_cypher_returning_row_producing_edges() {
    let store = store().await;

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Team {id: 'eng'});
            ",
        )
        .await
        .expect("seed row-producing return graph");

    let result = store
        .execute_cypher_mutation_returning(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'returning'}]->(b)
            RETURN e.label, e.source, e.id;
            ",
        )
        .await
        .expect("row-producing RETURN");

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            creates: 1,
            matched_rows: 2,
            changed_edges: 2,
            edge_upserts: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "e.label".to_string(),
                "e.source".to_string(),
                "e.id".to_string()
            ],
            rows: vec![
                vec![
                    Value::from("MEMBER_OF"),
                    Value::from("returning"),
                    Value::Null
                ],
                vec![
                    Value::from("MEMBER_OF"),
                    Value::from("returning"),
                    Value::Null
                ],
            ],
        }
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_returning_generates_row_edge_ids() {
    let store = store().await;
    let props = Props::from([("source".to_string(), Value::from("generated"))]);
    let expected = generated_row_edge_id(
        &NodeId::new("ada"),
        &Label::new("MEMBER_OF"),
        &NodeId::new("eng"),
        &props,
    );

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Team {id: 'eng'});
            ",
        )
        .await
        .expect("seed generated row edge graph");

    let result = store
        .execute_cypher_mutation_returning_with_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            CREATE (a)-[e:MEMBER_OF {source: 'generated'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreate,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        )
        .await
        .expect("row-producing RETURN with generated relationship id");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: vec![vec![Value::from(expected.as_str())]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Create,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(expected),
        }]
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_returning_generates_row_merge_edge_ids() {
    let store = store().await;
    let props = Props::from([("source".to_string(), Value::from("generated-merge"))]);
    let expected = generated_row_edge_id(
        &NodeId::new("ada"),
        &Label::new("MEMBER_OF"),
        &NodeId::new("eng"),
        &props,
    );

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Team {id: 'eng'});
            ",
        )
        .await
        .expect("seed generated row merge edge graph");

    let result = store
        .execute_cypher_mutation_returning_with_options(
            "
            MATCH (a:Person {status: 'active'}), (b:Team {id: 'eng'})
            MERGE (a)-[e:MEMBER_OF {source: 'generated-merge'}]->(b)
            RETURN e.id;
            ",
            CypherMutationOptions {
                relationship_id_policy: CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
                collect_written_edge_identities: true,
                ..CypherMutationOptions::default()
            },
        )
        .await
        .expect("row-producing MERGE RETURN with generated relationship id");

    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["e.id".to_string()],
            rows: vec![vec![Value::from(expected.as_str())]],
        }
    );
    assert_eq!(
        result.mutation.written_edge_identities,
        vec![CypherWrittenEdgeIdentity {
            kind: GraphMutationPlanKind::Merge,
            from: NodeId::new("ada"),
            label: Label::new("MEMBER_OF"),
            to: NodeId::new("eng"),
            id: Some(expected),
        }]
    );
}

#[tokio::test]
#[ignore = "requires a live Sail server on 127.0.0.1:50051"]
async fn test_execute_cypher_returning_broad_match_delete_edges() {
    let store = store().await;

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'ada', status: 'active'});
            CREATE (:Person {id: 'bob', status: 'active'});
            CREATE (:Person {id: 'eve', status: 'inactive'});
            CREATE (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-1', weight: 3}]->(:Person {id: 'bob'});
            CREATE (:Person {id: 'ada'})-[e:KNOWS {id: 'edge-2', weight: 7}]->(:Person {id: 'eve'});
            ",
        )
        .await
        .expect("seed graph for broad edge delete return");

    let result = store
        .execute_cypher_mutation_returning(
            "
            MATCH (a:Person {status: 'active'})-[e:KNOWS]->(b:Person {status: 'active'})
            DELETE e
            RETURN e.id, e.label, e.weight;
            ",
        )
        .await
        .expect("return broad deleted edge rows");

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 1,
            changed_edges: 1,
            edge_deletes: 1,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec![
                "e.id".to_string(),
                "e.label".to_string(),
                "e.weight".to_string()
            ],
            rows: vec![vec![
                Value::from("edge-1"),
                Value::from("KNOWS"),
                Value::Int(3)
            ]],
        }
    );
    assert_eq!(
        store
            .get_edges(EdgeQuery::default())
            .await
            .expect("read remaining edges")
            .into_iter()
            .map(|edge| edge.id.map(|id| id.as_str().to_string()))
            .collect::<Vec<_>>(),
        vec![Some("edge-2".to_string())]
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
async fn test_execute_cypher_returning_broad_match_delete_nodes() {
    let store = store().await;

    store
        .execute_cypher_mutation(
            "
            CREATE (:Person {id: 'person-inactive-1', name: 'Ada', status: 'inactive'});
            CREATE (:Person {id: 'person-inactive-2', name: 'Bob', status: 'inactive'});
            CREATE (:Person {id: 'person-active-1', name: 'Cam', status: 'active'});
            ",
        )
        .await
        .expect("seed graph for broad delete return");

    let result = store
        .execute_cypher_mutation_returning(
            "
            MATCH (n:Person {status: 'inactive'})
            DELETE n
            RETURN n.id, n.name ORDER BY n.id;
            ",
        )
        .await
        .expect("return broad deleted node rows");

    assert_eq!(
        result.mutation.report,
        GraphMutationReport {
            deletes: 1,
            matched_rows: 2,
            changed_nodes: 2,
            node_deletes: 2,
            ..GraphMutationReport::default()
        }
    );
    assert_eq!(
        result.table,
        CypherResultTable {
            columns: vec!["n.id".to_string(), "n.name".to_string()],
            rows: vec![
                vec![Value::from("person-inactive-1"), Value::from("Ada")],
                vec![Value::from("person-inactive-2"), Value::from("Bob")],
            ],
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
            .get_node(&NodeId::new("person-active-1"))
            .await
            .expect("read active node")
            .is_some()
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
            node_inserts: 1,
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
async fn test_execute_cypher_mutation_collects_written_node_identities() {
    let store = store().await;
    let default_result = store
        .execute_cypher_mutation_result_with_options(
            "
            CREATE (:Person {id: 'ada'});
            MERGE (:Person {id: 'bob'});
            ",
            CypherMutationOptions::default(),
        )
        .await
        .expect("execute default node writes");
    assert!(default_result.written_node_identities.is_empty());

    store.clear().await.expect("clear graph before collect run");
    let result = store
        .execute_cypher_mutation_result_with_options(
            "
            CREATE (:Person {id: 'ada'});
            MERGE (:Person {id: 'bob'});
            CREATE (n:Person {name: 'Generated'});
            ",
            CypherMutationOptions {
                node_id_policy: CypherNodeIdPolicy::GenerateForCreate,
                collect_written_node_identities: true,
                ..CypherMutationOptions::default()
            },
        )
        .await
        .expect("execute node writes with identity collection");

    assert_eq!(result.written_node_identities.len(), 3);
    assert!(
        result
            .written_node_identities
            .contains(&CypherWrittenNodeIdentity {
                kind: GraphMutationPlanKind::Create,
                label: Label::new("Person"),
                id: NodeId::new("ada"),
            })
    );
    assert!(
        result
            .written_node_identities
            .contains(&CypherWrittenNodeIdentity {
                kind: GraphMutationPlanKind::Merge,
                label: Label::new("Person"),
                id: NodeId::new("bob"),
            })
    );
    assert_eq!(result.generated_node_ids.len(), 1);
    let generated_id = result.generated_node_ids[0].id.clone();
    assert!(
        result
            .written_node_identities
            .contains(&CypherWrittenNodeIdentity {
                kind: GraphMutationPlanKind::Create,
                label: Label::new("Person"),
                id: generated_id,
            })
    );
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

#[test]
fn cypher_ddl_parses_node_unique_constraint() {
    let statements = sail_cypher_ddl(
        "CREATE CONSTRAINT person_id IF NOT EXISTS FOR (n:Person) REQUIRE n.id IS UNIQUE",
    )
    .expect("parse create constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: Some("person_id".to_string()),
            if_not_exists: true,
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_node_required_constraint_without_name() {
    let statements = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL")
        .expect("parse create constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_relationship_constraint() {
    let statements =
        sail_cypher_ddl("CREATE CONSTRAINT FOR ()-[r:KNOWS]-() REQUIRE r.since IS NOT NULL")
            .expect("parse relationship constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::EdgePropertyRequired {
                label: Label::new("KNOWS"),
                key: "since".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_accepts_legacy_on_assert_spelling() {
    let statements = sail_cypher_ddl("CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE")
        .expect("parse legacy constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::CreateConstraint {
            name: None,
            if_not_exists: false,
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );
}

#[test]
fn cypher_ddl_parses_drop_constraint() {
    let statements =
        sail_cypher_ddl("DROP CONSTRAINT person_id IF EXISTS").expect("parse drop constraint");
    assert_eq!(
        statements,
        vec![CypherDdlStatement::DropConstraint {
            name: "person_id".to_string(),
            if_exists: true,
        }]
    );
}

#[test]
fn cypher_constraints_collects_multiple_statements() {
    let constraints = sail_cypher_constraints(
        "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.id IS UNIQUE; \
         CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL",
    )
    .expect("collect constraints");
    assert_eq!(
        constraints,
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_ddl_rejects_predicate_variable_mismatch() {
    let error = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE m.id IS UNIQUE")
        .expect_err("variable mismatch must fail");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_ddl_rejects_unknown_predicate() {
    let error = sail_cypher_ddl("CREATE CONSTRAINT FOR (n:Person) REQUIRE n.id IS NODE KEY")
        .expect_err("node key must be rejected");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_constraints_rejects_drop() {
    let error = sail_cypher_constraints("DROP CONSTRAINT person_id")
        .expect_err("drop must be rejected by constraints collector");
    assert!(matches!(error, GrustError::CypherSyntax(_)), "{error:?}");
}

#[test]
fn cypher_constraint_registry_applies_create_and_drop() {
    let mut registry = CypherConstraintRegistry::new();

    let created = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
             FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("create constraint");
    assert_eq!(
        created,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.named_constraints(),
        vec![NamedGraphConstraint {
            name: "person_id".to_string(),
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        }]
    );
    assert_eq!(
        registry.constraints(),
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "id".to_string(),
        }]
    );

    let dropped = registry
        .apply_cypher("DROP CONSTRAINT person_id")
        .expect("drop constraint");
    assert_eq!(
        dropped,
        CypherDdlApplicationReport {
            dropped: 1,
            ..Default::default()
        }
    );
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_constraint_registry_honors_if_modifiers() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
             FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("initial create");

    let skipped = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id IF NOT EXISTS \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("duplicate create with IF NOT EXISTS should skip");
    assert_eq!(
        skipped,
        CypherDdlApplicationReport {
            skipped: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.constraints(),
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "id".to_string(),
        }]
    );

    let missing = registry
        .apply_cypher("DROP CONSTRAINT missing IF EXISTS")
        .expect("missing drop with IF EXISTS should not fail");
    assert_eq!(
        missing,
        CypherDdlApplicationReport {
            missing: 1,
            ..Default::default()
        }
    );
}

#[test]
fn cypher_constraint_registry_rejects_duplicate_or_missing_names() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
             FOR (n:Person) REQUIRE n.id IS UNIQUE",
        )
        .expect("initial create");

    let duplicate = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect_err("duplicate create without IF NOT EXISTS should fail");
    assert!(
        matches!(duplicate, GrustError::CypherExecution(_)),
        "{duplicate:?}"
    );

    let missing = registry
        .apply_cypher("DROP CONSTRAINT missing")
        .expect_err("missing drop without IF EXISTS should fail");
    assert!(
        matches!(missing, GrustError::CypherExecution(_)),
        "{missing:?}"
    );
}

#[test]
fn cypher_constraint_registry_preserves_anonymous_constraints() {
    let mut registry = CypherConstraintRegistry::new();
    let report = registry
        .apply_cypher(
            "CREATE CONSTRAINT FOR (n:Person) REQUIRE n.name IS NOT NULL; \
             CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create anonymous and named constraints");
    assert_eq!(
        report,
        CypherDdlApplicationReport {
            created: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        registry.anonymous_constraints(),
        &[GraphConstraint::NodePropertyRequired {
            label: Label::new("Person"),
            key: "name".to_string(),
        }]
    );
    assert_eq!(
        registry.constraints(),
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "name".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_constraint_registry_serializes_for_external_persistence() {
    let base = GraphSchema::builder()
        .required_node_property("Person", "id")
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&base);
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create named constraint");

    let json = registry.to_json().expect("serialize registry");
    assert!(json.contains("person_email"));
    let round_trip = CypherConstraintRegistry::from_json(&json).expect("deserialize registry");
    assert_eq!(round_trip, registry);
    assert_eq!(
        round_trip.constraints(),
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        ]
    );

    let error = CypherConstraintRegistry::from_json("{not json}")
        .expect_err("invalid registry JSON should fail");
    assert!(matches!(error, GrustError::Serialization(_)), "{error:?}");
}

#[test]
fn cypher_constraint_registry_projects_into_existing_schema() {
    let base = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .edge(
            "KNOWS",
            vec![Label::new("Person")],
            vec![Label::new("Person")],
            vec![Field::optional("since", FieldType::Int)],
        )
        .build();
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create constraint");

    let schema = registry.apply_to_schema(&base);
    assert_eq!(schema.nodes, base.nodes);
    assert_eq!(schema.edges, base.edges);
    assert_eq!(
        schema.constraints,
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "email".to_string(),
        }]
    );
}

#[test]
fn cypher_constraint_registry_can_start_from_schema_constraints() {
    let base = GraphSchema::builder()
        .node("Person", vec![Field::required("id", FieldType::String)])
        .required_node_property("Person", "id")
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&base);
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create named constraint");

    let schema = registry.apply_to_schema(&base);
    assert_eq!(
        schema.constraints,
        vec![
            GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
            GraphConstraint::NodePropertyRequired {
                label: Label::new("Person"),
                key: "id".to_string(),
            },
        ]
    );
}

#[test]
fn cypher_constraint_registry_batches_are_atomic() {
    let mut registry = CypherConstraintRegistry::new();
    let error = registry
        .apply_cypher(
            "CREATE CONSTRAINT person_id FOR (n:Person) REQUIRE n.id IS UNIQUE; \
             DROP CONSTRAINT missing",
        )
        .expect_err("failing batch should reject");
    assert!(matches!(error, GrustError::CypherExecution(_)), "{error:?}");
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_ddl_schema_helper_applies_schema_to_store() {
    let store = MemoryGraphStore::new();
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&schema);

    let applied = futures_executor::block_on(apply_cypher_ddl_to_schema(
        &store,
        &schema,
        &mut registry,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect("apply DDL to schema and store");

    assert_eq!(
        applied.report,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(
        applied.schema.constraints,
        vec![GraphConstraint::NodePropertyUnique {
            label: Label::new("Person"),
            key: "email".to_string(),
        }]
    );

    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect("first unique email");
    let error = futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("ada@example.test"))]),
    )))
    .expect_err("applied schema should reject duplicate unique property");
    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
}

#[test]
fn cypher_ddl_schema_helper_does_not_mutate_registry_when_store_rejects_schema() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("first node");
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("second node");

    let schema = GraphSchema::builder()
        .node("Person", vec![Field::optional("email", FieldType::String)])
        .build();
    let mut registry = CypherConstraintRegistry::from_schema(&schema);
    let error = futures_executor::block_on(apply_cypher_ddl_to_schema(
        &store,
        &schema,
        &mut registry,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect_err("schema validation should reject existing duplicates");

    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert!(registry.constraints().is_empty());
}

#[test]
fn cypher_schema_manager_applies_ddl_and_exports_registry() {
    let store = MemoryGraphStore::new();
    let schema = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::required("id", FieldType::String),
                Field::optional("email", FieldType::String),
            ],
        )
        .build();
    let mut manager = CypherSchemaManager::new(schema);

    let applied = futures_executor::block_on(manager.apply_cypher_ddl(
        &store,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect("manager applies DDL");

    assert_eq!(
        applied.report,
        CypherDdlApplicationReport {
            created: 1,
            ..Default::default()
        }
    );
    assert_eq!(manager.schema, applied.schema);
    assert_eq!(
        manager.registry.named_constraints(),
        vec![NamedGraphConstraint {
            name: "person_email".to_string(),
            constraint: GraphConstraint::NodePropertyUnique {
                label: Label::new("Person"),
                key: "email".to_string(),
            },
        }]
    );

    let registry_json = manager.registry_json().expect("export registry");
    let imported = CypherSchemaManager::from_registry_json(manager.schema.clone(), &registry_json)
        .expect("import registry");
    assert_eq!(imported, manager);
}

#[test]
fn cypher_schema_manager_keeps_state_when_schema_apply_fails() {
    let store = MemoryGraphStore::new();
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("first node");
    futures_executor::block_on(store.put_node(&Node::new(
        "Person",
        "p2",
        Props::from([("email".to_string(), Value::from("same@example.test"))]),
    )))
    .expect("second node");

    let schema = GraphSchema::builder()
        .node("Person", vec![Field::optional("email", FieldType::String)])
        .build();
    let mut manager = CypherSchemaManager::new(schema);
    let before = manager.clone();

    let error = futures_executor::block_on(manager.apply_cypher_ddl(
        &store,
        "CREATE CONSTRAINT person_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
    ))
    .expect_err("manager should surface backend schema validation failure");

    assert!(matches!(error, GrustError::Schema(_)), "{error:?}");
    assert_eq!(manager, before);
}

#[test]
fn unique_node_conflict_detects_persisted_duplicate() {
    let existing = vec![
        Node::new(
            "Person",
            "p1",
            Props::from([("email".to_string(), Value::from("a@x.com"))]),
        ),
        Node::new(
            "Person",
            "p2",
            Props::from([("email".to_string(), Value::from("b@x.com"))]),
        ),
    ];
    // A different node id reusing p1's email conflicts.
    let candidate = Node::new(
        "Person",
        "p3",
        Props::from([("email".to_string(), Value::from("a@x.com"))]),
    );
    assert_eq!(
        unique_node_conflict(&existing, &candidate, &Label::new("Person"), "email"),
        Some(&NodeId::new("p1"))
    );
}

#[test]
fn unique_node_conflict_allows_same_id_update_and_other_labels() {
    let existing = vec![Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("a@x.com"))]),
    )];
    // Same id rewriting its own value is an update, not a conflict.
    let update = Node::new(
        "Person",
        "p1",
        Props::from([("email".to_string(), Value::from("a@x.com"))]),
    );
    assert_eq!(
        unique_node_conflict(&existing, &update, &Label::new("Person"), "email"),
        None
    );
    // A different label is unaffected by the Person constraint.
    let other = Node::new(
        "Company",
        "c1",
        Props::from([("email".to_string(), Value::from("a@x.com"))]),
    );
    assert_eq!(
        unique_node_conflict(&existing, &other, &Label::new("Person"), "email"),
        None
    );
}

#[test]
fn unique_edge_conflict_uses_structural_identity() {
    let existing = vec![Edge::new(
        "RATED",
        "u1",
        "m1",
        Props::from([("token".to_string(), Value::from("t1"))]),
    )];
    // Different endpoints reusing the token conflict.
    let candidate = Edge::new(
        "RATED",
        "u2",
        "m2",
        Props::from([("token".to_string(), Value::from("t1"))]),
    );
    assert_eq!(
        unique_edge_conflict(&existing, &candidate, &Label::new("RATED"), "token"),
        Some(edge_key(&existing[0]))
    );
    // The same structural edge rewriting its own token is an update.
    let update = Edge::new(
        "RATED",
        "u1",
        "m1",
        Props::from([("token".to_string(), Value::from("t1"))]),
    );
    assert_eq!(
        unique_edge_conflict(&existing, &update, &Label::new("RATED"), "token"),
        None
    );
}

#[test]
fn sail_cypher_returning_orders_skips_and_limits_rows() {
    let store = MemoryGraphStore::new();
    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'a', age: 30});
            CREATE (:Person {id: 'b', age: 20});
            CREATE (:Person {id: 'c', age: 40});
            MATCH (n:Person) SET n.seen = true
            RETURN n.id, n.age ORDER BY n.age DESC SKIP 1 LIMIT 1;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();
    assert_eq!(
        result.table.columns,
        vec!["n.id".to_string(), "n.age".to_string()]
    );
    // ages descending: c(40), a(30), b(20); SKIP 1 drops c; LIMIT 1 keeps a.
    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("a"), Value::Int(30)]]
    );
}

#[test]
fn sail_cypher_returning_orders_ascending_with_alias() {
    let store = MemoryGraphStore::new();
    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
            CREATE (:Person {id: 'a', age: 30});
            CREATE (:Person {id: 'b', age: 20});
            CREATE (:Person {id: 'c', age: 40});
            MATCH (n:Person) SET n.seen = true
            RETURN n.id AS id, n.age AS age ORDER BY age;
            ",
            CypherMutationOptions::default(),
        ))
        .unwrap();
    assert_eq!(
        result.table.columns,
        vec!["id".to_string(), "age".to_string()]
    );
    // Ascending by age: b(20), a(30), c(40).
    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("b"), Value::Int(20)],
            vec![Value::from("a"), Value::Int(30)],
            vec![Value::from("c"), Value::Int(40)],
        ]
    );
}

#[tokio::test]
#[ignore = "requires live Sail server on 127.0.0.1:50051"]
async fn test_save_and_load_cypher_constraint_registry() {
    let store = store().await;
    let name = format!("test_{}", uuid::Uuid::new_v4().simple());

    assert_eq!(
        store
            .load_cypher_constraint_registry(&name)
            .await
            .expect("load missing registry"),
        None
    );

    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_cypher(
            "CREATE CONSTRAINT person_email \
             FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .expect("create registry constraint");
    store
        .save_cypher_constraint_registry(&name, &registry)
        .await
        .expect("save registry");

    let loaded = store
        .load_cypher_constraint_registry(&name)
        .await
        .expect("load registry")
        .expect("registry row");
    assert_eq!(loaded, registry);

    registry
        .apply_cypher("DROP CONSTRAINT person_email")
        .expect("drop registry constraint");
    store
        .save_cypher_constraint_registry(&name, &registry)
        .await
        .expect("overwrite registry");
    let loaded = store
        .load_cypher_constraint_registry(&name)
        .await
        .expect("reload registry")
        .expect("registry row");
    assert_eq!(loaded, registry);
}
