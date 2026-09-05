use super::*;

fn plan(cypher: &str) -> ScalarCountReadPushdown {
    plan_scalar_count_read(cypher, &CypherParameters::new(), &NoTypeHints)
        .unwrap()
        .unwrap_or_else(|| panic!("expected native scalar count: {cypher}"))
}

#[test]
fn renders_one_scalar_without_match_row_transport_or_pagination() {
    let count = plan("MATCH (n:Person) RETURN count(*) AS total SKIP $skip LIMIT $limit");
    assert_eq!(count.column_count(), 1);
    assert_eq!(
        count.to_sql(&SqliteDialect).unwrap(),
        "SELECT COUNT(*) FROM \"grust_nodes\" WHERE label = 'Person'"
    );
    // Existing row-source planner / Sail are not automatically opted in.
    let row_source = plan_read(
        "MATCH (n:Person) RETURN count(*) AS total",
        &CypherParameters::new(),
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        row_source.to_sql(&SparkDialect),
        "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person'"
    );
}

#[test]
fn renders_directed_and_shared_variable_sources() {
    assert_eq!(
        plan("MATCH (a:A)<-[:R]-(b:B)-[:S]->(c:C) RETURN count(*)")
            .to_sql(&SqliteDialect)
            .unwrap(),
        "SELECT COUNT(*) FROM \"grust_nodes\" n0 \
         JOIN \"grust_edges\" e0 ON e0.dst_id = n0.id \
         JOIN \"grust_nodes\" n1 ON n1.id = e0.src_id \
         JOIN \"grust_edges\" e1 ON e1.src_id = n1.id \
         JOIN \"grust_nodes\" n2 ON n2.id = e1.dst_id \
         WHERE e0.edge_type = 'R' AND e1.edge_type = 'S' \
         AND n0.label = 'A' AND n1.label = 'B' AND n2.label = 'C'"
    );
    assert_eq!(
        plan("MATCH (a:A)-[:R]->(b), (b)-[:S]->(a) RETURN count(*)")
            .to_sql(&SqliteDialect)
            .unwrap(),
        "SELECT COUNT(*) FROM \"grust_nodes\" n0, \"grust_nodes\" n1, \
         \"grust_edges\" e0, \"grust_edges\" e1 \
         WHERE e0.src_id = n0.id AND e0.dst_id = n1.id AND e0.edge_type = 'R' \
         AND e1.src_id = n1.id AND e1.dst_id = n0.id AND e1.edge_type = 'S' \
         AND n0.label = 'A'"
    );
    assert!(
        plan("MATCH (a)-[:R]-(b) RETURN count(*)")
            .to_sql(&SqliteDialect)
            .unwrap()
            .contains("ON (e0.src_id = n0.id OR e0.dst_id = n0.id)")
    );
}

