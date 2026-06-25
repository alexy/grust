//! returning2 tests (split verbatim from the former monolithic tests.rs).
use super::*;

#[test]
fn cypher_returning_projects_restricted_numeric_sign_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'sign-nested', score: -3, nickname: 'Ada'})
                RETURN sign(abs(n.score)) AS positive_sign,
                       sign(size(n.nickname)) AS nickname_sign,
                       sign(-42) AS literal_sign;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric sign projections should execute");
    assert_eq!(
        nested_sign.table.rows,
        vec![vec![Value::Int(1), Value::Int(1), Value::Int(-1)]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'sign'}) SET n.nested_sign_counted = true
                RETURN count(sign(abs(n.score))) AS rows,
                       sum(sign(abs(n.score))) AS total_scores,
                       collect(sign(abs(n.score))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric sign aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([1, 1])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_numeric_casts_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'cast-nested', score: -3, nickname: 'Ada'})
                RETURN toFloat(abs(n.score)) AS score_float,
                       toInteger(size(n.nickname)) AS nickname_size,
                       toInteger('42') AS literal_int;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric cast projections should execute");
    assert_eq!(
        nested_cast.table.rows,
        vec![vec![Value::Float(3.0), Value::Int(3), Value::Int(42)]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'cast'}) SET n.nested_cast_counted = true
                RETURN count(toFloat(abs(n.score))) AS rows,
                       sum(toFloat(abs(n.score))) AS total_scores,
                       collect(toFloat(abs(n.score))) AS scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested numeric cast aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Float(19.0),
            Value::Json(serde_json::json!([7.2, 11.8])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_boolean_cast_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'bool-nested', active: true, active_text: 'false'})
                RETURN toBoolean(toString(n.active)) AS active_string,
                       toBoolean(coalesce(n.missing, n.active_text)) AS fallback_text,
                       toBoolean('true') AS literal_bool;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested boolean cast projections should execute");
    assert_eq!(
        nested_cast.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(false),
            Value::Bool(true),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'bool'}) SET n.nested_bool_counted = true
                RETURN count(toBoolean(toString(n.active))) AS rows,
                       count(DISTINCT toBoolean(toString(n.active))) AS distinct_states,
                       collect(toBoolean(toString(n.active))) AS states;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested boolean cast aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Int(2),
            Value::Json(serde_json::json!([true, false])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_list_casts_on_memory_facade() {
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
            .expect("nested list cast arguments should execute");
    assert_eq!(
        nested_cast.table.rows,
        vec![vec![Value::StringArray(vec!["b".to_string()])]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'list-cast'}) SET n.nested_cast_counted = true
                RETURN count(toStringList(tail(n.scores))) AS rows,
                       collect(toStringList(tail(n.scores))) AS score_tails;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested list cast aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([["5"], ["9"]])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_transforms_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'string-nested', name: 'Ada'})
                RETURN toLower(coalesce(n.name, 'unknown')) AS lower_name,
                       toUpper(toLower(n.name)) AS nested_name,
                       toLower('STATIC') AS literal_name;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string transforms should execute");
    assert_eq!(
        nested_transform.table.rows,
        vec![vec![
            Value::from("ada"),
            Value::from("ADA"),
            Value::from("static"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'string'}) SET n.nested_string_counted = true
                RETURN count(toLower(coalesce(n.nickname, n.team))) AS rows,
                       collect(toUpper(toLower(n.team))) AS teams;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string transform aggregates should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["ENG", "OPS"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_trims_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'trim-nested', name: ' Ada '})
                RETURN trim(toLower(n.name)) AS trimmed_lower,
                       lTrim(coalesce(n.nickname, ' fallback ')) AS fallback,
                       rTrim(' static ') AS literal_trim;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string trims should execute");
    assert_eq!(
        nested_trim.table.rows,
        vec![vec![
            Value::from("ada"),
            Value::from("fallback "),
            Value::from(" static"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'trim'}) SET n.nested_trim_counted = true
                RETURN count(trim(toLower(n.team))) AS rows,
                       collect(rTrim(toUpper(n.team))) AS teams;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string trim aggregates should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([" ENG", " OPS"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_substring_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'substring-nested', name: 'Ada'})
                RETURN substring(toLower(n.name), 0, 1) AS lowered_initial,
                       substring(coalesce(n.nickname, 'Fallback'), 0, 4) AS fallback_prefix,
                       substring('static', 1, 3) AS literal_slice;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested substring arguments should execute");
    assert_eq!(
        nested_substring.table.rows,
        vec![vec![
            Value::from("a"),
            Value::from("Fall"),
            Value::from("tat"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'substring'}) SET n.nested_substring_counted = true
                RETURN count(substring(toLower(n.team), 0, 3)) AS rows,
                       collect(substring(toLower(n.team), 0, 3)) AS prefixes;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested substring aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["eng", "ope"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_replace_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'replace-nested', name: 'Ada'})
                RETURN replace(toLower(n.name), 'a', 'A') AS rewritten,
                       replace(coalesce(n.nickname, 'Fallback'), 'Fall', 'Call') AS fallback,
                       replace('static', 'sta', 'plas') AS literal_rewrite;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested replace arguments should execute");
    assert_eq!(
        nested_replace.table.rows,
        vec![vec![
            Value::from("AdA"),
            Value::from("Callback"),
            Value::from("plastic"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'replace'}) SET n.nested_replace_counted = true
                RETURN count(replace(toLower(n.team), '-team', '')) AS rows,
                       collect(replace(toLower(n.team), '-team', '')) AS teams;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested replace aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["eng", "ops"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_predicates_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'predicate-nested', name: 'Ada'})
                RETURN contains(toLower(n.name), 'a') AS contains_a,
                       startsWith(coalesce(n.nickname, 'Fallback'), 'Fall') AS fallback_starts,
                       endsWith('static', 'tic') AS literal_ends;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string predicate arguments should execute");
    assert_eq!(
        nested_predicate.table.rows,
        vec![vec![
            Value::Bool(true),
            Value::Bool(true),
            Value::Bool(true),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'predicate'}) SET n.nested_predicate_counted = true
                RETURN count(startsWith(toLower(n.team), 'eng')) AS rows,
                       collect(contains(toUpper(n.team), 'A')) AS states;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string predicate aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([false, true])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_slices_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'slice-nested', name: 'Ada'})
                RETURN left(toLower(n.name), 1) AS lowered_initial,
                       right(coalesce(n.nickname, 'Fallback'), 4) AS fallback_suffix,
                       left('static', 3) AS literal_prefix;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string slice arguments should execute");
    assert_eq!(
        nested_slice.table.rows,
        vec![vec![
            Value::from("a"),
            Value::from("back"),
            Value::from("sta"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'slice'}) SET n.nested_slice_counted = true
                RETURN count(left(toLower(n.team), 3)) AS rows,
                       collect(right(toUpper(n.team), 3)) AS suffixes;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string slice aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["ING", "ONS"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_reverse_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'reverse-nested', name: 'Ada'})
                RETURN reverse(toLower(n.name)) AS reversed_lower,
                       reverse(coalesce(n.nickname, n.name, 'unknown')) AS display,
                       reverse(range(1, 3)) AS reversed_range,
                       reverse('static') AS literal_reverse;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string reverse arguments should execute");
    assert_eq!(
        nested_reverse.table.rows,
        vec![vec![
            Value::from("ada"),
            Value::from("adA"),
            Value::IntArray(vec![3, 2, 1]),
            Value::from("citats"),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'reverse'}) SET n.nested_reverse_counted = true
                RETURN count(reverse(toLower(n.team))) AS rows,
                       collect(reverse(toUpper(n.team))) AS teams;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string reverse aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!(["GNIREENIGNE", "SNOITAREPO"])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_string_split_on_memory_facade() {
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
            "
                CREATE (n:Person {id: 'split-nested', path: 'A/B'})
                RETURN split(toLower(n.path), '/') AS lowered_parts,
                       split(coalesce(n.nickname, 'fallback/name'), '/') AS fallback_parts,
                       split('static/value', '/') AS literal_parts;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string split arguments should execute");
    assert_eq!(
        nested_split.table.rows,
        vec![vec![
            Value::Json(serde_json::json!(["a", "b"])),
            Value::Json(serde_json::json!(["fallback", "name"])),
            Value::Json(serde_json::json!(["static", "value"])),
        ]]
    );

    let nested_aggregates =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'split'}) SET n.nested_split_counted = true
                RETURN count(split(toLower(n.team), '/')) AS rows,
                       collect(split(toLower(n.team), '/')) AS teams;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested string split aggregate projections should execute");
    assert_eq!(
        nested_aggregates.table.rows,
        vec![vec![
            Value::Int(2),
            Value::Json(serde_json::json!([
                ["engineering", "platform"],
                ["operations", "support"]
            ])),
        ]]
    );
}

#[test]
fn cypher_returning_projects_restricted_case_on_memory_facade() {
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

    let nested =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.nested_case = true
                RETURN n.id AS id,
                       CASE WHEN n.team = 'eng' THEN toUpper(n.id) ELSE coalesce(n.nickname, 'other') END AS group
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted CASE branch projections");
    assert_eq!(
        nested.table.rows,
        vec![
            vec![Value::from("case-bob"), Value::from("CASE-BOB")],
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

    let nested_aggregate =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                MATCH (n:Person {status: 'case'}) SET n.nested_case_aggregate = true
                RETURN collect(CASE WHEN n.team = 'eng' THEN toUpper(n.id) ELSE coalesce(n.nickname, 'other') END) AS buckets;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested restricted CASE aggregate projections");
    assert_eq!(
        nested_aggregate.table.rows,
        vec![vec![Value::Json(serde_json::json!(["CASE-BOB", "other"]))]]
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
                 RETURN CASE WHEN n.team = 'eng' THEN [n.id] ELSE 'other' END;",
            CypherMutationOptions::default(),
        ))
        .expect_err("CASE branches should still reject nested composites");
    assert!(
        matches!(
            error,
            GrustError::CypherUnsupportedCardinality(_) | GrustError::CypherSyntax(_)
        ),
        "{error:?}"
    );

    let error =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person {status: 'case'}) SET n.flag = true
                RETURN CASE WHEN n.team = 'eng' THEN m.id ELSE 'other' END;
                ",
            CypherMutationOptions::default(),
        ))
        .expect_err("CASE branches should reject cross-variable values");
    assert!(
        matches!(error, GrustError::CypherUnsupportedCardinality(_)),
        "{error:?}"
    );
}

#[test]
fn cypher_returning_projects_broad_node_rows_on_memory_facade() {
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
fn cypher_returning_projects_deleted_broad_node_rows_on_memory_facade() {
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
fn cypher_returning_projects_broad_edge_rows_on_memory_facade() {
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
fn cypher_returning_projects_deleted_broad_edge_rows_on_memory_facade() {
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
fn cypher_returning_evaluates_row_produced_edge_values() {
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
fn cypher_returning_allows_control_words_as_aliases() {
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
fn cypher_returning_generic_strict_create_checks_memory_facade() {
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
fn cypher_returning_rejects_deferred_result_forms() {
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
fn cypher_returning_classifies_materialization_targets() {
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::All),
        CypherReturnTargetMaterialization::Star
    );
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::Element),
        CypherReturnTargetMaterialization::Element
    );
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::Property("id".into())),
        CypherReturnTargetMaterialization::DirectProperty
    );
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::Literal(Value::Int(1))),
        CypherReturnTargetMaterialization::ScalarProjection
    );
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::ElementId),
        CypherReturnTargetMaterialization::ElementFunction
    );
    assert_eq!(
        classify_return_target_materialization(&CypherReturnTarget::PathLength),
        CypherReturnTargetMaterialization::PathFunction
    );
}

