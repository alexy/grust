//! returning1 tests (split verbatim from the former monolithic tests.rs).
use super::*;

#[test]
fn cypher_returning_projects_row_producing_paths_on_memory_facade() {
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
fn cypher_returning_projects_matched_relationship_paths_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'matched-path-ada', status: 'matched-path'});
                CREATE (:Person {id: 'matched-path-bob', status: 'matched-path'});
                CREATE (:Team {id: 'matched-path-team'});
                MATCH (n:Person {status: 'matched-path'}), (t:Team {id: 'matched-path-team'})
                CREATE (n)-[:MEMBER_OF {source: 'matched-path'}]->(t);
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'matched-path'}]->(t:Team)
                SET r.checked = true
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       n.id AS person,
                       t.id AS team,
                       r.checked AS checked
                ORDER BY person;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("matched relationship path RETURN");

    assert_eq!(
        result.table.columns,
        vec![
            "p".to_string(),
            "hops".to_string(),
            "path_nodes".to_string(),
            "path_relationships".to_string(),
            "person".to_string(),
            "team".to_string(),
            "checked".to_string()
        ]
    );
    assert_eq!(result.table.rows.len(), 2);
    assert_eq!(result.table.rows[0][4], Value::from("matched-path-ada"));
    assert_eq!(result.table.rows[1][4], Value::from("matched-path-bob"));
    for row in &result.table.rows {
        let Value::Json(path) = &row[0] else {
            panic!("RETURN p should project a JSON path");
        };
        assert_eq!(row[1], Value::Int(1));
        assert_eq!(row[5], Value::from("matched-path-team"));
        assert_eq!(row[6], Value::Bool(true));
        assert_eq!(path["nodes"][0]["id"], row[4].to_json());
        assert_eq!(path["nodes"][1]["id"], row[5].to_json());
        assert_eq!(path["relationships"][0]["from"], row[4].to_json());
        assert_eq!(path["relationships"][0]["to"], row[5].to_json());
        assert_eq!(
            path["relationships"][0]["label"],
            serde_json::json!("MEMBER_OF")
        );
        assert_eq!(
            path["relationships"][0]["props"]["checked"],
            serde_json::json!({"type": "bool", "value": true})
        );
        assert_eq!(row[2].to_json(), path["nodes"]);
        assert_eq!(row[3].to_json(), path["relationships"]);
    }

    let removed =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'matched-path'}]->(t:Team)
                REMOVE r.checked
                RETURN p, r.checked AS checked, n.id AS person
                ORDER BY n.id
                LIMIT 1;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("matched relationship REMOVE path RETURN");

    assert_eq!(removed.table.rows[0][1], Value::Null);
    let Value::Json(path) = &removed.table.rows[0][0] else {
        panic!("RETURN p after REMOVE should project a JSON path");
    };
    assert!(
        path["relationships"][0]["props"].get("checked").is_none(),
        "removed relationship property should be absent in path JSON"
    );
}

