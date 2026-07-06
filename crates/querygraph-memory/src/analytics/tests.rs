use super::*;
use chrono::{TimeZone, Utc};

fn recalled(id: &str, text: &str, kind: MemoryKind, y: i32) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind,
        label: typesec_memory::Label::Internal,
        content: MemoryContent::text(text),
        entities: vec![],
        provenance: Provenance::Operator,
        valid_from: Utc.with_ymd_and_hms(y, 1, 1, 0, 0, 0).unwrap(),
    }
}

#[test]
fn dedup_groups_exact_duplicates() {
    let mems = vec![
        recalled("m1", "Alice likes espresso", MemoryKind::Semantic, 2023),
        recalled("m2", "alice likes espresso", MemoryKind::Semantic, 2024),
        recalled("m3", "Bob likes tea", MemoryKind::Semantic, 2024),
    ];
    let plan = dedup_plan(&mems);
    assert_eq!(plan.steps.len(), 1, "one duplicate group superseded");
    match &plan.steps[0] {
        ConsolidationStep::Supersede { superseded, .. } => assert_eq!(superseded.len(), 2),
        _ => panic!("expected supersede"),
    }
}

#[test]
fn contradiction_detects_and_invalidates_the_older() {
    let mems = vec![
        recalled("old", "Alice lives in Rome", MemoryKind::Semantic, 2023),
        recalled("new", "Alice lives in Venice", MemoryKind::Semantic, 2025),
        recalled("unrelated", "Bob drinks tea", MemoryKind::Semantic, 2024),
    ];
    let (found, plan) = contradiction_plan(&mems);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].newer.as_str(), "new");
    assert_eq!(found[0].older.as_str(), "old");
    match &plan.steps[0] {
        ConsolidationStep::Invalidate { ids } => assert_eq!(ids[0].as_str(), "old"),
        _ => panic!("expected invalidate of the older assertion"),
    }
}

#[test]
fn importance_weights_recency_and_kind() {
    let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    let recent_profile = recalled("a", "x", MemoryKind::Profile, 2025);
    let old_episode = recalled("b", "y", MemoryKind::Episodic, 2020);
    assert!(
        importance(&recent_profile, now) > importance(&old_episode, now),
        "a recent profile outweighs an old episode"
    );
}
