#[cfg(any(
    feature = "helix",
    feature = "ladybug",
    feature = "lancedb",
    feature = "pggraph",
    feature = "postgres-pgq",
    feature = "surreal"
))]
mod materialize;

#[cfg(feature = "falkor")]
mod falkor;

#[cfg(any(feature = "helix", feature = "surreal"))]
use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grust_core::{Graph, GraphAdminStore, GraphStore, Value};
use grust_cypher::pushdown::{NoTypeHints, SqlDialect, plan_read};
use grust_cypher::{CypherParameters, CypherResultTable};
use grust_postgres_core::{PostgresGraphConfig, PostgresGraphStore, PostgresReadDialect};
use grust_turso::{TursoGraphStore, TursoReadDialect};

#[cfg(feature = "falkor")]
use grust_falkor::{FalkorConfig, FalkorGraphStore};
#[cfg(feature = "helix")]
use grust_helix::{HelixHttpConfig, HelixHttpGraphStore};
#[cfg(feature = "ladybug")]
use grust_ladybug::LadybugGraphStore;
#[cfg(feature = "lancedb")]
use grust_lancedb::{LanceDbConfig, LanceDbGraphStore};
#[cfg(feature = "pggraph")]
use grust_pggraph::{PgGraphConfig, PgGraphStore};
#[cfg(feature = "postgres-pgq")]
use grust_postgres_pgq::{PostgresPgqConfig, PostgresPgqStore};
#[cfg(feature = "sail")]
use grust_sail::{SailConfig, SailGraphStore};
#[cfg(feature = "surreal")]
use grust_surreal::{SurrealConfig, SurrealHttpGraphStore};

use crate::queries::QueryCase;
use crate::report::{BackendIdentityV2, ExecutionClass, ExecutionDescriptorV2};

pub struct PreparedBackend {
    inner: Backend,
    pub load_ns: u64,
}

#[derive(Debug)]
pub enum QueryExecutionError {
    Timeout(String),
    Error(String),
}

impl From<String> for QueryExecutionError {
    fn from(error: String) -> Self {
        Self::Error(error)
    }
}

enum Backend {
    Memory(Arc<Graph>),
    Turso(TursoGraphStore),
    Postgres(PostgresGraphStore),
    #[cfg(feature = "ladybug")]
    Ladybug(LadybugGraphStore),
    #[cfg(feature = "falkor")]
    Falkor {
        client: falkor::SharedFalkorNativeClient,
    },
    #[cfg(feature = "surreal")]
    Surreal(SurrealHttpGraphStore),
    #[cfg(feature = "lancedb")]
    LanceDb {
        store: LanceDbGraphStore,
        _directory: tempfile::TempDir,
    },
    #[cfg(feature = "sail")]
    Sail(SailGraphStore),
    #[cfg(feature = "pggraph")]
    PgGraph(PgGraphStore),
    #[cfg(feature = "postgres-pgq")]
    PostgresPgq(PostgresPgqStore),
    #[cfg(feature = "helix")]
    Helix(HelixHttpGraphStore),
}

impl PreparedBackend {
    pub async fn prepare(id: &str, graph: &Graph) -> Result<Self, String> {
        match id {
            "memory" => prepare_memory(graph).await,
            "turso" => prepare_turso(graph).await,
            "postgres" => prepare_postgres(graph).await,
            #[cfg(feature = "ladybug")]
            "ladybug" => prepare_ladybug(graph).await,
            #[cfg(feature = "falkor")]
            "falkor" => prepare_falkor(graph).await,
            #[cfg(feature = "surreal")]
            "surreal" => prepare_surreal(graph).await,
            #[cfg(feature = "lancedb")]
            "lancedb" => prepare_lancedb(graph).await,
            #[cfg(feature = "sail")]
            "sail" => prepare_sail(graph).await,
            #[cfg(feature = "pggraph")]
            "pggraph" => prepare_pggraph(graph).await,
            #[cfg(feature = "postgres-pgq")]
            "postgres-pgq" => prepare_postgres_pgq(graph).await,
            #[cfg(feature = "helix")]
            "helix" => prepare_helix(graph).await,
            _ => Err(format!("backend {id:?} is not compiled into this runner")),
        }
    }