#[test]
fn cypher_returning_classifies_scalar_projection_kinds() {
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::All),
        CypherReturnScalarProjectionKind::Star
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::Element),
        CypherReturnScalarProjectionKind::Element
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::Property("id".into())),
        CypherReturnScalarProjectionKind::DirectProperty
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::Literal(Value::Bool(true))),
        CypherReturnScalarProjectionKind::Literal
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::MapProjection(
            CypherReturnMapProjection {
                variable: "n".into(),
                entries: vec![CypherReturnMapProjectionEntry {
                    output_key: "id".into(),
                    value: CypherReturnTarget::Property("id".into()),
                }],
            },
        )),
        CypherReturnScalarProjectionKind::Map
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::ListProjection(
            CypherReturnListProjection {
                variable: Some("n".into()),
                terms: vec![CypherReturnTarget::Property("id".into())],
            },
        )),
        CypherReturnScalarProjectionKind::List
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::Case(CypherReturnCase {
            key: "status".into(),
            equals: Value::from("active"),
            then_target: Box::new(CypherReturnTarget::Literal(Value::Bool(true))),
            else_target: Box::new(CypherReturnTarget::Literal(Value::Bool(false))),
        })),
        CypherReturnScalarProjectionKind::Conditional
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::Coalesce(CypherReturnCoalesce {
            variable: Some("n".into()),
            terms: vec![CypherReturnTarget::Property("name".into())],
        },)),
        CypherReturnScalarProjectionKind::Coalesce
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyExists("name".into())),
        CypherReturnScalarProjectionKind::Introspection
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyListIndex(
            CypherReturnListIndexProjection {
                key: "tags".into(),
                index: CypherReturnListBound {
                    variable: None,
                    target: Box::new(CypherReturnTarget::Literal(Value::Int(0))),
                },
            },
        )),
        CypherReturnScalarProjectionKind::ListAccess
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyListPredicate(
            CypherReturnListPredicateProjection {
                key: "tags".into(),
                predicate: CypherReturnListPredicate::Any,
                item_variable: "tag".into(),
                equals_variable: None,
                equals: Box::new(CypherReturnTarget::Literal(Value::from("speaker"))),
            },
        )),
        CypherReturnScalarProjectionKind::ListPredicate
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyAbs(
            CypherReturnAbsProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("score".into())),
            },
        )),
        CypherReturnScalarProjectionKind::Numeric
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyToString(
            CypherReturnToStringProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("id".into())),
            },
        )),
        CypherReturnScalarProjectionKind::Conversion
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PropertyStringSplit(
            CypherReturnStringSplit {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("path".into())),
                delimiter: "/".into(),
            },
        )),
        CypherReturnScalarProjectionKind::String
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::ElementId),
        CypherReturnScalarProjectionKind::ElementFunction
    );
    assert_eq!(
        classify_return_scalar_projection(&CypherReturnTarget::PathNodes),
        CypherReturnScalarProjectionKind::PathFunction
    );
}

