use chrono::{TimeZone, Timelike, Utc};
use serde_json::Value;
use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, Label, MemoryContent, MemoryId, MemoryKind, Provenance,
    RecalledMemory,
};

use super::{deduplicate, reconcile};

#[test]
fn plans_are_stable_across_input_permutations() {
    let memories = vec![
        memory("d2", "Same   finding", 2),
        memory("r1", "Alice lives in Rome", 1),
        memory("d1", "same finding", 1),
        memory("r2", "Alice lives in Paris", 2),
        memory("single", "Unrelated value", 3),
    ];
    let expected_dedup = fingerprint(&deduplicate(&memories).plan);
    let expected_reconcile = fingerprint(&reconcile(&memories).plan);

    for order in [
        [4, 3, 2, 1, 0],
        [2, 0, 4, 1, 3],
        [1, 4, 3, 0, 2],
        [3, 2, 1, 0, 4],
    ] {
        let permuted: Vec<_> = order
            .into_iter()
            .map(|index| memories[index].clone())
            .collect();
        assert_eq!(fingerprint(&deduplicate(&permuted).plan), expected_dedup);
        assert_eq!(fingerprint(&reconcile(&permuted).plan), expected_reconcile);
    }
}

#[test]
fn multiple_groups_have_canonical_step_and_member_order() {
    let memories = vec![
        memory("z2", "Zeta fact", 3),
        memory("a2", "ALPHA fact", 2),
        memory("z1", "zeta   fact", 1),
        memory("a1", "alpha fact", 1),
    ];
    let planning = deduplicate(&memories);
    assert_eq!(planning.group_count, 2);
    let [alpha, zeta] = planning.plan.steps.as_slice() else {
        panic!("two canonical duplicate groups expected")
    };
    assert_superseded(alpha, &["a1", "a2"]);
    assert_superseded(zeta, &["z1", "z2"]);

    let contradictions = vec![
        memory("alice-old", "Alice lives in Rome", 1),
        memory("bob-old", "Bob drinks tea", 1),
        memory("alice-new", "Alice lives in Paris", 2),
        memory("bob-new", "Bob drinks coffee", 2),
    ];
    let planning = reconcile(&contradictions);
    assert_eq!(planning.invalidated_count, 2);
    assert_eq!(
        invalidated(&planning.plan),
        vec!["alice-old".to_owned(), "bob-old".to_owned()]
    );
}

#[test]
fn validity_ties_use_the_staged_microsecond_then_id_order() {
    let base = Utc
        .with_ymd_and_hms(2026, 8, 5, 12, 0, 0)
        .single()
        .expect("fixture time");
    let actually_older = memory_at(
        "z-source",
        "Alice lives in Rome",
        base.with_nanosecond(100).expect("fixture nanos"),
    );
    let actually_newer = memory_at(
        "a-source",
        "Alice lives in Paris",
        base.with_nanosecond(900).expect("fixture nanos"),
    );

    let planning = reconcile(&[actually_newer.clone(), actually_older.clone()]);
    assert_eq!(
        planning.pairs,
        [("z-source".to_owned(), "a-source".to_owned())]
            .into_iter()
            .collect()
    );
    assert_eq!(invalidated(&planning.plan), vec!["a-source".to_owned()]);

    let duplicates = vec![
        memory_at("z-source", "Same finding", actually_older.valid_from),
        memory_at("a-source", "same finding", actually_newer.valid_from),
    ];
    let planning = deduplicate(&duplicates);
    let [step] = planning.plan.steps.as_slice() else {
        panic!("one duplicate group expected")
    };
    assert_superseded(step, &["a-source", "z-source"]);
}

fn memory(id: &str, text: &str, second: u32) -> RecalledMemory {
    memory_at(
        id,
        text,
        Utc.with_ymd_and_hms(2026, 8, 5, 12, 0, second)
            .single()
            .expect("fixture time"),
    )
}

fn memory_at(id: &str, text: &str, valid_from: chrono::DateTime<Utc>) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind: MemoryKind::Semantic,
        label: Label::Internal,
        content: MemoryContent::text(text),
        entities: Vec::new(),
        provenance: Provenance::Operator,
        valid_from,
    }
}

fn fingerprint(plan: &ConsolidationPlan) -> Value {
    serde_json::to_value(plan).expect("serialize plan")
}

fn assert_superseded(step: &ConsolidationStep, expected: &[&str]) {
    let ConsolidationStep::Supersede { superseded, .. } = step else {
        panic!("supersede step expected")
    };
    assert_eq!(
        superseded.iter().map(MemoryId::as_str).collect::<Vec<_>>(),
        expected
    );
}

fn invalidated(plan: &ConsolidationPlan) -> Vec<String> {
    plan.steps
        .iter()
        .flat_map(|step| match step {
            ConsolidationStep::Invalidate { ids } => ids
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect::<Vec<_>>(),
            ConsolidationStep::Supersede { .. } => Vec::new(),
        })
        .collect()
}