    /// Loads a projected-FK dataset incrementally into backends whose query
    /// path does not need to retain the decoded source graph.
    pub async fn prepare_projected_chunks<I>(id: &str, chunks: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = Result<Graph, String>>,
    {
        match id {
            "turso" => prepare_turso_chunks(chunks).await,
            "postgres" => prepare_postgres_chunks(chunks).await,
            #[cfg(feature = "sail")]
            "sail" => prepare_sail_chunks(chunks).await,
            _ => Err(format!(
                "backend {id:?} does not expose streamed projected-FK preparation"
            )),
        }
    }

    pub fn execution(&self, case: &QueryCase) -> Result<ExecutionDescriptorV2, String> {
        let (class, language, transport) = match &self.inner {
            Backend::Memory(_) => (
                ExecutionClass::InProcessReference,
                "Grust portable Cypher",
                "in-process",
            ),
            Backend::Turso(_) => (
                sql_execution_class(case, &TursoReadDialect::new("grust"))?,
                "Grust portable Cypher",
                "embedded",
            ),
            Backend::Postgres(_) => {
                let config = postgres_config();
                (
                    sql_execution_class(case, &PostgresReadDialect::new(&config))?,
                    "Grust portable Cypher",
                    "PostgreSQL wire",
                )
            }
            #[cfg(feature = "falkor")]
            Backend::Falkor { .. } => (
                ExecutionClass::BackendNativeAggregate,
                "FalkorDB openCypher",
                "Redis GRAPH.RO_QUERY",
            ),
            #[cfg(feature = "sail")]
            Backend::Sail(_) => (
                sql_execution_class(case, &grust_cypher::pushdown::SparkDialect)?,
                "Grust portable Cypher",
                "Spark Connect",
            ),
            #[cfg(feature = "ladybug")]
            Backend::Ladybug(_) => materialized_execution(),
            #[cfg(feature = "surreal")]
            Backend::Surreal(_) => materialized_execution(),
            #[cfg(feature = "lancedb")]
            Backend::LanceDb { .. } => materialized_execution(),
            #[cfg(feature = "pggraph")]
            Backend::PgGraph(_) => materialized_execution(),
            #[cfg(feature = "postgres-pgq")]
            Backend::PostgresPgq(_) => materialized_execution(),
            #[cfg(feature = "helix")]
            Backend::Helix(_) => materialized_execution(),
        };
        Ok(ExecutionDescriptorV2 {
            class: Some(class),
            language: language.to_string(),
            transport: transport.to_string(),
            backend_query_sha256: match class {
                ExecutionClass::BackendNativeAggregate => {
                    Some(crate::queries::sha256(self.backend_query(case)?.as_bytes()))
                }
                _ => None,
            },
        })
    }

    pub async fn execute_count(
        &self,
        case: &QueryCase,
        source: &Graph,
        timeout_ms: u64,
    ) -> Result<i64, QueryExecutionError> {
        let _ = source;
        let _ = timeout_ms;
        match &self.inner {
            Backend::Memory(graph) => {
                let graph = Arc::clone(graph);
                let case = case.clone();
                blocking_count_with_timeout(timeout_ms, move || {
                    portable_count(graph.as_ref(), &case)
                })
                .await
            }
            Backend::Turso(store) => {
                let table = query_result(
                    store
                        .run_read_query(&case.executable, &CypherParameters::new())
                        .await,
                )?;
                query_result(count_from(table))
            }
            Backend::Postgres(store) => {
                let table = query_result(
                    store
                        .run_read_query(&case.executable, &CypherParameters::new())
                        .await,
                )?;
                query_result(count_from(table))
            }
            #[cfg(feature = "ladybug")]
            Backend::Ladybug(store) => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
            #[cfg(feature = "falkor")]
            Backend::Falkor { client } => falkor::execute_count(client, case, timeout_ms).await,
            #[cfg(feature = "surreal")]
            Backend::Surreal(store) => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
            #[cfg(feature = "lancedb")]
            Backend::LanceDb { store, .. } => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
            #[cfg(feature = "sail")]
            Backend::Sail(store) => {
                let table = query_result(
                    store
                        .run_read_query(&case.executable, &CypherParameters::new())
                        .await,
                )?;
                query_result(count_from(table))
            }
            #[cfg(feature = "pggraph")]
            Backend::PgGraph(store) => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
            #[cfg(feature = "postgres-pgq")]
            Backend::PostgresPgq(store) => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
            #[cfg(feature = "helix")]
            Backend::Helix(store) => {
                query_result(materialize::materialized_count(store, source, case).await)
            }
        }
    }

    /// Memory owns a cooperative, quiescent blocking boundary and FalkorDB
    /// owns a server/socket deadline. Both must bypass the generic async
    /// wrapper so a timed-out operation cannot overlap the next sample.
    pub fn manages_query_timeout(&self) -> bool {
        if matches!(&self.inner, Backend::Memory(_)) {
            return true;
        }
        #[cfg(feature = "falkor")]
        if matches!(&self.inner, Backend::Falkor { .. }) {
            return true;
        }
        false
    }

    fn backend_query(&self, case: &QueryCase) -> Result<String, String> {
        let _ = case;
        match &self.inner {
            #[cfg(feature = "falkor")]
            Backend::Falkor { .. } => Ok(falkor::adapt_query(case)),
            _ => Err("backend does not use a native query adapter".to_string()),
        }
    }
}

fn query_result<T, E: std::fmt::Display>(result: Result<T, E>) -> Result<T, QueryExecutionError> {
    result.map_err(|error| QueryExecutionError::Error(error.to_string()))
}

#[cfg(any(
    feature = "helix",
    feature = "ladybug",
    feature = "lancedb",
    feature = "pggraph",
    feature = "postgres-pgq",
    feature = "surreal"
))]
fn materialized_execution() -> (ExecutionClass, &'static str, &'static str) {
    (
        ExecutionClass::BackendMaterializeRustReference,
        "Grust portable Cypher",
        "GraphStore materialization",
    )
}