#[test]
fn cypher_returning_builds_scalar_ast() {
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::All),
        CypherReturnScalarAst::Star
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::Element),
        CypherReturnScalarAst::Element
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::Property("id".into())),
        CypherReturnScalarAst::DirectProperty("id")
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::Literal(Value::Int(1))),
        CypherReturnScalarAst::Literal(Value::Int(1))
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::PropertyListIndex(
            CypherReturnListIndexProjection {
                key: "tags".into(),
                index: CypherReturnListBound {
                    variable: None,
                    target: Box::new(CypherReturnTarget::Literal(Value::Int(1))),
                },
            },
        )),
        CypherReturnScalarAst::PropertyListIndex(CypherReturnListIndexProjection {
            key,
            index,
        }) if key == "tags"
            && index.variable.is_none()
            && *index.target == CypherReturnTarget::Literal(Value::Int(1))
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::PropertyNumericRound(
            CypherReturnNumericRoundProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("score".into())),
                round: CypherReturnNumericRound::Ceil,
            }
        )),
        CypherReturnScalarAst::PropertyNumericRound(CypherReturnNumericRoundProjection {
            variable: Some(variable),
            target,
            round: CypherReturnNumericRound::Ceil,
        }) if variable == "n" && **target == CypherReturnTarget::Property("score".into())
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::PropertyStringTransform(
            CypherReturnStringTransformProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("name".into())),
                transform: CypherReturnStringTransform::Upper,
            }
        )),
        CypherReturnScalarAst::PropertyStringTransform(
            CypherReturnStringTransformProjection {
                variable: Some(variable),
                target,
                transform: CypherReturnStringTransform::Upper,
            }
        ) if variable == "n" && matches!(target.as_ref(), CypherReturnTarget::Property(key) if key == "name")
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::PropertyStringTransform(
            CypherReturnStringTransformProjection {
                variable: None,
                target: Box::new(CypherReturnTarget::Literal(Value::from("ADA"))),
                transform: CypherReturnStringTransform::Lower,
            }
        )),
        CypherReturnScalarAst::PropertyStringTransform(
            CypherReturnStringTransformProjection {
                variable: None,
                target,
                transform: CypherReturnStringTransform::Lower,
            }
        ) if matches!(target.as_ref(), CypherReturnTarget::Literal(Value::String(value)) if value == "ADA")
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::ElementId),
        CypherReturnScalarAst::ElementFunction
    ));
    assert!(matches!(
        scalar_return_ast(&CypherReturnTarget::PathRelationships),
        CypherReturnScalarAst::PathFunction
    ));
}