#[test]
fn cypher_returning_projects_deleted_relationship_paths_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'deleted-path-ada', status: 'deleted-path'});
                CREATE (:Person {id: 'deleted-path-bob', status: 'deleted-path'});
                CREATE (:Team {id: 'deleted-path-team'});
                MATCH (n:Person {status: 'deleted-path'}), (t:Team {id: 'deleted-path-team'})
                CREATE (n)-[:MEMBER_OF {source: 'deleted-path'}]->(t);
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'deleted-path'}]->(t:Team)
                DELETE r
                RETURN p,
                       length(p) AS hops,
                       nodes(p) AS path_nodes,
                       relationships(p) AS path_relationships,
                       n.id AS person,
                       t.id AS team,
                       r.source AS source
                ORDER BY person;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("deleted relationship path RETURN");

    assert_eq!(
        result.table.columns,
        vec![
            "p".to_string(),
            "hops".to_string(),
            "path_nodes".to_string(),
            "path_relationships".to_string(),
            "person".to_string(),
            "team".to_string(),
            "source".to_string()
        ]
    );
    assert_eq!(result.table.rows.len(), 2);
    assert_eq!(result.mutation.report.edge_deletes, 2);
    assert_eq!(result.table.rows[0][4], Value::from("deleted-path-ada"));
    assert_eq!(result.table.rows[1][4], Value::from("deleted-path-bob"));
    for row in &result.table.rows {
        let Value::Json(path) = &row[0] else {
            panic!("RETURN p should project the deleted relationship as a JSON path");
        };
        assert_eq!(row[1], Value::Int(1));
        assert_eq!(row[5], Value::from("deleted-path-team"));
        assert_eq!(row[6], Value::from("deleted-path"));
        assert_eq!(path["nodes"][0]["id"], row[4].to_json());
        assert_eq!(path["nodes"][1]["id"], row[5].to_json());
        assert_eq!(path["relationships"][0]["from"], row[4].to_json());
        assert_eq!(path["relationships"][0]["to"], row[5].to_json());
        assert_eq!(
            path["relationships"][0]["label"],
            serde_json::json!("MEMBER_OF")
        );
        assert_eq!(row[2].to_json(), path["nodes"]);
        assert_eq!(row[3].to_json(), path["relationships"]);
    }

    let remaining = futures_executor::block_on(store.get_edges(EdgeQuery {
        from: None,
        to: None,
        label: Some(Label::new("MEMBER_OF")),
    }))
    .expect("remaining relationship scan");
    assert!(
        remaining
            .iter()
            .all(|edge| edge.props.get("source") != Some(&Value::from("deleted-path"))),
        "MATCH DELETE should remove the relationships whose paths were returned"
    );

    let endpoint_delete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'deleted-path-endpoint-a'});
                CREATE (:Team {id: 'deleted-path-endpoint-t'});
                CREATE (:Person {id: 'deleted-path-endpoint-a'})-[:MEMBER_OF {source: 'deleted-path-endpoint'}]->(:Team {id: 'deleted-path-endpoint-t'});
                MATCH p = (n:Person)-[r:MEMBER_OF {source: 'deleted-path-endpoint'}]->(t:Team)
                DELETE r, n
                RETURN p, n.id AS person, t.id AS team, r.source AS source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("endpoint-deleting relationship path RETURN");
    assert_eq!(endpoint_delete.table.rows.len(), 1);
    assert_eq!(
        endpoint_delete.table.rows[0][1],
        Value::from("deleted-path-endpoint-a")
    );
    assert_eq!(
        endpoint_delete.table.rows[0][2],
        Value::from("deleted-path-endpoint-t")
    );
    assert_eq!(
        endpoint_delete.table.rows[0][3],
        Value::from("deleted-path-endpoint")
    );
    let Value::Json(endpoint_path) = &endpoint_delete.table.rows[0][0] else {
        panic!("RETURN p should project the endpoint-deleting relationship as a JSON path");
    };
    assert_eq!(
        endpoint_path["nodes"][0]["id"],
        serde_json::json!("deleted-path-endpoint-a")
    );
    assert_eq!(
        endpoint_path["nodes"][1]["id"],
        serde_json::json!("deleted-path-endpoint-t")
    );
    assert_eq!(
        endpoint_path["relationships"][0]["from"],
        serde_json::json!("deleted-path-endpoint-a")
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("deleted-path-endpoint-a")))
            .expect("deleted endpoint lookup")
            .is_none(),
        "DELETE r, n should remove the endpoint node after snapshotting the path"
    );

    let resolved_endpoint_delete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'deleted-path-resolved-a'});
                CREATE (:Team {id: 'deleted-path-resolved-t'});
                CREATE (:Person {id: 'deleted-path-resolved-a'})-[:MEMBER_OF {source: 'deleted-path-resolved'}]->(:Team {id: 'deleted-path-resolved-t'});
                MATCH p = (n:Person {id: 'deleted-path-resolved-a'})-[r:MEMBER_OF {source: 'deleted-path-resolved'}]->(t:Team {id: 'deleted-path-resolved-t'})
                DELETE r, n
                RETURN p, n.id AS person, t.id AS team, r.source AS source;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("resolved endpoint-deleting relationship path RETURN");
    assert_eq!(resolved_endpoint_delete.table.rows.len(), 1);
    assert_eq!(
        resolved_endpoint_delete.table.rows[0][1],
        Value::from("deleted-path-resolved-a")
    );
    assert_eq!(
        resolved_endpoint_delete.table.rows[0][2],
        Value::from("deleted-path-resolved-t")
    );
    assert_eq!(
        resolved_endpoint_delete.table.rows[0][3],
        Value::from("deleted-path-resolved")
    );
    let Value::Json(resolved_path) = &resolved_endpoint_delete.table.rows[0][0] else {
        panic!("RETURN p should project the resolved endpoint-deleting path");
    };
    assert_eq!(
        resolved_path["nodes"][0]["id"],
        serde_json::json!("deleted-path-resolved-a")
    );
    assert_eq!(
        resolved_path["nodes"][1]["id"],
        serde_json::json!("deleted-path-resolved-t")
    );
    assert!(
        futures_executor::block_on(store.get_node(&NodeId::new("deleted-path-resolved-a")))
            .expect("resolved deleted endpoint lookup")
            .is_none(),
        "resolved endpoint delete should remove the endpoint node after snapshotting the path"
    );

    let node_path_delete_err = execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
                CREATE (:Person {id: 'deleted-node-path'});
                MATCH p = (n:Person {id: 'deleted-node-path'})
                DELETE n
                RETURN p;
                ",
        CypherMutationOptions::default(),
    );
    let node_path_delete_err = futures_executor::block_on(node_path_delete_err)
        .expect_err("node DELETE path variables should stay rejected");
    assert!(
        matches!(node_path_delete_err, GrustError::CypherSyntax(_)),
        "{node_path_delete_err:?}"
    );
}

