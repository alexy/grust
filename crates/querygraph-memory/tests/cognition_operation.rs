use std::str::FromStr;

use querygraph_memory::cognition::{
    CognitionOperation, DEDUPLICATE_ALGORITHM_SPEC_VERSION, RECONCILE_ALGORITHM_SPEC_VERSION,
};
use typesec_memory::MAX_COGNITION_ALGORITHM_BYTES;

#[test]
fn canonical_operation_names_round_trip_without_duplication() {
    for operation in [
        CognitionOperation::Deduplicate,
        CognitionOperation::Reconcile,
    ] {
        assert_eq!(
            CognitionOperation::from_str(operation.as_str()).expect("canonical operation"),
            operation
        );
        assert_eq!(operation.to_string(), operation.as_str());
    }
}

#[test]
fn operation_parsing_is_strict() {
    for invalid in [
        "deduplicate",
        "marciana.Deduplicate",
        "marciana.deduplicate ",
        "marciana.reconciliation",
        "",
    ] {
        assert!(CognitionOperation::try_from(invalid).is_err());
    }
}

#[test]
fn unsupported_operation_error_is_fixed_and_retains_no_caller_text() {
    let caller_text = format!("protected-operation-{}", "x".repeat(16 * 1024));
    let error = CognitionOperation::try_from(caller_text.as_str())
        .expect_err("unsupported operation must fail");

    assert_eq!(error.to_string(), "unsupported cognition operation");
    assert!(!error.to_string().contains("protected-operation"));
    assert_eq!(std::mem::size_of_val(&error), 0);
}

#[test]
fn native_algorithm_identity_accepts_only_exact_bounded_pairs() {
    for operation in [
        CognitionOperation::Deduplicate,
        CognitionOperation::Reconcile,
    ] {
        let spec_version = operation.algorithm_spec_version();
        assert_eq!(spec_version, "2");
        assert!(
            operation
                .is_native_algorithm(&format!("{}.reference", operation.as_str()), spec_version)
        );
        assert!(
            operation.is_native_algorithm(&format!("{}.sail", operation.as_str()), spec_version)
        );
        for (algorithm, version) in [
            (format!("{}.reference", operation.as_str()), "1".into()),
            (format!("{}.sail", operation.as_str()), "0.12.0".into()),
            (format!("{}.sail", operation.as_str()), "forged".into()),
            (
                format!("{}.unknown", operation.as_str()),
                spec_version.into(),
            ),
            (
                format!("{}\n.reference", operation.as_str()),
                spec_version.into(),
            ),
            (
                "a".repeat(MAX_COGNITION_ALGORITHM_BYTES + 1),
                spec_version.into(),
            ),
            (
                format!("{}.reference", operation.as_str()),
                "v".repeat(MAX_COGNITION_ALGORITHM_BYTES + 1),
            ),
        ] {
            assert!(!operation.is_native_algorithm(&algorithm, &version));
        }
    }
}

#[test]
fn per_operation_spec_versions_are_explicit_and_package_independent() {
    assert_eq!(DEDUPLICATE_ALGORITHM_SPEC_VERSION, "2");
    assert_eq!(RECONCILE_ALGORITHM_SPEC_VERSION, "2");
    assert_ne!(
        DEDUPLICATE_ALGORITHM_SPEC_VERSION,
        env!("CARGO_PKG_VERSION")
    );
    assert_ne!(RECONCILE_ALGORITHM_SPEC_VERSION, env!("CARGO_PKG_VERSION"));
}
