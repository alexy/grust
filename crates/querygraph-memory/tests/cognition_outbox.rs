#![cfg(feature = "turso")]

mod support;

use std::sync::{Arc, Barrier};

use chrono::Duration;
use grust_core::prelude::{GraphStore, NodeId, Start, Traversal, Value};
use querygraph_memory::TursoMemoryStore;
use querygraph_memory::cognition::{
    CognitionStateError, MAX_COGNITION_BEARER_TOKEN_BYTES, MAX_COGNITION_OUTBOX_CLAIM,
};
use typesec_memory::{IndexMutation, MemoryId};

use support::cognition_vault::CognitionFixture;
use support::{at, config, job_key, record};

#[test]
fn failed_delivery_recovers_after_reopen_and_ack_is_idempotent() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_outbox_recovery");
    let first = {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let source = record("source", "private source text", None);
        let fixture = CognitionFixture::new(store, source, "job");
        fixture.apply().expect("apply cognition");
        assert!(
            fixture
                .store()
                .claim_cognition_outbox(
                    &job_key("job"),
                    "worker/failed",
                    at(3),
                    Duration::minutes(61),
                    10
                )
                .is_err()
        );
        fixture
            .store()
            .claim_cognition_outbox(
                &job_key("job"),
                "worker/failed",
                at(3),
                Duration::seconds(1),
                10,
            )
            .expect("claim first delivery")
            .into_iter()
            .next()
            .expect("outbox work")
    };

    let reopened = TursoMemoryStore::open_with_config(config.clone()).expect("reopen store");
    let recovered = reopened
        .claim_cognition_outbox(
            &job_key("job"),
            "worker/recovery",
            at(5),
            Duration::minutes(1),
            10,
        )
        .expect("reclaim expired delivery")
        .into_iter()
        .next()
        .expect("recovered outbox work");
    assert_eq!(recovered.entry_id(), first.entry_id());
    assert_eq!(recovered.mutation(), first.mutation());
    assert_eq!(recovered.attempt(), 2);
    assert_ne!(recovered.token(), first.token());
    assert!(
        reopened
            .ack_cognition_outbox(recovered.entry_id(), recovered.token(), at(6))
            .expect("ack repair")
    );
    assert!(
        !reopened
            .ack_cognition_outbox(recovered.entry_id(), recovered.token(), at(6))
            .expect("idempotent acknowledgement")
    );
    drop(reopened);

    let final_store = TursoMemoryStore::open_with_config(config).expect("final reopen");
    assert!(
        final_store
            .claim_cognition_outbox(
                &job_key("job"),
                "worker/other",
                at(7),
                Duration::minutes(1),
                10
            )
            .expect("list remaining work")
            .is_empty()
    );
}

#[test]
fn concurrent_workers_cannot_claim_the_same_outbox_entry() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_outbox_exclusive");
    {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let source = record("source", "private source text", None);
        let fixture = CognitionFixture::new(store, source, "job");
        fixture.apply().expect("apply cognition");
    }

    let left = Arc::new(TursoMemoryStore::open_with_config(config.clone()).expect("left store"));
    let right = Arc::new(TursoMemoryStore::open_with_config(config).expect("right store"));
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for (store, owner) in [(left, "worker/left"), (right, "worker/right")] {
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            let mut attempt = 0;
            loop {
                attempt += 1;
                match store.claim_cognition_outbox(
                    &job_key("job"),
                    owner,
                    at(3),
                    Duration::minutes(1),
                    10,
                ) {
                    Ok(claims) => break claims,
                    Err(CognitionStateError::Backend(_)) if attempt < 16 => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("claim attempt failed: {error}"),
                }
            }
        }));
    }
    let claims: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("worker completes"))
        .collect();
    assert_eq!(claims.len(), 1);
}

#[test]
fn outbox_claims_are_bound_to_the_exact_committed_job() {
    let dir = tempfile::tempdir().expect("temporary database");
    let config = config(&dir, "cognition_outbox_jobs");
    for (job, source) in [("job-a", "source-a"), ("job-b", "source-b")] {
        let store = TursoMemoryStore::open_with_config(config.clone()).expect("open store");
        let fixture = CognitionFixture::new(store, record(source, "source text", None), job);
        fixture.apply().expect("apply cognition");
    }
    let store = TursoMemoryStore::open_with_config(config).expect("reopen store");

    let left = store
        .claim_cognition_outbox(
            &job_key("job-a"),
            "worker-a",
            at(3),
            Duration::minutes(1),
            10,
        )
        .expect("claim job A");
    let right = store
        .claim_cognition_outbox(
            &job_key("job-b"),
            "worker-b",
            at(3),
            Duration::minutes(1),
            10,
        )
        .expect("claim job B");
    assert_eq!(
        left[0].mutation(),
        &IndexMutation::Remove(MemoryId::from_string("source-a"))
    );
    assert_eq!(
        right[0].mutation(),
        &IndexMutation::Remove(MemoryId::from_string("source-b"))
    );
}

#[test]
fn outbox_rejects_backdated_claim_and_acknowledgement_clocks() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox_clock"))
        .expect("open store");
    let fixture =
        CognitionFixture::new(store, record("source", "private source text", None), "job");
    fixture.apply().expect("apply cognition");
    let claim = fixture
        .store()
        .claim_cognition_outbox(&job_key("job"), "worker", at(3), Duration::minutes(1), 1)
        .expect("claim outbox")
        .into_iter()
        .next()
        .expect("outbox work");

    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &job_key("job"),
            "other",
            at(2),
            Duration::minutes(1),
            1
        ),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    assert!(matches!(
        fixture
            .store()
            .ack_cognition_outbox(claim.entry_id(), claim.token(), at(2)),
        Err(CognitionStateError::InvalidTransition(_))
    ));
    assert!(
        fixture
            .store()
            .ack_cognition_outbox(claim.entry_id(), claim.token(), at(4))
            .expect("ack with monotonic time")
    );
}

