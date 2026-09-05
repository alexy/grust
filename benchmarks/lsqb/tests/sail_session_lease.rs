//! Qualification probe only; does not enable shared-session matrix execution.
#![cfg(feature = "sail")]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use grust_core::{Graph, GraphAdminStore, GraphStore, Value};
use grust_cypher::CypherParameters;
use grust_sail::{SailConfig, SailGraphStore, SailWarehouse};

const SESSION: &str = "GRUST_SAIL_LEASE_PROBE_SESSION";

fn worker(config: &SailConfig, released: bool) -> Result<(), String> {
    let mut child = Command::new(std::env::current_exe().map_err(|_| "locate probe")?)
        .args([
            "--ignored",
            "--exact",
            "borrowed_session_worker",
            "--nocapture",
        ])
        .env(SESSION, &config.session_id)
        .env(
            "GRUST_SAIL_LEASE_RELEASED",
            if released { "1" } else { "0" },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "spawn lease probe worker")?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().map_err(|_| "wait lease probe worker")? {
            return if status.success() {
                Ok(())
            } else {
                Err("lease probe worker failed".into())
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("lease probe worker exceeded 30-second deadline".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn endpoint() -> String {
    assert_eq!(
        std::env::var("GRUST_SAIL_LEASE_DISPOSABLE").as_deref(),
        Ok("1"),
        "explicit disposable-server opt-in required"
    );
    std::env::var("SAIL_ENDPOINT").expect("explicit SAIL_ENDPOINT required")
}

#[tokio::test]
#[ignore = "private child entrypoint; run coordinator_owned_session_survives_fresh_workers instead"]
async fn borrowed_session_worker() {
    let config = SailConfig {
        endpoint: endpoint(),
        session_id: std::env::var(SESSION).expect("coordinator session required"),
        warehouse: SailWarehouse::ServerManaged,
        ..SailConfig::default()
    };
    let store = SailGraphStore::connect(config)
        .await
        .expect("attach session");
    let result = store
        .run_read_query(
            "MATCH (n:LeaseProbe) RETURN count(n)",
            &CypherParameters::new(),
        )
        .await;
    if std::env::var("GRUST_SAIL_LEASE_RELEASED").as_deref() == Ok("1") {
        assert!(
            result.is_err(),
            "released owner session must not reconnect to its graph"
        );
    } else {
        let table = result.expect("borrowed session must support the benchmark read path");
        assert_eq!(table.rows, vec![vec![Value::from(1i64)]]);
        let graph = store.read_graph().await.expect("read shared graph");
        assert_eq!(graph.nodes.len(), 1);
        assert_eq!(graph.nodes[0].id.as_str(), "lease-probe-node");
    }
    // Deliberately drop without ReleaseSession: only the coordinator owns it.
}

#[tokio::test]
#[ignore = "requires an explicitly opted-in disposable Sail server; no benchmark may run concurrently"]
async fn coordinator_owned_session_survives_fresh_workers() {
    let config = SailConfig {
        endpoint: endpoint(),
        ..SailConfig::default()
    };
    let owner = SailGraphStore::connect(config.clone())
        .await
        .expect("connect owner");
    let work = tokio::time::timeout(Duration::from_secs(60), async {
        owner
            .bootstrap()
            .await
            .map_err(|_| "bootstrap owned session")?;
        let mut builder = Graph::builder();
        let _ = builder.node("LeaseProbe", "lease-probe-node").finish();
        owner
            .put_graph(&builder.build())
            .await
            .map_err(|_| "load probe graph")?;
        worker(&config, false)?;
        worker(&config, false)?;
        let graph = owner
            .read_graph()
            .await
            .map_err(|_| "owner lost shared graph")?;
        if graph.nodes.len() != 1 {
            return Err("owner graph changed".to_string());
        }
        Ok::<_, String>(())
    })
    .await;
    // Attempt release even after qualification failure. Do not silently enable
    // shared sessions until both borrower visibility and owner cleanup pass.
    owner.close().await.expect("release coordinator session");
    work.expect("probe setup deadline")
        .expect("shared session qualification");
    worker(&config, true).expect("fresh worker must reject released session");
    let healthy = SailGraphStore::connect(SailConfig {
        endpoint: endpoint(),
        ..SailConfig::default()
    })
    .await
    .expect("connect independent health session");
    let response = healthy.query_arrow_ipc("SELECT 42").await;
    healthy.close().await.expect("release health session");
    assert!(
        response.is_ok(),
        "server remains responsive after owner release"
    );
}
