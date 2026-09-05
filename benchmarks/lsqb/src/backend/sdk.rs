//! Distinct network SDK lanes; existing HTTP identities remain unchanged.
//! These measure materialization plus Grust reference query execution.

use super::*;

pub(super) fn execution(transport: &'static str) -> (ExecutionClass, &'static str, &'static str) {
    (
        ExecutionClass::BackendMaterializeRustReference,
        "Grust portable Cypher",
        transport,
    )
}

#[cfg(feature = "helix")]
fn helix_config(graph: &Graph) -> grust_helix::HelixSdkConfig {
    grust_helix::HelixSdkConfig {
        // The SDK may need a different server API than the direct HTTP lane.
        base_url: env::var("HELIX_SDK_BASE_URL")
            .unwrap_or_else(|_| "http://helix-sdk:8080".to_string()),
        labels: node_labels(graph),
        ..grust_helix::HelixSdkConfig::default()
    }
}

#[cfg(feature = "helix")]
pub(super) fn attach_helix(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = grust_helix::HelixSdkGraphStore::connect(helix_config(graph))
        .map_err(|_| "connect to Helix Rust SDK failed".to_string())?;
    Ok(PreparedBackend {
        inner: Backend::HelixSdk(store),
        load_ns: 0,
    })
}

#[cfg(feature = "helix")]
pub(super) async fn prepare_helix(graph: &Graph) -> Result<PreparedBackend, String> {
    let mut prepared = attach_helix(graph)?;
    let Backend::HelixSdk(store) = &prepared.inner else {
        unreachable!("attach_helix constructs only the Helix SDK variant")
    };
    let started = Instant::now();
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    prepared.load_ns = elapsed_ns(started)?;
    Ok(prepared)
}

#[cfg(feature = "surreal")]
fn surreal_config(graph: &Graph) -> SurrealConfig {
    SurrealConfig {
        url: env::var("SURREAL_SDK_URL").unwrap_or_else(|_| "ws://surreal:8000".to_string()),
        database: "matrix_sdk".to_string(),
        ..super::surreal_config(graph)
    }
}

#[cfg(feature = "surreal")]
pub(super) async fn attach_surreal(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = grust_surreal::SurrealSdkGraphStore::connect(surreal_config(graph))
        .await
        .map_err(|_| "connect to SurrealDB Rust SDK failed".to_string())?;
    Ok(PreparedBackend {
        inner: Backend::SurrealSdk(store),
        load_ns: 0,
    })
}

#[cfg(feature = "surreal")]
pub(super) async fn prepare_surreal(graph: &Graph) -> Result<PreparedBackend, String> {
    let mut prepared = attach_surreal(graph).await?;
    let Backend::SurrealSdk(store) = &prepared.inner else {
        unreachable!("attach_surreal constructs only the Surreal SDK variant")
    };
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    prepared.load_ns = elapsed_ns(started)?;
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_transport_does_not_claim_native_query_execution() {
        for transport in ["Helix Rust SDK / HTTP", "SurrealDB Rust SDK / WebSocket"] {
            let (class, language, actual_transport) = execution(transport);
            assert_eq!(class, ExecutionClass::BackendMaterializeRustReference);
            assert_eq!(language, "Grust portable Cypher");
            assert_eq!(actual_transport, transport);
        }
    }

    #[cfg(feature = "surreal")]
    #[test]
    fn surreal_sdk_and_http_use_distinct_databases() {
        let graph = Graph::default();
        assert_ne!(
            surreal_config(&graph).database,
            super::super::surreal_config(&graph).database
        );
    }
}