#[test]
fn resolves_exact_string_filters_and_escapes_parameters() {
    let mut params = CypherParameters::new();
    params.insert("name".into(), Value::from("O'Hara\\雪"));
    let count = plan_scalar_count_read(
        "MATCH (a:Person {name: $name})-[r:R {kind:'reply'}]->(b) WHERE b.kind = 'Comment' RETURN count(*)",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    let sql = count.to_sql(&SqliteDialect).unwrap();
    assert!(sql.starts_with("SELECT COUNT(*) FROM "), "{sql}");
    assert!(
        sql.contains("json_type(n0.props, '$.name') = 'text' AND json_extract(n0.props, '$.name') COLLATE BINARY = 'O''Hara\\雪'"),
        "{sql}"
    );
    assert!(
        sql.contains("json_type(n1.props, '$.kind') = 'text'")
            && sql.contains("json_type(e0.props, '$.kind') = 'text'"),
        "{sql}"
    );
}

#[test]
fn unproven_filters_do_not_inherit_row_source_coercion() {
    for source in [
        "MATCH (n {label:'Person'}) RETURN count(*)",
        "MATCH (n) WHERE n.label = 'Person' RETURN count(*)",
        "MATCH (a)-[:R]->(b {label:'Person'}) RETURN count(*)",
        "MATCH (a), (b {label:'Person'}) RETURN count(*)",
        "MATCH (n {age:7}) RETURN count(*)",
        "MATCH (n) WHERE n.age = 7 RETURN count(*)",
        "MATCH (n) WHERE n.age = 1.5 RETURN count(*)",
        "MATCH (n) WHERE n.active = true RETURN count(*)",
        "MATCH (n) WHERE n.kind <> 'Comment' RETURN count(*)",
        "MATCH (n) WHERE n.kind IN ['Comment'] RETURN count(*)",
        "MATCH (n) WHERE n.kind IS NULL RETURN count(*)",
        "MATCH (n) WHERE n.kind = 'Comment' OR n.kind = 'Post' RETURN count(*)",
        "MATCH (n) WHERE NOT n.kind = 'Comment' RETURN count(*)",
    ] {
        let source = plan_read(source, &CypherParameters::new(), &NoTypeHints)
            .unwrap()
            .unwrap();
        assert!(source.scalar_count_read().is_none());
    }
}

#[test]
fn exact_predicates_require_dialect_opt_in_without_changing_sail_sql() {
    let query = "MATCH (n:Person {kind:'Comment'}) RETURN count(*)";
    let source = plan_read(query, &CypherParameters::new(), &NoTypeHints)
        .unwrap()
        .unwrap();
    assert_eq!(
        source.to_sql(&SparkDialect),
        "SELECT id, label, props FROM `grust_nodes` WHERE label = 'Person' AND GET_JSON_OBJECT(props, '$.kind') = 'Comment'"
    );
    let scalar = source.scalar_count_read().unwrap();
    assert!(!scalar.supported_by(&SparkDialect));
    assert!(scalar.to_sql(&SparkDialect).is_err());
    assert!(scalar.supported_by(&SqliteDialect));
    assert!(
        scalar
            .to_sql(&SqliteDialect)
            .unwrap()
            .contains("json_type(props, '$.kind') = 'text'")
    );
    let unfiltered = plan("MATCH (n:Person) RETURN count(*)");
    assert!(unfiltered.supported_by(&SparkDialect));
    assert_eq!(
        unfiltered.to_sql(&SparkDialect).unwrap(),
        "SELECT COUNT(*) FROM `grust_nodes` WHERE label = 'Person'"
    );

    let params = [("value".into(), Value::from("contains\0nul"))]
        .into_iter()
        .collect();
    let scalar = plan_scalar_count_read(
        "MATCH (n {kind:$value}) RETURN count(*)",
        &params,
        &NoTypeHints,
    )
    .unwrap()
    .unwrap();
    assert!(!scalar.supported_by(&SqliteDialect));
    assert!(scalar.to_sql(&SqliteDialect).is_err());
}

#[test]
fn refuses_unproven_aggregate_or_source_shapes() {
    for query in [
        "MATCH (n) RETURN count(n)",
        "MATCH (n) RETURN count(DISTINCT n)",
        "MATCH (n) RETURN DISTINCT count(*)",
        "MATCH (n) RETURN count(*) + 1",
        "MATCH (n) RETURN n.label, count(*)",
        "MATCH (n) RETURN count(*), count(*)",
        "MATCH (n) RETURN count(*) AS c ORDER BY c",
        "MATCH (n) WITH n RETURN count(*)",
        "MATCH (a) MATCH (b) RETURN count(*)",
        "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        "OPTIONAL MATCH (a) RETURN count(*)",
        "MATCH (a)-[:R*1..2]->(b) RETURN count(*)",
        "MATCH p = (a)-[:R]->(b) RETURN count(*)",
        "MATCH (a)-[:R]->(b) WHERE a.x = b.x RETURN count(*)",
        "MATCH (a) RETURN count(*) UNION MATCH (b) RETURN count(*)",
        "MATCH (a)-[:R]->(b), (a)-[:S]-(c) RETURN count(*)",
    ] {
        assert!(
            plan_scalar_count_read(query, &CypherParameters::new(), &NoTypeHints)
                .unwrap_or_else(|error| panic!("invalid test query {query}: {error}"))
                .is_none(),
            "unexpected scalar plan: {query}"
        );
    }
}

#[test]
fn requires_disjoint_relationship_types_across_positions() {
    for query in [
        "MATCH (a)-[:R]->(b)-[:R]->(c) RETURN count(*)",
        "MATCH (a)-[:R|S]->(b)-[:S|T]->(c) RETURN count(*)",
        "MATCH (a)-[]->(b)-[:S]->(c) RETURN count(*)",
        "MATCH (a)-[:R]->(b), (a)-[:R]->(c) RETURN count(*)",
        "MATCH (a)-[:R]->(b), (b)-[]->(c) RETURN count(*)",
    ] {
        assert!(
            plan_scalar_count_read(query, &CypherParameters::new(), &NoTypeHints)
                .unwrap()
                .is_none(),
            "unproven relationship reuse: {query}"
        );
    }
    plan("MATCH (a)-[:R|S]->(b)-[:T|U]->(c) RETURN count(*)");
    plan("MATCH (a)-[]->(b) RETURN count(*)");
    plan("MATCH (a)-[:R]->(a) RETURN count(*)");
    plan("MATCH (), () RETURN count(*)");
}

#[test]
fn decodes_zero_and_maximum_counts_and_preserves_aliases() {
    for (query, column) in [
        ("MATCH (n) RETURN count(*)", "expr"),
        ("MATCH (n) RETURN COUNT(*) AS `total count`", "total count"),
    ] {
        for count in [0, 7, i64::MAX] {
            let result = plan(query)
                .project_text_rows(
                    vec![vec![Some(count.to_string())]],
                    &CypherParameters::new(),
                )
                .unwrap();
            assert_eq!(result.columns, vec![column]);
            assert_eq!(result.rows, vec![vec![Value::Int(count)]]);
        }
    }
}

#[test]
fn rejects_malformed_count_results() {
    for rows in [
        vec![],
        vec![vec![]],
        vec![vec![None]],
        vec![vec![Some("-1".into())]],
        vec![vec![Some("9223372036854775808".into())]],
        vec![vec![Some("1.0".into())]],
        vec![vec![Some("not a number".into())]],
        vec![vec![Some("1".into()), Some("2".into())]],
        vec![vec![Some("1".into())], vec![Some("2".into())]],
    ] {
        assert!(
            plan("MATCH (n) RETURN count(*)")
                .project_text_rows(rows, &CypherParameters::new())
                .is_err()
        );
    }
}

#[test]
fn final_pagination_and_errors_match_reference() {
    let graph = grust_core::Graph::new(vec![Node::new("N", "n", grust_core::Props::new())], vec![]);
    let mut params = CypherParameters::new();
    params.insert("skip".into(), Value::Int(1));
    params.insert("limit".into(), Value::Int(0));
    for suffix in [
        "",
        " SKIP 0 LIMIT 1",
        " SKIP 1",
        " LIMIT 0",
        " SKIP $skip LIMIT $limit",
        " SKIP 2 - 1 LIMIT 1 + 1",
        " SKIP -1",
        " LIMIT -1",
        " SKIP null",
        " LIMIT 'bad'",
        " SKIP $missing",
        " LIMIT $missing",
        " SKIP 1 LIMIT $missing",
    ] {
        let query = format!("MATCH (n) RETURN count(*) AS c{suffix}");
        let expected = crate::read::run_read_query(&graph, &query, &params);
        let actual = plan_scalar_count_read(&query, &params, &NoTypeHints)
            .unwrap()
            .unwrap()
            .project_text_rows(vec![vec![Some("1".into())]], &params);
        match (actual, expected) {
            (Ok(actual), Ok(expected)) => assert_eq!(actual, expected, "{query}"),
            (Err(actual), Err(expected)) => {
                assert_eq!(actual.to_string(), expected.to_string(), "{query}")
            }
            other => panic!("pagination mismatch for {query}: {other:?}"),
        }
    }
}
