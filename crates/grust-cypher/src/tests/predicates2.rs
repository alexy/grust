//! predicates2 tests (split verbatim from the former monolithic tests.rs).
use super::*;

#[test]
fn cypher_match_where_in_or_equal_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-in-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-in-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-in-or-cara', status: 'review'});
                CREATE (:Person {id: 'where-in-or-dan', status: 'blocked'});
                CREATE (:Person {id: 'where-in-or-missing'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending'] OR n.status = 'review'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted IN OR equality WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-in-or-ada"), Value::Bool(true)],
            vec![Value::from("where-in-or-bob"), Value::Bool(true)],
            vec![Value::from("where-in-or-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_duplicate_folded_values_execute_once_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-fold-dedup-ada', status: 'active'});
                CREATE (:Person {id: 'where-fold-dedup-bob', status: 'pending'});
                CREATE (:Person {id: 'where-fold-dedup-cara', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.status = 'active'
                   OR n.status = 'active'
                   OR n.status IN ['pending', 'pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("duplicate folded WHERE values should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-fold-dedup-ada"), Value::Bool(true)],
            vec![Value::from("where-fold-dedup-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_nested_or_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-nested-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-nested-or-cara', status: 'review'});
                CREATE (:Person {id: 'where-nested-or-dan', status: 'blocked'});
                CREATE (:Person {id: 'where-nested-or-missing'});
                MATCH (n:Person)
                WHERE (n.status = 'active' OR n.status = 'pending')
                   OR n.status = 'review'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested folded OR WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-nested-or-ada"), Value::Bool(true)],
            vec![Value::from("where-nested-or-bob"), Value::Bool(true)],
            vec![Value::from("where-nested-or-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_intersected_in_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-in-intersect-ada', status: 'active'});
                CREATE (:Person {id: 'where-in-intersect-bob', status: 'pending'});
                CREATE (:Person {id: 'where-in-intersect-cara', status: 'review'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending']
                  AND n.status IN ['pending', 'review']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("intersected IN WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-in-intersect-bob"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_empty_in_intersection_matches_no_memory_rows() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-empty-in-ada', status: 'active'});
                CREATE (:Person {id: 'where-empty-in-bob', status: 'pending'});
                MATCH (n:Person)
                WHERE n.status IN ['active']
                  AND n.status IN ['pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("empty IN intersections should execute on memory facade");

    assert!(result.table.rows.is_empty());
}

#[test]
fn cypher_match_where_equality_membership_canonicalization_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-eq-membership-ada', status: 'active'});
                CREATE (:Person {id: 'where-eq-membership-bob', status: 'pending'});
                CREATE (:Person {id: 'where-eq-membership-cara', status: 'review'});
                CREATE (:Person {id: 'where-eq-membership-dan', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending', 'review']
                  AND NOT n.status IN ['blocked', 'pending']
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("equality/membership WHERE canonicalization should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-eq-membership-ada"), Value::Bool(true)],
            vec![Value::from("where-eq-membership-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_inequality_canonicalization_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-neq-canonical-ada', status: 'active'});
                CREATE (:Person {id: 'where-neq-canonical-bob', status: 'pending'});
                CREATE (:Person {id: 'where-neq-canonical-cara', status: 'review'});
                MATCH (n:Person)
                WHERE n.status IN ['active', 'pending', 'review']
                  AND n.status <> 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("inequality WHERE canonicalization should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-neq-canonical-ada"), Value::Bool(true)],
            vec![Value::from("where-neq-canonical-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_order_canonicalization_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-order-canonical-ada', score: 9});
                CREATE (:Person {id: 'where-order-canonical-bob', score: 13});
                CREATE (:Person {id: 'where-order-canonical-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score >= 10
                  AND n.score > 12
                  AND n.score <= 20
                  AND n.score < 18
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("order WHERE canonicalization should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-order-canonical-bob"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_equality_order_canonicalization_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-eq-order-ada', score: 9});
                CREATE (:Person {id: 'where-eq-order-bob', score: 13});
                CREATE (:Person {id: 'where-eq-order-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score = 13
                  AND n.score >= 10
                  AND n.score < 20
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("equality/order WHERE canonicalization should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-eq-order-bob"), Value::Bool(true)]]
    );
}

#[test]
fn cypher_match_where_membership_order_canonicalization_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-in-order-ada', score: 9});
                CREATE (:Person {id: 'where-in-order-bob', score: 13});
                CREATE (:Person {id: 'where-in-order-cara', score: 19});
                MATCH (n:Person)
                WHERE n.score IN [9, 13, 19]
                  AND n.score > 10
                  AND n.score < 18
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("membership/order WHERE canonicalization should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-in-order-bob"), Value::Bool(true)]]
    );
}

#[test]
fn cypher_match_where_negated_or_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-or-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-or-dan', status: 'archived'});
                CREATE (:Person {id: 'where-not-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'blocked' OR n.status = 'archived')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted negated OR WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-or-ada"), Value::Bool(true)],
            vec![Value::from("where-not-or-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_negated_null_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-null-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-null-or-cara', status: null});
                CREATE (:Person {id: 'where-not-null-or-dan'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'inactive' OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-null-or-ada"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-null-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-nested-null-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-nested-null-or-cara', status: 'paused'});
                CREATE (:Person {id: 'where-not-nested-null-or-dan', status: null});
                CREATE (:Person {id: 'where-not-nested-null-or-eve'});
                MATCH (n:Person)
                WHERE NOT ((n.status = 'inactive' OR n.status = 'paused') OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated null OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-nested-null-or-ada"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_negated_null_membership_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-membership-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-null-membership-or-bob', status: 'inactive'});
                CREATE (:Person {id: 'where-not-null-membership-or-cara', status: 'paused'});
                CREATE (:Person {id: 'where-not-null-membership-or-dan', status: null});
                CREATE (:Person {id: 'where-not-null-membership-or-eve'});
                MATCH (n:Person)
                WHERE NOT (n.status IN ['inactive', 'paused'] OR n.status = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null membership OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-null-membership-or-ada"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_negated_null_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-string-or-alan"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-string-or-bob"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-nested-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-nested-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-nested-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated null string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-nested-string-or-alan"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-nested-string-or-bob"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_null_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-null-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-null-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-null-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null string ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null string ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-null-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-null-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_null_mixed_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-null-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'] OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null mixed string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-mixed-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-mixed-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_mixed_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null mixed string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-null-mixed-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-null-mixed-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_mixed_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR n.name IN ['Alan'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated mixed string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-mixed-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-mixed-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_mixed_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-mixed-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M' OR (n.name IN ['Alan'] OR n.name = 'Bob'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated mixed string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-mixed-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-mixed-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name < 'M')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_string_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-string-order-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-zoe', name: 'Zoe'});
                CREATE (:Person {id: 'where-not-nested-string-order-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-string-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name < 'M')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated string/order OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-string-order-or-mira"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-string-order-or-zoe"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_mixed_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated mixed string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-mixed-string-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-mixed-string-or-mira"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_mixed_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated mixed string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-nested-mixed-string-or-mira"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_negated_null_mixed_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-null-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-cara', name: null});
                CREATE (:Person {id: 'where-not-null-mixed-string-or-dan'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR n.name IN ['Alan'] OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated null mixed string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-null-mixed-string-or-bob"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_mixed_string_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-mira', name: 'Mira'});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-null', name: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-string-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr') OR (n.name IN ['Alan'] OR n.name = 'Bob') OR n.name = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated null mixed string OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-nested-null-mixed-string-or-mira"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_negated_null_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-null-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-null-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-null-order-or-dan', score: null});
                CREATE (:Person {id: 'where-not-null-order-or-eve'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-order-or-cara"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-null-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-null-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-null-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-null-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-nested-null-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-nested-null-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-null-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated null ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-null-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-null-order-or-cara"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-null-order-or-dan"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_null_mixed_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-eve', score: 20});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-fay', score: 21});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-null-mixed-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18] OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated null mixed ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-null-mixed-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-null-mixed-order-or-eve"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_null_mixed_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-null-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-null-mixed-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score IN [15, 18] OR n.score = null)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated null mixed ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-null-mixed-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-null-mixed-order-or-eve"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score > 20)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-order-or-bob"), Value::Bool(true)],
            vec![Value::from("where-not-order-or-cara"), Value::Bool(true)],
            vec![Value::from("where-not-order-or-dan"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-order-or-ada', score: 5});
                CREATE (:Person {id: 'where-not-nested-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-order-or-dan', score: 20});
                CREATE (:Person {id: 'where-not-nested-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-nested-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-nested-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score <= 5)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-order-or-cara"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-order-or-dan"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_mixed_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-eve', score: 20});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-fay', score: 21});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-null', score: null});
                CREATE (:Person {id: 'where-not-nested-mixed-order-or-missing'});
                MATCH (n:Person)
                WHERE NOT ((n.score < 10 OR n.score > 20) OR n.score IN [15, 18])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested negated mixed ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-mixed-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-mixed-order-or-eve"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_mixed_order_or_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-mixed-order-or-ada', score: 9});
                CREATE (:Person {id: 'where-not-mixed-order-or-bob', score: 10});
                CREATE (:Person {id: 'where-not-mixed-order-or-cara', score: 15});
                CREATE (:Person {id: 'where-not-mixed-order-or-dan', score: 18});
                CREATE (:Person {id: 'where-not-mixed-order-or-eve', score: 21});
                CREATE (:Person {id: 'where-not-mixed-order-or-fay', score: null});
                CREATE (:Person {id: 'where-not-mixed-order-or-gus'});
                MATCH (n:Person)
                WHERE NOT (n.score < 10 OR n.score IN [15, 18])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated mixed ordered OR terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-mixed-order-or-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-mixed-order-or-eve"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_and_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-and-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-and-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-and-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-and-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status <> 'active' AND n.status <> 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted negated AND WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-and-ada"), Value::Bool(true)],
            vec![Value::from("where-not-and-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_negated_and_subsumed_terms_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-and-subsumed-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-and-subsumed-bob', status: 'pending'});
                CREATE (:Person {id: 'where-not-and-subsumed-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-not-and-subsumed-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'active' AND n.status IN ['active', 'pending'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated AND subsumed terms should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-and-subsumed-bob"), Value::Bool(true)],
            vec![
                Value::from("where-not-and-subsumed-cara"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_string_and_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-string-and-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-string-and-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-string-and-bob', name: 'Bob'});
                CREATE (:Person {id: 'where-not-string-and-missing'});
                MATCH (n:Person)
                WHERE NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted negated string AND WHERE predicates should execute");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-not-string-and-ada"), Value::Bool(true)],
            vec![Value::from("where-not-string-and-grace"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_nested_negated_string_and_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-nested-string-and-ada', name: 'Ada'});
                CREATE (:Person {id: 'where-not-nested-string-and-grace', name: 'Grace'});
                CREATE (:Person {id: 'where-not-nested-string-and-alan', name: 'Alan'});
                CREATE (:Person {id: 'where-not-nested-string-and-bob', name: 'Bob'});
                MATCH (n:Person)
                WHERE NOT ((NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr') AND NOT n.name STARTS WITH 'Al')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("nested negated string AND WHERE predicates should execute");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-not-nested-string-and-ada"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-string-and-alan"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-not-nested-string-and-grace"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_duplicate_negated_and_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-dup-and-ada', status: 'active'});
                CREATE (:Person {id: 'where-not-dup-and-bob', status: 'blocked'});
                CREATE (:Person {id: 'where-not-dup-and-missing'});
                MATCH (n:Person)
                WHERE NOT (n.status = 'blocked' AND n.status = 'blocked')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("duplicate negated AND WHERE predicates should execute");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-dup-and-ada"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_or_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-ada', status: 'active'});
                CREATE (:Person {id: 'where-or-bob', status: 'pending'});
                CREATE (:Person {id: 'where-or-cara', status: 'blocked'});
                CREATE (:Person {id: 'where-or-missing'});
                MATCH (n:Person)
                WHERE n.status = 'active' OR n.status = 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted OR WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-or-ada"), Value::Bool(true)],
            vec![Value::from("where-or-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_or_of_and_groups_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-and-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-and-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-and-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-or-and-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("factored OR of AND WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-or-and-ada"), Value::Bool(true)],
            vec![Value::from("where-or-and-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_boolean_ast_factored_or_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-ast-or-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-ast-or-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-ast-or-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-ast-or-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.kind = 'person' AND n.status = 'active'
                   OR n.kind = 'person' AND n.status = 'pending'
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("boolean AST factored OR WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-ast-or-ada"), Value::Bool(true)],
            vec![Value::from("where-ast-or-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_negated_subsumed_factored_or_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-not-subsumed-or-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-not-subsumed-or-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-not-subsumed-or-bot', kind: 'bot', status: 'active'});
                CREATE (:Person {id: 'where-not-subsumed-or-missing', status: 'active'});
                MATCH (n:Person)
                WHERE NOT ((n.kind = 'person' AND n.status = 'active') OR n.kind = 'person')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated subsumed factored OR should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-not-subsumed-or-bot"),
            Value::Bool(true)
        ],]
    );
}

#[test]
fn cypher_match_where_canonicalized_or_branch_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-branch-canon-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-branch-canon-bob', kind: 'person', status: 'review'});
                CREATE (:Person {id: 'where-or-branch-canon-cara', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-branch-canon-dan', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person'
                       AND n.status = 'active'
                       AND n.status IN ['active', 'pending'])
                   OR (n.kind = 'person'
                       AND n.status = 'review'
                       AND n.status IN ['review', 'archived'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("canonicalized OR branch predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-or-branch-canon-ada"), Value::Bool(true),],
            vec![Value::from("where-or-branch-canon-bob"), Value::Bool(true),],
        ]
    );
}

#[test]
fn cypher_match_where_pruned_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-prune-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-prune-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-prune-cara', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'blocked' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("pruned impossible OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-or-prune-ada"), Value::Bool(true)]]
    );
}

#[test]
fn cypher_match_where_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-subsumed-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-subsumed-bob', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-subsumed-cara', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-or-subsumed-ada"), Value::Bool(true)],
            vec![Value::from("where-or-subsumed-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_semantically_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-semantic-subsumed-ada', kind: 'person', status: 'active', region: 'us'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-bob', kind: 'person', status: 'pending', region: 'eu'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-cara', kind: 'person', status: 'blocked', region: 'us'});
                CREATE (:Person {id: 'where-or-semantic-subsumed-dan', kind: 'bot', status: 'active', region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status = 'active' AND n.region = 'us')
                   OR (n.kind = 'person' AND n.status IN ['active', 'pending'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("semantically subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-semantic-subsumed-ada"),
                Value::Bool(true),
            ],
            vec![
                Value::from("where-or-semantic-subsumed-bob"),
                Value::Bool(true),
            ],
        ]
    );
}

#[test]
fn cypher_match_where_string_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-string-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-string-subsumed-grace', kind: 'person', name: 'Grace'});
                CREATE (:Person {id: 'where-or-string-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-string-subsumed-adbot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
                   OR (n.kind = 'person' AND (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("string-subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-string-subsumed-ada"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-string-subsumed-grace"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_negated_string_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-neg-string-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-grace', kind: 'person', name: 'Grace'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-neg-string-subsumed-adbot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND NOT (n.name STARTS WITH 'Ad' OR n.name STARTS WITH 'Gr'))
                   OR (n.kind = 'person' AND NOT n.name STARTS WITH 'Ad')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("negated string-subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-neg-string-subsumed-bob"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-neg-string-subsumed-grace"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_null_check_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let non_null =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-null-subsumed-ada', kind: 'person', name: 'Ada'});
                CREATE (:Person {id: 'where-or-null-subsumed-bob', kind: 'person', name: 'Bob'});
                CREATE (:Person {id: 'where-or-null-subsumed-null', kind: 'person', name: null});
                CREATE (:Person {id: 'where-or-null-subsumed-missing', kind: 'person'});
                CREATE (:Person {id: 'where-or-null-subsumed-bot', kind: 'bot', name: 'AdaBot'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.name STARTS WITH 'Ad')
                   OR (n.kind = 'person' AND n.name IS NOT NULL)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("null-check subsumed OR branches should execute on memory facade");

    assert_eq!(
        non_null.table.rows,
        vec![
            vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
            vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
        ]
    );

    let simple =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person)
                WHERE n.name STARTS WITH 'Ad' OR n.name IS NOT NULL
                SET n.simple_selected = true
                RETURN n.id AS id, n.simple_selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("simple null-check subsumed OR terms should execute on memory facade");

    assert_eq!(
        simple.table.rows,
        vec![
            vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
            vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
            vec![Value::from("where-or-null-subsumed-bot"), Value::Bool(true)],
        ]
    );

    let negated_simple =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                MATCH (n:Person)
                WHERE NOT (n.name STARTS WITH 'Ad' OR n.name IS NOT NULL)
                SET n.negated_simple_selected = true
                RETURN n.id AS id, n.negated_simple_selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("negated simple null-check subsumed OR terms should execute on memory facade");

    assert_eq!(
        negated_simple.table.rows,
        vec![
            vec![
                Value::from("where-or-null-subsumed-missing"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-null-subsumed-null"),
                Value::Bool(true)
            ],
        ]
    );

    let null = futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
        &store,
        "
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.nickname = null)
                   OR (n.kind = 'person' AND n.nickname IS NULL)
                SET n.needs_nickname = true
                RETURN n.id AS id, n.needs_nickname AS needs_nickname
                ORDER BY id;
                ",
        CypherMutationOptions::default(),
    ))
    .expect("IS NULL subsumed OR branches should execute on memory facade");

    assert_eq!(
        null.table.rows,
        vec![
            vec![Value::from("where-or-null-subsumed-ada"), Value::Bool(true)],
            vec![Value::from("where-or-null-subsumed-bob"), Value::Bool(true)],
            vec![
                Value::from("where-or-null-subsumed-missing"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-null-subsumed-null"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_singleton_not_in_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-not-in-subsumed-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-cara', kind: 'person', status: 'blocked'});
                CREATE (:Person {id: 'where-or-not-in-subsumed-dan', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status <> 'blocked')
                   OR (n.kind = 'person' AND NOT n.status IN ['blocked'])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("singleton NOT IN subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-not-in-subsumed-ada"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-not-in-subsumed-bob"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_singleton_in_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-in-subsumed-ada', kind: 'person', status: 'active', region: 'us'});
                CREATE (:Person {id: 'where-or-in-subsumed-bob', kind: 'person', status: 'active', region: 'eu'});
                CREATE (:Person {id: 'where-or-in-subsumed-cara', kind: 'person', status: 'pending', region: 'us'});
                CREATE (:Person {id: 'where-or-in-subsumed-dan', kind: 'bot', status: 'active', region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status IN ['active'] AND n.region = 'us')
                   OR (n.kind = 'person' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("singleton IN subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-or-in-subsumed-ada"), Value::Bool(true)],
            vec![Value::from("where-or-in-subsumed-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_equality_preferred_over_singleton_in_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-or-in-prefer-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-or-in-prefer-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-or-in-prefer-bot', kind: 'bot', status: 'active'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.status IN ['active'])
                   OR (n.kind = 'person' AND n.status = 'active')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("equality-preferred singleton IN OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![
            Value::from("where-or-in-prefer-ada"),
            Value::Bool(true)
        ]]
    );
}

#[test]
fn cypher_match_where_order_inequality_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-neq-subsumed-ada', kind: 'person', score: 25, region: 'us'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-cara', kind: 'person', score: 5, region: 'us'});
                CREATE (:Person {id: 'where-or-order-neq-subsumed-dan', kind: 'bot', score: 25, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND n.score <> 5)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound inequality-subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-order-neq-subsumed-ada"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-order-neq-subsumed-bob"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_order_not_in_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-ada', kind: 'person', score: 25, region: 'us'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-cara', kind: 'person', score: 5, region: 'us'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-dan', kind: 'person', score: 10, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-not-in-subsumed-bot', kind: 'bot', score: 25, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND NOT n.score IN [5, 10])
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound NOT IN subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-order-not-in-subsumed-ada"),
                Value::Bool(true)
            ],
            vec![
                Value::from("where-or-order-not-in-subsumed-bob"),
                Value::Bool(true)
            ],
        ]
    );
}

#[test]
fn cypher_match_where_order_subsumed_or_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
            futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
                &store,
                "
                CREATE (:Person {id: 'where-or-order-subsumed-ada', kind: 'person', score: 9, region: 'us'});
                CREATE (:Person {id: 'where-or-order-subsumed-bob', kind: 'person', score: 13, region: 'eu'});
                CREATE (:Person {id: 'where-or-order-subsumed-cara', kind: 'person', score: 21, region: 'us'});
                CREATE (:Person {id: 'where-or-order-subsumed-dan', kind: 'bot', score: 30, region: 'us'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.score > 20 AND n.region = 'us')
                   OR (n.kind = 'person' AND n.score > 10)
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
                CypherMutationOptions::default(),
            ))
            .expect("ordered-bound subsumed OR branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![
                Value::from("where-or-order-subsumed-bob"),
                Value::Bool(true),
            ],
            vec![
                Value::from("where-or-order-subsumed-cara"),
                Value::Bool(true),
            ],
        ]
    );
}

#[test]
fn cypher_match_where_nested_or_terms_inside_and_branches_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-nested-or-ada', region: 'us', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-bob', region: 'us', status: 'pending'});
                CREATE (:Person {id: 'where-nested-or-cara', region: 'eu', status: 'active'});
                CREATE (:Person {id: 'where-nested-or-dan', region: 'eu', status: 'blocked'});
                CREATE (:Person {id: 'where-nested-or-eve', region: 'apac', status: 'active'});
                MATCH (n:Person)
                WHERE (n.region = 'us' AND (n.status = 'active' OR n.status = 'pending'))
                   OR (n.region = 'eu' AND (n.status = 'active' OR n.status = 'pending'))
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("nested OR terms inside factored AND branches should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-nested-or-ada"), Value::Bool(true)],
            vec![Value::from("where-nested-or-bob"), Value::Bool(true)],
            vec![Value::from("where-nested-or-cara"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_duplicate_factored_branch_predicates_execute_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-branch-dedup-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-branch-dedup-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-branch-dedup-cara', kind: 'system', status: 'active'});
                CREATE (:Person {id: 'where-branch-dedup-dan', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE (n.kind = 'person' AND n.kind = 'person' AND n.status = 'active')
                   OR (n.kind = 'person' AND n.status = 'pending' AND n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("duplicate predicates inside factored OR branches should execute");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-branch-dedup-ada"), Value::Bool(true)],
            vec![Value::from("where-branch-dedup-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_duplicate_predicates_execute_once_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-dedup-ada', kind: 'person', status: 'active'});
                CREATE (:Person {id: 'where-dedup-bob', kind: 'person', status: 'pending'});
                CREATE (:Person {id: 'where-dedup-cara', kind: 'person', status: 'blocked'});
                MATCH (n:Person)
                WHERE n.kind = 'person'
                  AND (n.status = 'active' OR n.status = 'pending')
                  AND n.kind = 'person'
                  AND (n.status = 'active' OR n.status = 'pending')
                SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("duplicate WHERE predicates should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![
            vec![Value::from("where-dedup-ada"), Value::Bool(true)],
            vec![Value::from("where-dedup-bob"), Value::Bool(true)],
        ]
    );
}

#[test]
fn cypher_match_where_in_predicates_execute_on_memory_facade() {
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
fn cypher_match_where_parenthesized_terms_execute_on_memory_facade() {
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
fn cypher_match_where_string_predicates_execute_on_memory_facade() {
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
fn cypher_match_where_not_executes_on_memory_facade() {
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
fn cypher_match_where_double_negation_executes_on_memory_facade() {
    let store = MemoryGraphStore::new();

    let result =
        futures_executor::block_on(execute_cypher_mutation_returning_with_options_on_store(
            &store,
            "
                CREATE (:Person {id: 'where-not-not-ada', active: true});
                CREATE (:Person {id: 'where-not-not-bob', active: false});
                CREATE (:Person {id: 'where-not-not-cara'});
                MATCH (n:Person) WHERE NOT NOT n.active = true SET n.selected = true
                RETURN n.id AS id, n.selected AS selected
                ORDER BY id;
                ",
            CypherMutationOptions::default(),
        ))
        .expect("restricted double-negated WHERE should execute on memory facade");

    assert_eq!(
        result.table.rows,
        vec![vec![Value::from("where-not-not-ada"), Value::Bool(true)]]
    );
}

#[test]
fn cypher_match_where_is_null_executes_on_memory_facade() {
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
fn cypher_match_where_negated_null_checks_execute_on_memory_facade() {
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
fn cypher_match_where_is_not_null_executes_on_memory_facade() {
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