#[test]
fn cypher_returning_classifies_scalar_ast_families() {
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::All)),
        CypherReturnScalarAstFamily::Binding
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::PathNodes)),
        CypherReturnScalarAstFamily::Wrapper
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::ListProjection(
            CypherReturnListProjection {
                variable: Some("n".into()),
                terms: vec![CypherReturnTarget::Property("id".into())],
            }
        ))),
        CypherReturnScalarAstFamily::Value
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::Case(
            CypherReturnCase {
                key: "status".into(),
                equals: Value::from("active"),
                then_target: Box::new(CypherReturnTarget::Literal(Value::Bool(true))),
                else_target: Box::new(CypherReturnTarget::Literal(Value::Bool(false))),
            }
        ))),
        CypherReturnScalarAstFamily::Control
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(&CypherReturnTarget::PropertySize(
            "tags".into()
        ))),
        CypherReturnScalarAstFamily::Introspection
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(
            &CypherReturnTarget::PropertyListContains(CypherReturnListContains {
                key: "tags".into(),
                needle: Value::from("speaker"),
            })
        )),
        CypherReturnScalarAstFamily::List
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(
            &CypherReturnTarget::PropertyNumericSign(CypherReturnNumericSignProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("score".into())),
            })
        )),
        CypherReturnScalarAstFamily::Numeric
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(
            &CypherReturnTarget::PropertyToBoolean(CypherReturnToBooleanProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("active".into())),
            })
        )),
        CypherReturnScalarAstFamily::Conversion
    );
    assert_eq!(
        classify_return_scalar_ast_family(&scalar_return_ast(
            &CypherReturnTarget::PropertyIsEmpty(CypherReturnIsEmptyProjection {
                variable: Some("n".into()),
                target: Box::new(CypherReturnTarget::Property("name".into())),
            })
        )),
        CypherReturnScalarAstFamily::String
    );
}

