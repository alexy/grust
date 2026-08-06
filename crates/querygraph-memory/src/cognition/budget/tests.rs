use super::*;
use chrono::{TimeZone, Utc};
use typesec_memory::{
    Label, MAX_COGNITION_SOURCE_BYTES, MemoryContent, MemoryId, MemoryKind, Provenance,
    RecalledMemory,
};

#[test]
fn scalar_budgets_accept_the_limit_and_reject_the_next_value() {
    for (limit, check) in [
        (
            MAX_RESULT_CHUNKS,
            check_result_chunks as fn(usize) -> Result<(), CognitionError>,
        ),
        (MAX_RESULT_ROWS, check_result_rows),
    ] {
        check(limit).expect("budget boundary is inclusive");
        assert!(matches!(
            check(limit + 1),
            Err(CognitionError::ResourceBudgetExceeded(_))
        ));
    }
}

#[cfg(feature = "sail")]
#[test]
fn arrow_budget_reuses_the_grust_sail_transport_limit() {
    assert_eq!(MAX_ARROW_BYTES, grust_sail::MAX_ARROW_IPC_PAYLOAD_BYTES);
    check_arrow_bytes(MAX_ARROW_BYTES).expect("Arrow budget boundary is inclusive");
    assert!(matches!(
        check_arrow_bytes(MAX_ARROW_BYTES + 1),
        Err(CognitionError::ResourceBudgetExceeded(_))
    ));
}

#[test]
fn reconcile_budget_is_checked_from_the_pairwise_work_bound() {
    let largest_allowed = (0..=typesec_memory::MAX_COGNITION_SOURCE_COUNT)
        .rev()
        .find(|count| check_reconcile_work(*count).is_ok())
        .expect("at least an empty input is allowed");
    check_reconcile_work(largest_allowed).expect("work boundary is inclusive");
    assert!(matches!(
        check_reconcile_work(largest_allowed + 1),
        Err(CognitionError::ResourceBudgetExceeded(
            "local reconcile work"
        ))
    ));
}

#[test]
fn resource_errors_are_fixed_and_do_not_include_observed_counts() {
    let error = check_result_rows(MAX_RESULT_ROWS + 7).expect_err("over budget");
    assert_eq!(
        error.to_string(),
        "cognition resource budget exceeded: Sail result rows"
    );
    assert!(
        !error
            .to_string()
            .contains(&(MAX_RESULT_ROWS + 7).to_string())
    );
}

#[test]
fn source_budget_errors_are_mapped_without_type_sec_details() {
    let error = source_budget_error(typesec_memory::CognitionApplyError::LimitExceeded(
        "upstream detail",
    ));
    assert_eq!(
        error.to_string(),
        "cognition resource budget exceeded: authorized source input"
    );
    assert!(!error.to_string().contains("upstream detail"));
}

#[test]
fn authorized_id_and_text_budget_accepts_exactly_four_mibibytes() {
    const ID: &str = "source";
    let exact = memory(ID, &"x".repeat(MAX_COGNITION_SOURCE_BYTES - ID.len()));
    check_authorized_input(std::slice::from_ref(&exact)).expect("inclusive source byte limit");

    let mut over_text = exact.content.text.clone();
    over_text.push('x');
    let over = memory(ID, &over_text);
    assert!(matches!(
        check_authorized_input(&[over]),
        Err(CognitionError::ResourceBudgetExceeded(
            "authorized source input"
        ))
    ));
}

fn memory(id: &str, text: &str) -> RecalledMemory {
    RecalledMemory {
        id: MemoryId::from_string(id),
        kind: MemoryKind::Semantic,
        label: Label::Internal,
        content: MemoryContent::text(text),
        entities: Vec::new(),
        provenance: Provenance::Operator,
        valid_from: Utc
            .with_ymd_and_hms(2026, 8, 5, 12, 0, 0)
            .single()
            .expect("fixture time"),
    }
}