#[test]
fn cypher_returning_projects_restricted_maps_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (n:Person {id: 'map-ada', name: 'Ada'})
                RETURN n {
                    .id,
                    .label,
                    display: n.name,
                    lower: toLower(n.name),
                    upper: toUpper(n.name),
                    name_size: size(n.name),
                    nickname: coalesce(n.nickname, n.name, 'unknown'),
                    marker: 'seen',
                    rank: 1,
                    active: true,
                    fallback: $fallback,
                    empty: null,
                    .missing
                } AS person;
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
            "lower": "ada",
            "upper": "ADA",
            "name_size": 3,
            "nickname": "Ada",
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
                RETURN count(n { team: toLower(n.team), marker: 'seen' }) AS mapped_rows,
                       count(DISTINCT n { team: toLower(n.team), marker: 'seen' }) AS distinct_maps,
                       collect(n { .id, kind: 'person', team: toUpper(n.team), team_size: size(n.team) }) AS people;
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
        matches!(
            error,
            GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
        ),
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
fn cypher_returning_projects_restricted_lists_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (n:Person {id: 'list-ada', name: 'Ada'})
                RETURN [
                    n.id,
                    n.label,
                    n.name,
                    toLower(n.name),
                    toUpper(n.name),
                    size(n.name),
                    coalesce(n.nickname, n.name, 'unknown'),
                    'seen',
                    1,
                    true,
                    null,
                    $marker,
                    n.missing
                ] AS person;
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
            "list-ada", "Person", "Ada", "ada", "ADA", 3, "Ada", "seen", 1, true, null, "param",
            null
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
                RETURN count([toLower(n.team), 'seen']) AS listed_rows,
                       count(DISTINCT [toLower(n.team), 'seen']) AS distinct_lists,
                       collect([toUpper(n.id), 'team', size(n.team)]) AS people;
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
        matches!(
            error,
            GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
        ),
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
fn cypher_returning_projects_restricted_literals_on_memory_facade() {
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
fn cypher_returning_projects_restricted_coalesce_on_memory_facade() {
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

    let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'coalesce-nested-ada', name: 'Ada', status: 'coalesce-nested'});
                CREATE (:Person {id: 'coalesce-nested-bob', nickname: 'B', status: 'coalesce-nested'});
                MATCH (n:Person {status: 'coalesce-nested'}) SET n.seen = true
                RETURN n.id AS id,
                       coalesce(toLower(n.nickname), toUpper(n.name), 'unknown') AS display,
                       coalesce(size(n.nickname), size(n.name), 0) AS name_size
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted coalesce projections should execute");
    assert_eq!(
        nested.table.rows,
        vec![
            vec![
                Value::from("coalesce-nested-ada"),
                Value::from("ADA"),
                Value::Int(3)
            ],
            vec![
                Value::from("coalesce-nested-bob"),
                Value::from("b"),
                Value::Int(1)
            ],
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

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'coalesce-nested'}) SET n.nested_coalesced = true
                RETURN count(coalesce(toLower(n.nickname), toUpper(n.name))) AS named,
                       collect(coalesce(toLower(n.nickname), toUpper(n.name))) AS names;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested coalesce aggregate projections should execute");
    assert_eq!(nested_aggregates.table.rows[0][0], Value::Int(2));
    assert_eq!(
        nested_aggregates.table.rows[0][1],
        Value::Json(serde_json::json!(["ADA", "b"]))
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

    let nested_list =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'coalesce-list', name: 'Ada'}) RETURN coalesce([n.name], 'unknown');",
                CypherMutationOptions::default(),
            ))
            .expect_err("coalesce arguments should reject nested list composites");
    assert!(
        matches!(nested_list, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_list:?}"
    );

    let nested_map =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "CREATE (n:Person {id: 'coalesce-map', name: 'Ada'}) RETURN coalesce(n { name: n.name }, 'unknown');",
                CypherMutationOptions::default(),
            ))
            .expect_err("coalesce arguments should reject nested map composites");
    assert!(
        matches!(nested_map, GrustError::CypherUnsupportedCardinality(_)),
        "{nested_map:?}"
    );

    let path_function_on_node =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "CREATE (n:Person {id: 'coalesce-nested'}) RETURN coalesce(length(n), 'unknown');",
            CypherMutationOptions::default(),
        ))
        .expect_err("nested path functions should still require path variables");
    assert!(
        matches!(
            path_function_on_node,
            GrustError::CypherUnsupportedCardinality(_)
        ),
        "{path_function_on_node:?}"
    );
}