async fn prepare_memory(graph: &Graph) -> Result<PreparedBackend, String> {
    let started = Instant::now();
    let graph = Arc::new(graph.clone());
    Ok(PreparedBackend {
        inner: Backend::Memory(graph),
        load_ns: elapsed_ns(started)?,
    })
}

async fn blocking_count_with_timeout<F>(
    timeout_ms: u64,
    work: F,
) -> Result<i64, QueryExecutionError>
where
    F: FnOnce() -> Result<i64, String> + Send + 'static,
{
    let mut task = tokio::task::spawn_blocking(work);
    match tokio::time::timeout(Duration::from_millis(timeout_ms), &mut task).await {
        Ok(completed) => completed
            .map_err(|error| {
                QueryExecutionError::Error(format!("blocking query task failed: {error}"))
            })
            .and_then(|result| result.map_err(QueryExecutionError::Error)),
        Err(_) => {
            // A blocking task cannot safely be aborted. Wait for it to leave
            // the shared process before returning a timeout so later samples
            // never overlap work from this one.
            let _ = task.await.map_err(|error| {
                QueryExecutionError::Error(format!(
                    "blocking query task failed while quiescing after timeout: {error}"
                ))
            })?;
            Err(QueryExecutionError::Timeout(format!(
                "exceeded {timeout_ms} ms; blocking work quiesced before the next sample"
            )))
        }
    }
}

async fn prepare_turso(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = TursoGraphStore::in_memory()
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Turso(store),
        load_ns: elapsed_ns(started)?,
    })
}

async fn prepare_turso_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let store = TursoGraphStore::in_memory()
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    put_projected_chunks(&store, chunks).await?;
    Ok(PreparedBackend {
        inner: Backend::Turso(store),
        load_ns: elapsed_ns(started)?,
    })
}

fn postgres_config() -> PostgresGraphConfig {
    PostgresGraphConfig {
        connection_string: env::var("POSTGRES_URL").unwrap_or_else(|_| {
            "host=postgres user=postgres password=postgres dbname=grust".to_string()
        }),
        table_prefix: "lsqb_matrix".to_string(),
        ..PostgresGraphConfig::default()
    }
}

async fn prepare_postgres(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = PostgresGraphStore::connect(postgres_config())
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Postgres(store),
        load_ns: elapsed_ns(started)?,
    })
}

