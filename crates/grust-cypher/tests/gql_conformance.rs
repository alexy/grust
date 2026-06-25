//! Conformance-corpus integration test (Unit 1 of `docs/GQL_GOAL.md`).
//!
//! Validates that every manifest under `tests/gql/*.json` parses, that each case
//! references a feature in the [`GqlFeature`] taxonomy (enforced by deserialize),
//! that `Rejected` cases carry an `errorKind`, and that case ids are unique
//! across the corpus. Execution of the cases against a backend is deferred to
//! Units 6/12; this test guards corpus integrity and keeps the manifest honest.

use std::fs;
use std::path::PathBuf;

use grust_cypher::{load_manifest, support_summary, GqlExpectation, GqlFeature};

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("gql")
}

fn manifest_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(manifest_dir())
        .expect("tests/gql directory must exist")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort();
    files
}

#[test]
fn corpus_directory_has_manifests() {
    let files = manifest_files();
    assert!(
        !files.is_empty(),
        "expected at least one manifest file under tests/gql/"
    );
}

#[test]
fn every_manifest_parses_and_is_consistent() {
    let mut total_cases = 0usize;
    let mut global_ids = std::collections::BTreeSet::new();
    for path in manifest_files() {
        let json = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let manifest = load_manifest(&json)
            .unwrap_or_else(|e| panic!("manifest {} failed to load: {e}", path.display()));
        assert!(
            !manifest.cases.is_empty(),
            "manifest {} has no cases",
            path.display()
        );
        for case in &manifest.cases {
            // Feature id already validated by deserialize; confirm round-trip.
            assert_eq!(GqlFeature::from_id(case.feature.id()), Some(case.feature));
            if case.expectation == GqlExpectation::Rejected {
                assert!(
                    case.error_kind.is_some(),
                    "rejected case {} in {} must carry an errorKind",
                    case.id,
                    path.display()
                );
            }
            let global_id = format!("{}::{}", path.file_stem().unwrap().to_string_lossy(), case.id);
            assert!(
                global_ids.insert(global_id.clone()),
                "duplicate global case id {global_id}"
            );
            total_cases += 1;
        }
    }
    assert!(
        total_cases >= 15,
        "expected a meaningful corpus, found only {total_cases} cases"
    );
}

#[test]
fn support_summary_generates() {
    // The "print or generate a current support summary" deliverable is callable
    // from the public crate surface.
    let summary = support_summary();
    assert!(summary.contains("Grust GQL/Cypher Support Summary"));
    assert!(summary.contains("strict-write"));
}