#[tokio::test]
async fn corrupt_outbox_status_and_lease_state_fails_closed() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox_corrupt"))
        .expect("open store");
    let fixture =
        CognitionFixture::new(store, record("source", "private source text", None), "job");
    fixture.apply().expect("apply cognition");
    let mut nodes = fixture
        .store()
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionIndexOutbox".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read outbox nodes");
    let mut node = nodes.pop().expect("outbox node");
    let Value::Json(payload) = node.props.get_mut("payload").expect("outbox payload") else {
        panic!("outbox payload is JSON")
    };
    payload["status"] = serde_json::json!("leased");
    fixture
        .store()
        .graph()
        .put_node(&node)
        .await
        .expect("tamper outbox fixture");

    assert!(matches!(
        fixture
            .store()
            .claim_cognition_outbox(&job_key("job"), "worker", at(3), Duration::minutes(1), 1),
        Err(CognitionStateError::Backend(message))
            if message.contains("status and credential state")
    ));
}

#[tokio::test]
async fn outbox_schema_and_copied_entry_identity_fail_closed() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox_identity"))
        .expect("open store");
    let fixture =
        CognitionFixture::new(store, record("source", "private source text", None), "job");
    fixture.apply().expect("apply cognition");
    let mut nodes = fixture
        .store()
        .graph()
        .traverse(Traversal {
            start: Start::NodesByLabel("CognitionIndexOutbox".into()),
            steps: Vec::new(),
            limit: None,
        })
        .await
        .expect("read outbox nodes");
    let original = nodes.pop().expect("outbox node");

    let mut copied = original.clone();
    copied.id = NodeId::from(format!("cog-outbox:{}", "f".repeat(64)).as_str());
    fixture
        .store()
        .graph()
        .put_node(&copied)
        .await
        .expect("copy payload under another node id");
    assert!(matches!(
        fixture
            .store()
            .ack_cognition_outbox(copied.id.as_str(), "unused-token", at(3)),
        Err(CognitionStateError::Backend(_))
    ));

    let mut unsupported = original;
    let Value::Json(payload) = unsupported
        .props
        .get_mut("payload")
        .expect("outbox payload")
    else {
        panic!("outbox payload is JSON")
    };
    payload["schemaVersion"] = serde_json::json!(2);
    fixture
        .store()
        .graph()
        .put_node(&unsupported)
        .await
        .expect("tamper schema version");
    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &job_key("job"),
            "worker",
            at(3),
            Duration::minutes(1),
            1,
        ),
        Err(CognitionStateError::Backend(_))
    ));
}

#[test]
fn outbox_claim_limit_is_public_and_fail_closed() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox_limit"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");

    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &job_key("job"),
            "worker",
            at(3),
            Duration::minutes(1),
            MAX_COGNITION_OUTBOX_CLAIM + 1,
        ),
        Err(CognitionStateError::Invalid(_))
    ));
    let mut invalid_key = serde_json::to_value(job_key("job")).expect("serialize key fixture");
    invalid_key["spaceId"] = serde_json::json!("memory/control\nspace");
    let invalid_key =
        serde_json::from_value(invalid_key).expect("deserialize intentionally invalid key");
    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &invalid_key,
            "worker",
            at(3),
            Duration::minutes(1),
            1,
        ),
        Err(CognitionStateError::Invalid(_))
    ));
    assert!(matches!(
        fixture.store().claim_cognition_outbox(
            &job_key("job"),
            " worker",
            at(3),
            Duration::minutes(1),
            1,
        ),
        Err(CognitionStateError::Invalid(_))
    ));
    assert!(
        !fixture
            .store()
            .claim_cognition_outbox(
                &job_key("job"),
                "worker",
                at(3),
                Duration::minutes(1),
                MAX_COGNITION_OUTBOX_CLAIM,
            )
            .expect("public maximum is accepted")
            .is_empty()
    );
}

#[test]
fn acknowledgement_inputs_are_bounded_before_graph_access() {
    let dir = tempfile::tempdir().expect("temporary database");
    let store = TursoMemoryStore::open_with_config(config(&dir, "cognition_outbox_ack_bounds"))
        .expect("open store");
    let fixture = CognitionFixture::new(store, record("source", "source text", None), "job");
    fixture.apply().expect("apply cognition");
    let claim = fixture
        .store()
        .claim_cognition_outbox(&job_key("job"), "worker", at(3), Duration::minutes(1), 1)
        .expect("claim repair")
        .into_iter()
        .next()
        .expect("outbox work");

    assert!(matches!(
        fixture.store().ack_cognition_outbox(
            &format!("cog-outbox:{}", "a".repeat(65)),
            claim.token(),
            at(4),
        ),
        Err(CognitionStateError::Invalid(_))
    ));
    assert!(matches!(
        fixture.store().ack_cognition_outbox(
            claim.entry_id(),
            &"x".repeat(MAX_COGNITION_BEARER_TOKEN_BYTES + 1),
            at(4),
        ),
        Err(CognitionStateError::Invalid(_))
    ));
    assert!(
        fixture
            .store()
            .ack_cognition_outbox(claim.entry_id(), claim.token(), at(4))
            .expect("valid acknowledgement remains usable")
    );
}