async fn prepare_postgres_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let store = PostgresGraphStore::connect(postgres_config())
        .await
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    put_projected_chunks(&store, chunks).await?;
    Ok(PreparedBackend {
        inner: Backend::Postgres(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "ladybug")]
async fn prepare_ladybug(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = LadybugGraphStore::in_memory().map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Ladybug(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "falkor")]
async fn prepare_falkor(graph: &Graph) -> Result<PreparedBackend, String> {
    prepare_falkor_chunks(std::iter::once(Ok(graph.clone()))).await
}

#[cfg(feature = "falkor")]
pub async fn prepare_falkor_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let redis_url = env::var("FALKOR_URL").unwrap_or_else(|_| "redis://falkor:6379".to_string());
    let graph_name = env::var("FALKOR_GRAPH").unwrap_or_else(|_| "lsqb_matrix".to_string());
    let store = FalkorGraphStore::new(FalkorConfig {
        redis_url: redis_url.clone(),
        graph: graph_name.clone(),
        batch_size: 1_000,
        ..FalkorConfig::default()
    });
    let mut native = falkor::FalkorNativeClient::connect(&redis_url, &graph_name)?;
    let started = Instant::now();
    store.clear().await.map_err(|err| err.to_string())?;
    let mut indexed = false;
    let mut loading_edges = false;
    for chunk in chunks {
        let chunk = chunk?;
        if !chunk.nodes.is_empty() {
            if loading_edges {
                return Err("Falkor LSQB chunks must load all nodes before edges".to_string());
            }
            let physical = falkor::prepare_graph(&Graph::new(chunk.nodes, Vec::new()))?;
            store
                .put_graph(&physical)
                .await
                .map_err(|err| err.to_string())?;
            if !indexed {
                native.create_entity_index()?;
                indexed = true;
            }
        }
        if !chunk.edges.is_empty() {
            if !indexed {
                return Err("Falkor LSQB edge chunks require at least one node chunk".to_string());
            }
            loading_edges = true;
            native.put_edges(&chunk.edges)?;
        }
    }
    let load_ns = elapsed_ns(started)?;
    Ok(PreparedBackend {
        inner: Backend::Falkor {
            client: native.into_shared(),
        },
        load_ns,
    })
}

#[cfg(feature = "surreal")]
async fn prepare_surreal(graph: &Graph) -> Result<PreparedBackend, String> {
    let config = SurrealConfig {
        url: env::var("SURREAL_URL").unwrap_or_else(|_| "http://surreal:8000/sql".to_string()),
        namespace: "lsqb".to_string(),
        database: "matrix".to_string(),
        labels: node_labels(graph),
        relationships: edge_labels(graph),
        ..SurrealConfig::default()
    };
    let store = SurrealHttpGraphStore::connect(config).map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Surreal(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "lancedb")]
async fn prepare_lancedb(graph: &Graph) -> Result<PreparedBackend, String> {
    let directory = tempfile::Builder::new()
        .prefix("grust-lsqb-lancedb-")
        .tempdir()
        .map_err(|err| err.to_string())?;
    let store = LanceDbGraphStore::connect(LanceDbConfig {
        uri: directory.path().to_string_lossy().into_owned(),
        table_prefix: "lsqb_matrix".to_string(),
        ..LanceDbConfig::default()
    })
    .await
    .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::LanceDb {
            store,
            _directory: directory,
        },
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "sail")]
async fn prepare_sail(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = SailGraphStore::connect(SailConfig {
        endpoint: env::var("SAIL_ENDPOINT").unwrap_or_else(|_| "http://sail:50051".to_string()),
        ..SailConfig::default()
    })
    .await
    // External endpoints may contain credentials. The adapter itself redacts
    // transport failures, and the benchmark boundary deliberately provides a
    // second defense so report details can never serialize the endpoint.
    .map_err(|_| "connect to Sail failed".to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Sail(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "sail")]
async fn prepare_sail_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let store = SailGraphStore::connect(SailConfig {
        endpoint: env::var("SAIL_ENDPOINT").unwrap_or_else(|_| "http://sail:50051".to_string()),
        ..SailConfig::default()
    })
    .await
    .map_err(|_| "connect to Sail failed".to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    put_projected_chunks(&store, chunks).await?;
    Ok(PreparedBackend {
        inner: Backend::Sail(store),
        load_ns: elapsed_ns(started)?,
    })
}

async fn put_projected_chunks<S, I>(store: &S, chunks: I) -> Result<(), String>
where
    S: GraphStore,
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let mut loading_edges = false;
    for chunk in chunks {
        let chunk = chunk.map_err(|error| format!("dataset.load: {error}"))?;
        if !chunk.nodes.is_empty() && loading_edges {
            return Err(
                "dataset.load: projected-FK chunks must load all nodes before edges".to_string(),
            );
        }
        loading_edges |= !chunk.edges.is_empty();
        store
            .put_graph(&chunk)
            .await
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[cfg(feature = "pggraph")]
async fn prepare_pggraph(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = PgGraphStore::connect(PgGraphConfig {
        connection_string: env::var("PGGRAPH_URL").unwrap_or_else(|_| {
            "host=pggraph user=postgres password=postgres dbname=graph".to_string()
        }),
        table_prefix: "lsqb_matrix".to_string(),
        auto_build: true,
        ..PgGraphConfig::default()
    })
    .await
    .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::PgGraph(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "postgres-pgq")]
async fn prepare_postgres_pgq(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = PostgresPgqStore::connect(PostgresPgqConfig {
        connection_string: env::var("POSTGRES_PGQ_URL").unwrap_or_else(|_| {
            "host=postgres-pgq user=postgres password=postgres dbname=graph".to_string()
        }),
        table_prefix: "lsqb_matrix".to_string(),
        graph_name: "lsqb_matrix_graph".to_string(),
        ..PostgresPgqConfig::default()
    })
    .await
    .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.bootstrap().await.map_err(|err| err.to_string())?;
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::PostgresPgq(store),
        load_ns: elapsed_ns(started)?,
    })
}

