#![cfg(feature = "turso")]

mod support;

use support::job_key;

use chrono::Duration;
use grust_core::prelude::{GraphStore, Start, Traversal, Value};
use querygraph_memory::TursoMemoryStore;
use querygraph_memory::cognition::CognitionStateError;

use support::{at, config, digest};

#[test]
fn persisted_job_schema_names_the_exact_typedid_request_digest() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_job_wire_schema"))
        .expect("open store");
    let typedid_request_digest = digest("verified TypeDID request");
    let job = store
        .submit_cognition_job(
            &job_key("job"),
            "scheduler",
            &typedid_request_digest,
            3,
            at(0),
        )
        .expect("submit job");
    let encoded = serde_json::to_value(job).expect("serialize durable job");

    assert_eq!(encoded["schemaVersion"], serde_json::json!(2));
    assert_eq!(
        encoded["transitionedAt"],
        serde_json::to_value(at(0)).expect("fixture timestamp")
    );
    assert!(encoded.get("updatedAt").is_none());
    assert_eq!(
        encoded["typedidRequestDigest"],
        serde_json::json!(typedid_request_digest)
    );
    assert!(encoded.get("requestDigest").is_none());
}

#[test]
fn scheduler_rejects_noncanonical_job_and_owner_identities() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_canonical_identity"))
        .expect("open store");

    for job_id in ["", " job", "job ", "job\n"] {
        let key = tampered_job_key("jobId", serde_json::json!(job_id));
        assert!(matches!(
            store.submit_cognition_job(&key, "scheduler", &digest("request"), 3, at(0)),
            Err(CognitionStateError::Invalid(_))
        ));
        assert!(matches!(
            store.cognition_job(&key),
            Err(CognitionStateError::Invalid(_))
        ));
    }
    assert!(matches!(
        store.submit_cognition_job(&job_key("job"), "scheduler\n", &digest("request"), 3, at(0)),
        Err(CognitionStateError::Invalid(_))
    ));
    for (field, value) in [
        ("spaceId", serde_json::json!("memory/control\nspace")),
        (
            "authorityScopeDigest",
            serde_json::json!(format!("sha256:{}", "A".repeat(64))),
        ),
        ("authorityScopeDigest", serde_json::json!("sha256:short")),
    ] {
        assert!(matches!(
            store.cognition_job(&tampered_job_key(field, value)),
            Err(CognitionStateError::Invalid(_))
        ));
    }
}

#[test]
fn scheduler_rejects_backdated_transitions_and_shorter_renewals() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_monotonic_state"))
        .expect("open store");
    store
        .submit_cognition_job(&job_key("job"), "scheduler", &digest("request"), 3, at(2))
        .expect("submit job");
    assert!(matches!(
        store.acquire_cognition_lease(&job_key("job"), "worker", at(1), Duration::minutes(5)),
        Err(CognitionStateError::InvalidTransition(_))
    ));

    let lease = store
        .acquire_cognition_lease(&job_key("job"), "worker", at(3), Duration::minutes(5))
        .expect("acquire current lease");
    let before = store
        .cognition_job(&job_key("job"))
        .expect("load job")
        .expect("job exists");
    assert!(matches!(
        store.renew_cognition_lease(&job_key("job"), lease.token(), at(2), Duration::minutes(10)),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    assert!(matches!(
        store.fail_cognition_job(&job_key("job"), lease.token(), "worker failed", at(2)),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    assert!(matches!(
        store.cancel_cognition_job(&job_key("job"), "scheduler", at(2)),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    assert_eq!(
        store
            .cognition_job(&job_key("job"))
            .expect("reload job")
            .expect("job exists"),
        before
    );

    assert!(matches!(
        store.renew_cognition_lease(&job_key("job"), lease.token(), at(4), Duration::minutes(1)),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    let renewed = store
        .renew_cognition_lease(&job_key("job"), lease.token(), at(4), Duration::minutes(10))
        .expect("extend lease");
    assert!(renewed.expires_at() > lease.expires_at());
}

#[tokio::test]
async fn addressed_job_digest_mismatch_is_rejected_as_corrupt_state() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_job_address"))
        .expect("open store");
    store
        .submit_cognition_job(&job_key("job"), "scheduler", &digest("request"), 3, at(0))
        .expect("submit job");
    mutate_job_payload(&store, |payload| {
        payload["jobDigest"] = serde_json::json!(digest("another job"));
    })
    .await;

    assert!(matches!(
        store.cognition_job(&job_key("job")),
        Err(CognitionStateError::Backend(_))
    ));
}

#[tokio::test]
async fn unsupported_persisted_job_schema_is_rejected() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_job_schema"))
        .expect("open store");
    store
        .submit_cognition_job(&job_key("job"), "scheduler", &digest("request"), 3, at(0))
        .expect("submit job");
    mutate_job_payload(&store, |payload| {
        payload["schemaVersion"] = serde_json::json!(3);
    })
    .await;

    assert!(matches!(
        store.cognition_job(&job_key("job")),
        Err(CognitionStateError::Backend(_))
    ));
}

#[tokio::test]
async fn impossible_persisted_attempt_budget_is_rejected() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_job_invariants"))
        .expect("open store");
    store
        .submit_cognition_job(&job_key("job"), "scheduler", &digest("request"), 3, at(0))
        .expect("submit job");
    mutate_job_payload(&store, |payload| {
        payload["attempts"] = serde_json::json!(4);
    })
    .await;

    assert!(matches!(
        store.cognition_job(&job_key("job")),
        Err(CognitionStateError::Backend(message))
            if message.contains("attempt counters")
    ));
}

async fn mutate_job_payload(store: &TursoMemoryStore, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut nodes = store
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionJob".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read job nodes");
    let mut node = nodes.pop().expect("job node");
    let Value::Json(payload) = node.props.get_mut("payload").expect("job payload") else {
        panic!("job payload is JSON")
    };
    mutate(payload);
    store
        .graph()
        .put_node(&node)
        .await
        .expect("tamper job fixture");
}

fn tampered_job_key(
    field: &str,
    value: serde_json::Value,
) -> typesec_memory::CognitionIdempotencyKey {
    let mut encoded = serde_json::to_value(job_key("valid-job")).expect("serialize key fixture");
    encoded[field] = value;
    serde_json::from_value(encoded).expect("deserialize intentionally invalid key")
}
