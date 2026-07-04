use crate::parser::parse_query;
use crate::*;

#[test]
fn parses_use_clause_and_reports_selection() {
    let query = parse_query("USE social MATCH (n:Person) RETURN n.id").expect("parse USE");
    assert_eq!(
        query_graph_selection(&query).expect("selection"),
        Some("social".to_string())
    );
}

#[test]
fn catalog_graph_selection_finds_named_graph() {
    let mut registry = CypherConstraintRegistry::new();
    registry
        .apply_statements(
            cypher_ddl("CREATE GRAPH TYPE social AS NODE Person (name STRING)")
                .expect("parse graph type"),
        )
        .expect("apply graph type");
    let catalog = registry.catalog_snapshot("social");

    let graph = ensure_catalog_graph_selection(&catalog, "social").expect("select social");
    assert_eq!(
        graph,
        NamedGraphCatalog {
            name: "social".to_string(),
            graph_type: Some("social".to_string()),
        }
    );
}

#[test]
fn read_executor_accepts_use_default_single_graph_fallback() {
    let graph = Graph::new(vec![Node::new("Person", "p1", Props::new())], vec![]);
    let table = crate::read::run_read_query(
        &graph,
        "USE default MATCH (n:Person) RETURN n.id",
        &CypherParameters::new(),
    )
    .expect("read query");
    assert_eq!(table.columns, vec!["n.id"]);
    assert_eq!(table.rows, vec![vec![Value::from("p1")]]);
}

#[test]
fn read_executor_rejects_wrong_single_graph_selection() {
    let graph = Graph::new(vec![Node::new("Person", "p1", Props::new())], vec![]);
    let error = crate::read::run_read_query(
        &graph,
        "USE social MATCH (n:Person) RETURN n.id",
        &CypherParameters::new(),
    )
    .expect_err("wrong graph should fail");
    assert!(matches!(error, GrustError::Unsupported(_)));
    assert!(error.to_string().contains("named-graph-selection"));
}

#[test]
fn named_graph_read_executor_accepts_matching_selection() {
    let graph = Graph::new(vec![Node::new("Person", "p1", Props::new())], vec![]);
    let table = crate::read::run_read_query_on_named_graph(
        &graph,
        "social",
        "USE social MATCH (n:Person) RETURN n.id",
        &CypherParameters::new(),
    )
    .expect("read query");
    assert_eq!(table.rows, vec![vec![Value::from("p1")]]);
}

#[test]
fn session_commands_parse_and_update_state() {
    let mut session = CypherSession::default();

    SessionCommand::parse("SET limit_rows = 25")
        .expect("parse SET")
        .expect("SET command")
        .apply(&mut session, None)
        .expect("apply SET");
    assert_eq!(
        session.settings.get("limit_rows"),
        Some(&Value::from(25i64))
    );

    SessionCommand::parse("USE social")
        .expect("parse USE")
        .expect("USE command")
        .apply(&mut session, None)
        .expect("apply USE");
    assert_eq!(session.current_graph, "social");

    SessionCommand::parse("RESET limit_rows")
        .expect("parse RESET")
        .expect("RESET command")
        .apply(&mut session, None)
        .expect("apply RESET");
    assert!(session.settings.is_empty());
}

#[test]
fn session_use_validates_catalog_when_provided() {
    let registry = CypherConstraintRegistry::new();
    let catalog = registry.catalog_snapshot("default");
    let mut session = CypherSession::default();

    let error = SessionCommand::parse("USE missing")
        .expect("parse USE")
        .expect("USE command")
        .apply(&mut session, Some(&catalog))
        .expect_err("missing graph");
    assert!(matches!(error, GrustError::Unsupported(_)));
    assert_eq!(session.current_graph, "default");

    SessionCommand::parse("USE default")
        .expect("parse USE")
        .expect("USE command")
        .apply(&mut session, Some(&catalog))
        .expect("known graph");
    assert_eq!(session.current_graph, "default");
}

#[test]
fn reset_all_clears_session_settings() {
    let mut session = CypherSession::default();
    SessionCommand::parse("SET a = true")
        .expect("parse SET")
        .expect("SET command")
        .apply(&mut session, None)
        .expect("apply SET");
    SessionCommand::parse("RESET ALL")
        .expect("parse RESET ALL")
        .expect("RESET ALL command")
        .apply(&mut session, None)
        .expect("apply RESET ALL");
    assert!(session.settings.is_empty());
}