#[cfg(feature = "helix")]
async fn prepare_helix(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = HelixHttpGraphStore::connect(HelixHttpConfig {
        query_url: env::var("HELIX_QUERY_URL")
            .unwrap_or_else(|_| "http://helix:8080/v1/query".to_string()),
        labels: node_labels(graph),
        ..HelixHttpConfig::default()
    })
    .map_err(|err| err.to_string())?;
    let started = Instant::now();
    store.clear().await.map_err(|err| err.to_string())?;
    store
        .put_graph(graph)
        .await
        .map_err(|err| err.to_string())?;
    Ok(PreparedBackend {
        inner: Backend::Helix(store),
        load_ns: elapsed_ns(started)?,
    })
}

pub fn identity(id: &str, adapter: &str) -> BackendIdentityV2 {
    let prefix = id.replace('-', "_").to_ascii_uppercase();
    BackendIdentityV2 {
        name: id.to_string(),
        adapter: adapter.to_string(),
        adapter_version: adapter_version(id).to_string(),
        runner_image: env::var("BENCHMARK_IMAGE").ok(),
        runner_image_id: env::var("BENCHMARK_IMAGE_ID").ok(),
        resource_components: env::var("BENCHMARK_RESOURCE_COMPONENTS")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or_else(|| resource_components(id)),
        service_version: nonempty_env(format!("{prefix}_VERSION")),
        image: nonempty_env(format!("{prefix}_IMAGE")),
        image_id: nonempty_env(format!("{prefix}_IMAGE_ID")),
        worker_threads: env::var(format!("{prefix}_WORKER_THREADS"))
            .ok()
            .and_then(|value| value.parse().ok()),
    }
}

fn adapter_version(id: &str) -> &'static str {
    // Crayfish is an intentionally scoped registry patch: only the Sail and
    // Surreal adapters and facade moved to 0.13.1. The internal benchmark
    // runner stays at 0.13.0 with every unchanged adapter, so its own package
    // version is the correct default for the remaining adapter identities.
    match id {
        "sail" | "surreal" => "0.13.1",
        _ => env!("CARGO_PKG_VERSION"),
    }
}

fn nonempty_env(name: String) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn resource_components(id: &str) -> u32 {
    if matches!(
        id,
        "postgres" | "falkor" | "surreal" | "sail" | "pggraph" | "postgres-pgq" | "helix"
    ) {
        2
    } else {
        1
    }
}

fn sql_execution_class(
    case: &QueryCase,
    dialect: &dyn SqlDialect,
) -> Result<ExecutionClass, String> {
    let plan = plan_read(&case.executable, &CypherParameters::new(), &NoTypeHints)
        .map_err(|err| err.to_string())?;
    Ok(if plan.is_some_and(|plan| plan.supported_by(dialect)) {
        ExecutionClass::BackendRowSourceRustProjection
    } else {
        ExecutionClass::BackendMaterializeRustReference
    })
}

/// Classifies a portable query without connecting to or loading a backend.
///
/// Setup failures and deliberately unavailable external services still need a
/// truthful per-query execution class. The SQL/Spark planners are pure, so the
/// same classification used by a prepared backend can be computed before any
/// service exists.
pub fn portable_execution_class(id: &str, case: &QueryCase) -> Result<ExecutionClass, String> {
    match id {
        "memory" => Ok(ExecutionClass::InProcessReference),
        "turso" => sql_execution_class(case, &TursoReadDialect::new("grust")),
        "postgres" => {
            let config = postgres_config();
            sql_execution_class(case, &PostgresReadDialect::new(&config))
        }
        #[cfg(feature = "sail")]
        "sail" => sql_execution_class(case, &grust_cypher::pushdown::SparkDialect),
        _ => Err(format!(
            "backend {id:?} has no compiled portable-query classifier"
        )),
    }
}

