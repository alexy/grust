use super::*;

fn config() -> SurrealConfig {
    SurrealConfig {
        relationships: vec!["presents".into(), "member_of".into()],
        ..SurrealConfig::default()
    }
}

#[test]
fn unfiltered_edge_read_preserves_the_full_relation_scan() {
    assert_eq!(
        surreal_get_edges_query(&EdgeQuery::default(), &config()).unwrap(),
        "SELECT *, meta::tb(id) AS __grust_label FROM `member_of`, `presents`;"
    );
}

#[test]
fn edge_read_pushes_from_and_to_independently() {
    for (from, to, expected) in [
        (Some("person-1"), None, "meta::id(in) = \"person-1\""),
        (None, Some("talk-1"), "meta::id(out) = \"talk-1\""),
        (
            Some("person-1"),
            Some("talk-1"),
            "meta::id(in) = \"person-1\" AND meta::id(out) = \"talk-1\"",
        ),
    ] {
        let query = EdgeQuery {
            from: from.map(NodeId::new),
            to: to.map(NodeId::new),
            label: None,
        };
        assert_eq!(
            surreal_get_edges_query(&query, &config()).unwrap(),
            format!(
                "SELECT *, meta::tb(id) AS __grust_label FROM `member_of`, `presents` WHERE {expected};"
            )
        );
    }
}

#[test]
fn edge_read_combines_endpoint_filters_with_an_explicit_relationship_table() {
    let query = EdgeQuery {
        from: Some(NodeId::new("person-1")),
        to: Some(NodeId::new("talk-1")),
        label: Some(Label::new("member-of")),
    };
    assert_eq!(
        surreal_get_edges_query(&query, &SurrealConfig::default()).unwrap(),
        "SELECT *, meta::tb(id) AS __grust_label FROM `member_of` WHERE meta::id(in) = \"person-1\" AND meta::id(out) = \"talk-1\";"
    );
}

#[test]
fn endpoint_filter_keeps_the_entire_logical_key_and_ignores_label_guesses() {
    let query = EdgeQuery {
        from: Some(NodeId::new("DifferentPrefix:person:1")),
        to: Some(NodeId::new("talk-without-prefix")),
        label: Some(Label::new("presents")),
    };
    let expected = "SELECT *, meta::tb(id) AS __grust_label FROM `presents` WHERE meta::id(in) = \"DifferentPrefix:person:1\" AND meta::id(out) = \"talk-without-prefix\";";
    for labels in [
        vec![],
        vec!["Person".into()],
        vec!["Talk".into(), "Person".into()],
    ] {
        let config = SurrealConfig {
            labels,
            ..SurrealConfig::default()
        };
        assert_eq!(surreal_get_edges_query(&query, &config).unwrap(), expected);
    }
}

#[test]
fn endpoint_predicate_values_are_escaped_data_even_with_query_punctuation() {
    // Include delimiters, comment markers, a backslash, Unicode, and control
    // characters. These are record keys, never identifier or query fragments.
    let id = "Person:O'Reilly\"; DELETE record; -- `雪`\\line\n\t";
    let expected_literal = "\"Person:O'Reilly\\\"; DELETE record; -- `雪`\\\\line\\n\\t\"";
    for from in [true, false] {
        let query = EdgeQuery {
            from: from.then(|| NodeId::new(id)),
            to: (!from).then(|| NodeId::new(id)),
            label: Some(Label::new("presents")),
        };
        let endpoint = if from { "in" } else { "out" };
        let rendered = surreal_get_edges_query(&query, &SurrealConfig::default()).unwrap();
        assert_eq!(
            rendered,
            format!(
                "SELECT *, meta::tb(id) AS __grust_label FROM `presents` WHERE meta::id({endpoint}) = {expected_literal};"
            )
        );
        assert_eq!(
            serde_json::from_str::<String>(expected_literal).unwrap(),
            id
        );
    }
}

#[test]
fn endpoint_literals_round_trip_through_the_surrealql_parser() {
    for id in [
        "Person:1",
        "",
        "x\"; RETURN true; -- \\ `雪`\n",
        "p:more:colons",
    ] {
        // The SDK exposes an inert-value parser, so verify literal escaping
        // against SurrealQL itself without a running database.
        assert_eq!(
            surrealdb::parse::value(&surreal_string(id)).unwrap(),
            surrealdb::types::Value::String(id.to_string())
        );
    }
}

#[test]
fn missing_endpoint_filters_do_not_bypass_relationship_validation() {
    for query in [
        EdgeQuery::default(),
        EdgeQuery {
            from: Some(NodeId::new("missing")),
            ..EdgeQuery::default()
        },
        EdgeQuery {
            to: Some(NodeId::new("missing")),
            ..EdgeQuery::default()
        },
    ] {
        let error = surreal_get_edges_query(&query, &SurrealConfig::default()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("SurrealConfig.relationships is empty")
        );
    }
}

#[test]
fn decoded_edge_postfilters_keep_exact_endpoints_labels_and_multiplicity() {
    let fixtures = [
        ("presents", "person:1", "talk:1"),
        ("presents", "person:1", "talk:2"),
        ("presents", "person:2", "talk:1"),
        ("presents", "person:1", "person:1"),
        ("member_of", "person:1", "talk:1"),
        ("presents", "person:10", "talk:10"),
        ("presents", "person:1", "talk:1"),
    ];
    // Exercise both existing HTTP string-record and SDK object-record shapes.
    for sdk in [false, true] {
        let edges = fixtures
            .iter()
            .enumerate()
            .map(|(index, (label, from, to))| {
                let endpoint = |table: &str, key: &str| {
                    if sdk {
                        serde_json::json!({"tb": table, "id": {"String": key}})
                    } else {
                        serde_json::json!(format!("{table}:`{key}`"))
                    }
                };
                surreal_edge_from_value(serde_json::json!({
                    "id": format!("{label}:edge{index}"),
                    "__grust_label": label,
                    "relationship": label,
                    "in": endpoint("actual_from_table", from),
                    "out": endpoint("actual_to_table", to),
                    "edge_id": index.to_string(),
                    "source": "fixture"
                }))
                .unwrap()
            })
            .collect::<Vec<_>>();

        for (from, to, label, expected) in [
            (None, None, None, vec![0, 1, 2, 3, 4, 5, 6]),
            (Some("person:1"), None, None, vec![0, 1, 3, 4, 6]),
            (None, Some("talk:1"), None, vec![0, 2, 4, 6]),
            (Some("person:1"), Some("talk:1"), None, vec![0, 4, 6]),
            (
                Some("person:1"),
                Some("talk:1"),
                Some("presents"),
                vec![0, 6],
            ),
            (Some("person:1"), Some("person:1"), None, vec![3]),
            (Some("missing"), None, None, vec![]),
            (None, Some("missing"), None, vec![]),
        ] {
            let query = EdgeQuery {
                from: from.map(NodeId::new),
                to: to.map(NodeId::new),
                label: label.map(Label::new),
            };
            let mut actual = edges.clone();
            filter_edges(&mut actual, &query);
            assert_eq!(
                actual,
                expected
                    .into_iter()
                    .map(|i| edges[i].clone())
                    .collect::<Vec<_>>()
            );
            // Query rendering must accept the same logical IDs without needing
            // actual_from_table/actual_to_table in the configured node labels.
            assert!(surreal_get_edges_query(&query, &config()).is_ok());
        }
    }
}