#[test]
fn cypher_returning_groups_mixed_aggregate_rows() {
    let store = MemoryGraphStore::new();

    let grouped =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'ada', status: 'active', team: 'eng', score: 10, code: 'eng/ada'});
                CREATE (:Person {id: 'bob', status: 'active', team: 'eng', score: 20, code: 'eng/bob'});
                CREATE (:Person {id: 'cara', status: 'active', team: 'ops', score: 7, code: 'ops/cara'});
                MATCH (n:Person {status: 'active'}) SET n.seen = true
                RETURN n.team AS team,
                       count(*) AS people,
                       sum(n.score) AS total,
                       collect(n.id) AS ids,
                       collect(split(n.code, '/')) AS code_parts,
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
            "code_parts".to_string(),
            "rows".to_string()
        ]
    );
    assert_eq!(grouped.table.rows.len(), 2);
    assert_eq!(
        &grouped.table.rows[0][..5],
        &[
            Value::from("eng"),
            Value::Int(2),
            Value::Int(30),
            Value::Json(serde_json::json!(["ada", "bob"])),
            Value::Json(serde_json::json!([["eng", "ada"], ["eng", "bob"]]))
        ]
    );
    let Value::Json(eng_rows) = &grouped.table.rows[0][5] else {
        panic!("collect(*) should return JSON rows");
    };
    assert_eq!(eng_rows.as_array().expect("array").len(), 2);
    assert_eq!(eng_rows[0]["n"]["id"], serde_json::json!("ada"));
    assert_eq!(eng_rows[1]["n"]["id"], serde_json::json!("bob"));
    assert_eq!(
        &grouped.table.rows[1][..5],
        &[
            Value::from("ops"),
            Value::Int(1),
            Value::Int(7),
            Value::Json(serde_json::json!(["cara"])),
            Value::Json(serde_json::json!([["ops", "cara"]]))
        ]
    );
    let Value::Json(ops_rows) = &grouped.table.rows[1][5] else {
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
fn cypher_returning_counts_materialized_rows_on_memory_facade() {
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

    let shared_projection_targets =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'count-projection-a', code: 'a/b', score: 7});
                CREATE (:Person {id: 'count-projection-b', code: 'c/d', score: 7});
                MATCH (n:Person) WHERE n.id STARTS WITH 'count-projection-'
                SET n.counted = true
                RETURN count(split(n.code, '/')) AS split_codes,
                       count(DISTINCT toString(n.score)) AS distinct_scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted count projections should share scalar materialization");
    assert_eq!(
        shared_projection_targets.table,
        CypherResultTable {
            columns: vec!["split_codes".to_string(), "distinct_scores".to_string()],
            rows: vec![vec![Value::Int(2), Value::Int(1)]],
        }
    );
}