fn portable_count(graph: &Graph, case: &QueryCase) -> Result<i64, String> {
    count_from(
        grust_cypher::read::run_read_query(graph, &case.executable, &CypherParameters::new())
            .map_err(|err| err.to_string())?,
    )
}

pub(crate) fn count_from(table: CypherResultTable) -> Result<i64, String> {
    if table.rows.len() != 1 || table.rows[0].len() != 1 {
        return Err(format!(
            "expected one result cell, got {} columns and {} rows",
            table.columns.len(),
            table.rows.len()
        ));
    }
    match &table.rows[0][0] {
        Value::Int(value) => Ok(*value),
        value => Err(format!("expected integer count, got {value:?}")),
    }
}

#[cfg(any(feature = "helix", feature = "surreal"))]
fn node_labels(graph: &Graph) -> Vec<String> {
    graph
        .nodes
        .iter()
        .map(|node| node.label.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(feature = "surreal")]
fn edge_labels(graph: &Graph) -> Vec<String> {
    graph
        .edges
        .iter()
        .map(|edge| edge.label.as_str().to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn elapsed_ns(started: Instant) -> Result<u64, String> {
    u64::try_from(started.elapsed().as_nanos())
        .map_err(|_| "elapsed duration exceeded u64 nanoseconds".to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use grust_core::{Edge, EdgeQuery, Graph, GraphStore, Node, NodeId, Props};
    use grust_memory::MemoryGraphStore;

    use super::{
        QueryExecutionError, adapter_version, blocking_count_with_timeout, put_projected_chunks,
        resource_components,
    };

    #[tokio::test]
    async fn projected_chunks_are_loaded_incrementally_in_node_edge_order() {
        let store = MemoryGraphStore::new();
        let nodes = Graph::new(
            vec![
                Node::new("Person", "a", Props::new()),
                Node::new("Person", "b", Props::new()),
            ],
            Vec::new(),
        );
        let edges = Graph::new(Vec::new(), vec![Edge::new("KNOWS", "a", "b", Props::new())]);

        put_projected_chunks(&store, [Ok(nodes.clone()), Ok(edges.clone())])
            .await
            .expect("node-first chunks load");
        assert!(
            store
                .get_node(&NodeId::new("a"))
                .await
                .expect("read node")
                .is_some()
        );
        assert_eq!(
            store
                .get_edges(EdgeQuery::default())
                .await
                .expect("read edges")
                .len(),
            1
        );

        let out_of_order = MemoryGraphStore::new();
        let error = put_projected_chunks(&out_of_order, [Ok(edges), Ok(nodes)])
            .await
            .expect_err("node chunks after edges must fail");
        assert_eq!(
            error,
            "dataset.load: projected-FK chunks must load all nodes before edges"
        );
    }

    #[tokio::test]
    async fn slow_blocking_timeout_waits_for_quiescence() {
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = Arc::clone(&finished);
        let started = Instant::now();

        let result = blocking_count_with_timeout(1, move || {
            std::thread::sleep(Duration::from_millis(30));
            worker_finished.store(true, Ordering::SeqCst);
            Ok(1)
        })
        .await;

        assert!(matches!(result, Err(QueryExecutionError::Timeout(_))));
        assert!(finished.load(Ordering::SeqCst));
        assert!(started.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn resource_components_distinguish_external_services() {
        for backend in [
            "postgres",
            "falkor",
            "surreal",
            "sail",
            "pggraph",
            "postgres-pgq",
            "helix",
        ] {
            assert_eq!(resource_components(backend), 2, "{backend}");
        }
        for backend in ["memory", "turso", "ladybug", "lancedb", "cocoindex"] {
            assert_eq!(resource_components(backend), 1, "{backend}");
        }
    }

    #[test]
    fn scoped_patch_reports_the_actual_adapter_versions() {
        assert_eq!(adapter_version("sail"), "0.13.1");
        assert_eq!(adapter_version("surreal"), "0.13.1");
        assert_eq!(adapter_version("memory"), "0.13.0");
    }
}
