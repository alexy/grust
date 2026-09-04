use super::*;

fn key(subject: &str, purpose: &str) -> CognitionIdempotencyKey {
    CognitionIdempotencyKey::for_authority(
        "memory/user:alice/semantic",
        subject,
        purpose,
        "shared-job",
    )
    .expect("canonical key fixture")
}

#[test]
fn every_derived_graph_identity_is_authority_scoped() {
    let left = key("did:key:alice", "research");
    for right in [
        key("did:key:bob", "research"),
        key("did:key:alice", "operations"),
    ] {
        assert_ne!(job_digest(&left), job_digest(&right));
        assert_ne!(job_node_id(&left), job_node_id(&right));
        assert_ne!(commit_key_digest(&left), commit_key_digest(&right));
        assert_ne!(outcome_node_id(&left), outcome_node_id(&right));
        assert_ne!(audit_node_id(&left), audit_node_id(&right));
        assert_ne!(commit_ledger_key(&left), commit_ledger_key(&right));
        let mutation = IndexMutation::Remove(typesec_memory::MemoryId::from_string("source"));
        assert_ne!(
            outbox_node_id(&commit_key_digest(&left), 0, &mutation).expect("left outbox id"),
            outbox_node_id(&commit_key_digest(&right), 0, &mutation).expect("right outbox id")
        );
    }
}

#[test]
fn authority_scope_matching_rederives_the_verified_scope() {
    let scoped = key("did:key:alice", "research");
    assert!(authority_scope_matches(
        &scoped,
        "did:key:alice",
        "research"
    ));
    assert!(!authority_scope_matches(&scoped, "did:key:bob", "research"));
    assert!(!authority_scope_matches(
        &scoped,
        "did:key:alice",
        "operations"
    ));
}