#[test]
fn cypher_returning_counts_distinct_materialized_values() {
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
fn cypher_returning_evaluates_restricted_numeric_aggregates() {
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
fn cypher_returning_rejects_unsupported_aggregate_forms() {
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
fn cypher_returning_collects_restricted_materialized_values() {
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

    let shared_projection_targets =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'aggregate-projection-a', code: 'a/b', score: 7});
                CREATE (:Person {id: 'aggregate-projection-b', code: 'c/d', score: 11});
                MATCH (n:Person) WHERE n.id STARTS WITH 'aggregate-projection-'
                SET n.seen = true
                RETURN collect(split(n.code, '/')) AS split_codes,
                       collect(toString(n.score)) AS string_scores;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted aggregate projections should share scalar materialization");
    assert_eq!(
        shared_projection_targets.table.rows,
        vec![vec![
            Value::Json(serde_json::json!([["a", "b"], ["c", "d"]])),
            Value::Json(serde_json::json!(["7", "11"])),
        ]]
    );

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
fn cypher_returning_collects_bound_elements() {
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
fn cypher_returning_count_rejects_unbound_variables() {
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
fn cypher_returning_accepts_limit_all_on_memory_facade() {
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
fn cypher_returning_accepts_offset_control() {
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
fn cypher_returning_distinct_dedupes_materialized_rows() {
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
fn cypher_returning_distinct_requires_projection() {
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
fn cypher_returning_orders_by_projection_expression() {
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
fn cypher_returning_generic_row_producing_edges_memory_facade() {
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