#[test]
fn cypher_returning_projects_restricted_element_functions_on_memory_facade() {
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
fn cypher_returning_projects_restricted_properties_and_keys_on_memory_facade() {
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
fn cypher_returning_projects_relationship_endpoints_on_memory_facade() {
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
fn cypher_returning_projects_restricted_identity_functions_on_memory_facade() {
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
fn cypher_returning_projects_restricted_exists_on_memory_facade() {
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
fn cypher_returning_projects_restricted_size_on_memory_facade() {
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
fn cypher_returning_projects_restricted_list_slices_on_memory_facade() {
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
            .expect("nested list slice bounds should execute");
    assert_eq!(
        nested_bound.table.rows,
        vec![vec![Value::IntArray(vec![1])]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'slice'}) SET n.nested_slice_counted = true
                RETURN count(n.scores[0..head(n.scores)]) AS rows,
                       collect(n.scores[0..head(n.scores)]) AS slices;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested list slice aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([[3, 5, 8], [7, 9, 13]])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_list_membership_on_memory_facade() {
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
fn cypher_returning_projects_restricted_list_predicates_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let concrete =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (a:Person {id: 'predicate-list-ada', tags: $tags, scores: $scores, marker: 'SPEAKER'});
                CREATE (b:Person {id: 'predicate-list-bob'});
                MATCH (a:Person {id: 'predicate-list-ada'}), (b:Person {id: 'predicate-list-bob'})
                CREATE (a)-[e:KNOWS {id: 'predicate-list-knows', weights: $weights}]->(b)
                RETURN any(t IN a.tags WHERE t = 'speaker') AS any_speaker,
                       any(t IN a.tags WHERE t = toLower(a.marker)) AS nested_any_speaker,
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
                CREATE (:Person {id: 'predicate-list-cara', status: 'list-predicate', tags: $tags_a, marker: 'SPEAKER'});
                CREATE (:Person {id: 'predicate-list-dan', status: 'list-predicate', tags: $tags_b, marker: 'SPEAKER'});
                MATCH (n:Person {status: 'list-predicate'}) SET n.predicate_checked = true
                RETURN n.id AS id,
                       any(t IN n.tags WHERE t = 'speaker') AS any_speaker,
                       any(t IN n.tags WHERE t = toLower(n.marker)) AS nested_any_speaker
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
            vec![
                Value::from("predicate-list-cara"),
                Value::Bool(true),
                Value::Bool(true)
            ],
            vec![
                Value::from("predicate-list-dan"),
                Value::Bool(false),
                Value::Bool(false)
            ],
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
                       collect(any(t IN n.tags WHERE t = 'speaker')) AS predicates,
                       collect(any(t IN n.tags WHERE t = toLower(n.marker))) AS nested_predicates;
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
fn cypher_returning_projects_restricted_list_indexes_on_memory_facade() {
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
                CREATE (:Person {id: 'index-cara', status: 'index', scores: $scores_a, indexes: $indexes_a});
                CREATE (:Person {id: 'index-dan', status: 'index', scores: $scores_b, indexes: $indexes_b});
                MATCH (n:Person {status: 'index'}) SET n.indexed = true
                RETURN n.id AS id, n.scores[0] AS score
                ORDER BY id;
                ",
                CypherMutationOptions {
                    parameters: CypherParameters::from([
                        ("scores_a".to_string(), Value::IntArray(vec![3, 5])),
                        ("scores_b".to_string(), Value::IntArray(vec![7, 9])),
                        ("indexes_a".to_string(), Value::IntArray(vec![0])),
                        ("indexes_b".to_string(), Value::IntArray(vec![0])),
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
                        Value::IntArray(vec![0, 7]),
                    )]),
                    ..CypherMutationOptions::default()
                },
            ))
            .expect("nested list index expressions should execute");
    assert_eq!(nested_index.table.rows, vec![vec![Value::Int(0)]]);

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'index'}) SET n.nested_index_counted = true
                RETURN count(n.scores[head(n.indexes)]) AS rows,
                       sum(n.scores[head(n.indexes)]) AS total_scores,
                       collect(n.scores[head(n.indexes)]) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested list index aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(10),
            Value::Json(serde_json::json!([3, 7])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_list_elements_on_memory_facade() {
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
        .expect("nested head arguments should execute");
    assert_eq!(nested_head.table.rows, vec![vec![Value::from("a")]]);

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'list'}) SET n.nested_head_counted = true
                RETURN count(head(tail(n.scores))) AS rows,
                       sum(head(tail(n.scores))) AS total_scores,
                       collect(head(tail(n.scores))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested head aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(14),
            Value::Json(serde_json::json!([5, 9])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_list_tail_on_memory_facade() {
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
        .expect("nested tail arguments should execute");
    assert_eq!(
        nested_tail.table.rows,
        vec![vec![Value::StringArray(vec!["b".to_string()])]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'tail-path-a', status: 'tail-path', path: 'a/b'});
                CREATE (:Person {id: 'tail-path-b', status: 'tail-path', path: 'c/d/e'});
                MATCH (n:Person {status: 'tail-path'}) SET n.nested_tail_counted = true
                RETURN count(tail(split(n.path, '/'))) AS rows,
                       collect(tail(split(n.path, '/'))) AS tails;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested tail aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([["b"], ["d", "e"]])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_is_empty_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'empty-nested', name: '', tags: $tags})
                RETURN isEmpty(toLower(n.name)) AS empty_lower,
                       isEmpty(coalesce(n.nickname, '')) AS fallback_empty,
                       isEmpty(range(1, 0)) AS empty_range,
                       isEmpty('static') AS literal_empty;
                ",
            CypherMutationOptions {
                parameters: CypherParameters::from([(
                    "tags".to_string(),
                    Value::StringArray(Vec::new()),
                )]),
                ..CypherMutationOptions::default()
            },
        ))
        .expect("nested isEmpty arguments should execute");
    assert_eq!(
        nested_empty.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(false),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'empty'}) SET n.nested_empty_counted = true
                RETURN count(isEmpty(toLower(n.nickname))) AS rows,
                       collect(isEmpty(coalesce(n.nickname, ''))) AS states;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested isEmpty aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_to_string_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'to-string-nested', name: 'Ada'})
                RETURN toString(toLower(n.name)) AS lowered,
                       toString(coalesce(n.nickname, 'Fallback')) AS fallback,
                       toString(42) AS literal_number;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested toString arguments should execute");
    assert_eq!(
        nested_to_string.table.rows,
        vec![vec![
            Value::from("ada"),
            Value::from("Fallback"),
            Value::from("42"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'to-string'}) SET n.nested_to_string_counted = true
                RETURN count(toString(toLower(n.id))) AS rows,
                       collect(toString(coalesce(n.nickname, n.score))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested toString aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["7", "11"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_abs_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'abs-nested', score: -3, nickname: 'Ada'})
                RETURN abs(abs(n.score)) AS nested_abs,
                       abs(size(n.nickname)) AS nickname_size,
                       abs(-42) AS literal_abs;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested abs arguments should execute");
    assert_eq!(
        nested_abs.table.rows,
        vec![vec![Value::Int(3), Value::Int(3), Value::Int(42)]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'abs'}) SET n.nested_abs_counted = true
                RETURN count(abs(abs(n.score))) AS rows,
                       sum(abs(abs(n.score))) AS total_scores,
                       collect(abs(abs(n.score))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested abs aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(18),
            Value::Json(serde_json::json!([7, 11])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_numeric_rounds_on_memory_facade() {
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

    let nested_rounds =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (n:Person {id: 'round-nested', score: -3.2, nickname: 'Ada'})
                RETURN ceil(abs(n.score)) AS debt_ceiling,
                       floor(abs(n.score)) AS debt_floor,
                       ceil(size(n.nickname)) AS nickname_ceiling,
                       floor(2.9) AS literal_floor;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric rounding projections should execute");
    assert_eq!(
        nested_rounds.table.rows,
        vec![vec![
            Value::Float(4.0),
            Value::Float(3.0),
            Value::Int(3),
            Value::Float(2.0),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'round'}) SET n.nested_round_counted = true
                RETURN count(ceil(abs(n.score))) AS rows,
                       sum(ceil(abs(n.score))) AS total_scores,
                       collect(ceil(abs(n.score))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric rounding aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Float(20.0),
            Value::Json(serde_json::json!([8.0, 12.0])),
        ]]
    );
}
