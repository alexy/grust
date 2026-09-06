#[cfg(any(
    feature = "helix",
    feature = "ladybug",
    feature = "lancedb",
    feature = "pggraph",
    feature = "postgres-pgq",
    feature = "surreal"
))]
mod materialize;

mod execution_plan;
pub use execution_plan::{
    memory_execution_plan, resident_count_plan, resident_store_execution_class, scalar_sql_query,
};

#[cfg(feature = "falkor")]
mod falkor;

#[cfg(feature = "sail")]
mod sail_session;

#[cfg(any(feature = "helix", feature = "surreal"))]
mod sdk;

#[cfg(any(feature = "helix", feature = "surreal"))]
use std::collections::BTreeSet;
use std::env;
use std::sync::Arc;
use std::time::{Duration, Instant};

use grust_core::{Graph, GraphAdminStore, GraphStore, TypedGraphIndex, Value};
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
use crate::report::{
    BackendIdentityV2, ExecutionClass, ExecutionDescriptorV2, LoadStrategyV3, RecoveryContractV3,
};

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
    Memory(Arc<TypedGraphIndex>),
    /// Turso plus the resident typed index the worker built from the store
    /// after loading it, inside `load_ns`.
    Turso {
        store: TursoGraphStore,
        resident: Arc<TypedGraphIndex>,
    },
    /// A PostgreSQL store with the resident typed index its worker built
    /// from the store's own rows before READY (`indexed_snapshot`).
    Postgres {
        store: PostgresGraphStore,
        resident: Arc<TypedGraphIndex>,
    },
    #[cfg(feature = "ladybug")]
    Ladybug(LadybugGraphStore),
    #[cfg(feature = "falkor")]
    Falkor {
        client: falkor::SharedFalkorNativeClient,
    },
    #[cfg(feature = "surreal")]
    Surreal(SurrealHttpGraphStore),
    #[cfg(feature = "surreal")]
    SurrealSdk(grust_surreal::SurrealSdkGraphStore),
    #[cfg(feature = "lancedb")]
    LanceDb {
        store: LanceDbGraphStore,
        _directory: tempfile::TempDir,
    },
    #[cfg(feature = "sail")]
    Sail(sail_session::Session),
    #[cfg(feature = "pggraph")]
    PgGraph(PgGraphStore),
    #[cfg(feature = "postgres-pgq")]
    PostgresPgq(PostgresPgqStore),
    #[cfg(feature = "helix")]
    Helix(HelixHttpGraphStore),
    #[cfg(feature = "helix")]
    HelixSdk(grust_helix::HelixSdkGraphStore),
}

impl PreparedBackend {
    pub fn configure_worker(&self, command: &mut std::process::Command) {
        let _ = command;
        #[cfg(feature = "sail")]
        if let Backend::Sail(session) = &self.inner {
            session.configure_worker(command);
        }
    }

    /// Release coordinator-owned remote state; borrowed workers only detach.
    /// Persistent service backends keep their dataset for the next attachment.
    pub async fn finish(self) -> Result<(), String> {
        #[cfg(feature = "sail")]
        if let Backend::Sail(store) = self.inner {
            return store.close().await;
        }
        Ok(())
    }

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
            #[cfg(feature = "surreal")]
            "surreal-sdk" => sdk::prepare_surreal(graph).await,
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
            #[cfg(feature = "helix")]
            "helix-sdk" => sdk::prepare_helix(graph).await,
            _ => Err(format!("backend {id:?} is not compiled into this runner")),
        }
    }

    /// Loads a projected-FK dataset directly into the backend-owned
    /// representation without first retaining a duplicate source graph.
    pub async fn prepare_projected_chunks<I>(id: &str, chunks: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = Result<Graph, String>>,
    {
        match id {
            "memory" => prepare_memory_chunks(chunks).await,
            "turso" => prepare_turso_chunks(chunks).await,
            "postgres" => prepare_postgres_chunks(chunks).await,
            #[cfg(feature = "sail")]
            "sail" => prepare_sail_chunks(chunks).await,
            _ => Err(format!(
                "backend {id:?} does not expose streamed projected-FK preparation"
            )),
        }
    }

    /// Reconnect to graph state loaded once by the coordinator without
    /// clearing or rewriting it. Only persistent service backends are listed;
    /// process-owned backends are reloaded inside every worker instead.
    pub async fn attach_existing(id: &str, source: &Graph) -> Result<Self, String> {
        let _ = source;
        match id {
            #[cfg(feature = "helix")]
            "helix-sdk" => sdk::attach_helix(source),
            #[cfg(feature = "surreal")]
            "surreal-sdk" => sdk::attach_surreal(source).await,
            #[cfg(feature = "sail")]
            "sail" => Ok(Self {
                inner: Backend::Sail(sail_session::Session::borrow().await?),
                load_ns: 0,
            }),
            "postgres" => {
                let store = PostgresGraphStore::connect(postgres_config())
                    .await
                    .map_err(|_| "connect to PostgreSQL failed".to_string())?;
                // The coordinator loaded the store once; every observation
                // worker reads it back and builds its own resident index
                // before READY, so the build lands in setup, never in a query.
                let build_started = Instant::now();
                let resident = store
                    .indexed_snapshot()
                    .await
                    .map_err(|err| format!("attach: resident index: {err}"))?;
                crate::load_progress::resident_index_built(&resident, build_started);
                Ok(Self {
                    inner: Backend::Postgres { store, resident },
                    load_ns: 0,
                })
            }
            #[cfg(feature = "falkor")]
            "falkor" => {
                let redis_url =
                    env::var("FALKOR_URL").unwrap_or_else(|_| "redis://falkor:6379".to_string());
                let graph_name =
                    env::var("FALKOR_GRAPH").unwrap_or_else(|_| "lsqb_matrix".to_string());
                Ok(Self {
                    inner: Backend::Falkor {
                        client: falkor::FalkorNativeClient::connect(&redis_url, &graph_name)?
                            .into_shared(),
                    },
                    load_ns: 0,
                })
            }
            #[cfg(feature = "surreal")]
            "surreal" => Ok(Self {
                inner: Backend::Surreal(
                    SurrealHttpGraphStore::connect(surreal_config(source))
                        .map_err(|_| "connect to SurrealDB failed".to_string())?,
                ),
                load_ns: 0,
            }),
            #[cfg(feature = "pggraph")]
            "pggraph" => Ok(Self {
                inner: Backend::PgGraph(
                    PgGraphStore::connect(pggraph_config())
                        .await
                        .map_err(|_| "connect to pgGraph failed".to_string())?,
                ),
                load_ns: 0,
            }),
            #[cfg(feature = "postgres-pgq")]
            "postgres-pgq" => Ok(Self {
                inner: Backend::PostgresPgq(
                    PostgresPgqStore::connect(postgres_pgq_config())
                        .await
                        .map_err(|_| "connect to PostgreSQL PGQ failed".to_string())?,
                ),
                load_ns: 0,
            }),
            #[cfg(feature = "helix")]
            "helix" => Ok(Self {
                inner: Backend::Helix(
                    HelixHttpGraphStore::connect(helix_config(source))
                        .map_err(|_| "connect to Helix failed".to_string())?,
                ),
                load_ns: 0,
            }),
            _ => Err(format!(
                "backend {id:?} does not have a safe attach-existing worker path"
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
            Backend::Turso { .. } => (
                resident_store_execution_class(case, &TursoReadDialect::new("grust"))?,
                "Grust portable Cypher",
                "embedded",
            ),
            Backend::Postgres { .. } => (
                resident_store_execution_class(
                    case,
                    &PostgresReadDialect::new(&postgres_config()),
                )?,
                "Grust portable Cypher",
                "PostgreSQL wire",
            ),
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
            #[cfg(feature = "surreal")]
            Backend::SurrealSdk(_) => sdk::execution("SurrealDB Rust SDK / WebSocket"),
            #[cfg(feature = "lancedb")]
            Backend::LanceDb { .. } => materialized_execution(),
            #[cfg(feature = "pggraph")]
            Backend::PgGraph(_) => materialized_execution(),
            #[cfg(feature = "postgres-pgq")]
            Backend::PostgresPgq(_) => materialized_execution(),
            #[cfg(feature = "helix")]
            Backend::Helix(_) => materialized_execution(),
            #[cfg(feature = "helix")]
            Backend::HelixSdk(_) => sdk::execution("Helix Rust SDK / HTTP"),
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
            Backend::Memory(index) => {
                let index = Arc::clone(index);
                let case = case.clone();
                blocking_count_with_timeout(timeout_ms, move || indexed_count(&index, &case)).await
            }
            Backend::Turso { store, resident } => {
                if scalar_sql_query("turso", case)?.is_none() && resident_count_plan(case)? {
                    let index = Arc::clone(resident);
                    let case = case.clone();
                    return blocking_count_with_timeout(timeout_ms, move || {
                        indexed_count(&index, &case)
                    })
                    .await;
                }
                let table = query_result(
                    store
                        .run_read_query(&case.executable, &CypherParameters::new())
                        .await,
                )?;
                query_result(count_from(table))
            }
            Backend::Postgres { store, resident } => {
                if scalar_sql_query("postgres", case)?.is_none() && resident_count_plan(case)? {
                    let index = Arc::clone(resident);
                    let case = case.clone();
                    return blocking_count_with_timeout(timeout_ms, move || {
                        indexed_count(&index, &case)
                    })
                    .await;
                }
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
            #[cfg(feature = "surreal")]
            Backend::SurrealSdk(store) => {
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
            #[cfg(feature = "helix")]
            Backend::HelixSdk(store) => {
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
            Backend::Turso { .. } => scalar_sql_query("turso", case)?
                .ok_or_else(|| "query has no native Turso count plan".to_string()),
            Backend::Postgres { .. } => scalar_sql_query("postgres", case)?
                .ok_or_else(|| "query has no native PostgreSQL count plan".to_string()),
            #[cfg(feature = "falkor")]
            Backend::Falkor { .. } => Ok(falkor::adapt_query(case)),
            _ => Err("backend does not use a native query adapter".to_string()),
        }
    }
}

pub fn load_strategy(id: &str, executed: bool) -> LoadStrategyV3 {
    if !executed {
        return LoadStrategyV3::NotExecuted;
    }
    if matches!(
        id,
        "postgres"
            | "falkor"
            | "surreal"
            | "surreal-sdk"
            | "pggraph"
            | "postgres-pgq"
            | "helix"
            | "helix-sdk"
            | "sail"
    ) {
        LoadStrategyV3::OnceWorkerAttach
    } else {
        LoadStrategyV3::PerObservationWorkerReload
    }
}

pub fn recovery_contract(id: &str, executed: bool) -> RecoveryContractV3 {
    if !executed {
        return RecoveryContractV3::NotApplicable;
    }
    match id {
        "memory" | "turso" | "ladybug" | "lancedb" => RecoveryContractV3::ProcessGroupAbsent,
        "postgres" | "pggraph" | "postgres-pgq" => RecoveryContractV3::PostgresSessionAbsent,
        "falkor" => RecoveryContractV3::FalkorServerDeadline,
        "sail" | "surreal" | "helix" => RecoveryContractV3::FailClosed,
        _ => RecoveryContractV3::FailClosed,
    }
}

/// Leave a small, deterministic part of the coordinator deadline for
/// FalkorDB to return its native TIMEOUT acknowledgement and reconnect. The
/// reserve is ten percent for short cutoffs and never exceeds five seconds,
/// so successful-query timing remains governed by the common hard deadline.
/// One second was not enough at SF0.1: FalkorDB checks its TIMEOUT between
/// operators and overshot it on q9, so the coordinator killed the worker
/// unacknowledged and the cell could not prove quiescence.
pub fn worker_query_timeout_ms(id: &str, coordinator_timeout_ms: u64) -> u64 {
    if id != "falkor" || coordinator_timeout_ms <= 1 {
        return coordinator_timeout_ms;
    }
    let acknowledgement_reserve_ms = (coordinator_timeout_ms / 10).clamp(1, 5_000);
    coordinator_timeout_ms
        .saturating_sub(acknowledgement_reserve_ms)
        .max(1)
}

/// Prove server-side quiescence after a forced termination or an
/// unacknowledged worker query error.
///
/// A process-owned backend needs no second proof after its process group is
/// absent. Remote backends without a cancellation/introspection contract fail
/// closed rather than allowing a potentially overlapping next observation.
pub async fn recover_after_unacknowledged_exit(
    id: &str,
    worker_token: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    match recovery_contract(id, true) {
        RecoveryContractV3::ProcessGroupAbsent => Ok(()),
        RecoveryContractV3::PostgresSessionAbsent => {
            let connection = match id {
                "postgres" => postgres_base_connection(),
                "pggraph" => pggraph_base_connection(),
                "postgres-pgq" => postgres_pgq_base_connection(),
                _ => unreachable!(),
            };
            wait_for_postgres_session_absence(&connection, worker_token, timeout_ms).await
        }
        RecoveryContractV3::FalkorServerDeadline => Err(
            "forced FalkorDB worker termination has no server-side quiescence proof".to_string(),
        ),
        RecoveryContractV3::FailClosed => Err(format!(
            "backend {id} cannot prove server-side quiescence after forced worker termination"
        )),
        RecoveryContractV3::NotApplicable => Ok(()),
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
    let index = TypedGraphIndex::new(Arc::new(graph.clone()))
        .map_err(|error| format!("dataset.load: {error}"))?;
    Ok(PreparedBackend {
        inner: Backend::Memory(Arc::new(index)),
        load_ns: elapsed_ns(started)?,
    })
}

async fn prepare_memory_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    // Start before advancing the lazy iterator so CSV decode is part of the
    // diagnostic load interval. Moving each chunk into this graph avoids the
    // full deep clone used by the already-decoded example path.
    let started = Instant::now();
    let mut graph = Graph::default();
    let mut loading_edges = false;
    for chunk in chunks {
        let chunk = chunk.map_err(|error| format!("dataset.load: {error}"))?;
        if !chunk.nodes.is_empty() && loading_edges {
            return Err(
                "dataset.load: projected-FK chunks must load all nodes before edges".to_string(),
            );
        }
        loading_edges |= !chunk.edges.is_empty();
        graph.nodes.extend(chunk.nodes);
        graph.edges.extend(chunk.edges);
    }
    let index =
        TypedGraphIndex::new(Arc::new(graph)).map_err(|error| format!("dataset.load: {error}"))?;
    Ok(PreparedBackend {
        inner: Backend::Memory(Arc::new(index)),
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
    let build_started = Instant::now();
    let resident = store
        .indexed_snapshot()
        .await
        .map_err(|err| format!("dataset.load: resident index: {err}"))?;
    crate::load_progress::resident_index_built(&resident, build_started);
    Ok(PreparedBackend {
        inner: Backend::Turso { store, resident },
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
    let build_started = Instant::now();
    let resident = store
        .indexed_snapshot()
        .await
        .map_err(|err| format!("dataset.load: resident index: {err}"))?;
    crate::load_progress::resident_index_built(&resident, build_started);
    Ok(PreparedBackend {
        inner: Backend::Turso { store, resident },
        load_ns: elapsed_ns(started)?,
    })
}

fn postgres_config() -> PostgresGraphConfig {
    PostgresGraphConfig {
        connection_string: worker_postgres_connection(postgres_base_connection()),
        table_prefix: "lsqb_matrix".to_string(),
        ..PostgresGraphConfig::default()
    }
}

fn postgres_base_connection() -> String {
    env::var("POSTGRES_URL").unwrap_or_else(|_| {
        "host=postgres user=postgres password=postgres dbname=grust".to_string()
    })
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
    let build_started = Instant::now();
    let resident = store
        .indexed_snapshot()
        .await
        .map_err(|err| format!("dataset.load: resident index: {err}"))?;
    crate::load_progress::resident_index_built(&resident, build_started);
    Ok(PreparedBackend {
        inner: Backend::Postgres { store, resident },
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
    let build_started = Instant::now();
    let resident = store
        .indexed_snapshot()
        .await
        .map_err(|err| format!("dataset.load: resident index: {err}"))?;
    crate::load_progress::resident_index_built(&resident, build_started);
    Ok(PreparedBackend {
        inner: Backend::Postgres { store, resident },
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
    let store =
        SurrealHttpGraphStore::connect(surreal_config(graph)).map_err(|err| err.to_string())?;
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

#[cfg(feature = "surreal")]
fn surreal_config(graph: &Graph) -> SurrealConfig {
    SurrealConfig {
        url: env::var("SURREAL_URL").unwrap_or_else(|_| "http://surreal:8000/sql".to_string()),
        namespace: "lsqb".to_string(),
        database: "matrix".to_string(),
        labels: node_labels(graph),
        relationships: edge_labels(graph),
        ..SurrealConfig::default()
    }
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
    let config = SailConfig {
        endpoint: env::var("SAIL_ENDPOINT").unwrap_or_else(|_| "http://sail:50051".to_string()),
        // Match the projected loader chunk size; avoid ten Delta commits per chunk.
        batch_size: 10_000,
        ..SailConfig::default()
    };
    let store = SailGraphStore::connect(config.clone())
        .await
        // External endpoints may contain credentials. The adapter itself redacts
        // transport failures, and the benchmark boundary deliberately provides a
        // second defense so report details can never serialize the endpoint.
        .map_err(|_| "connect to Sail failed".to_string())?;
    let started = Instant::now();
    let session = sail_session::Session::owned(store, config);
    let result = async {
        session.bootstrap().await.map_err(|err| err.to_string())?;
        session.clear().await.map_err(|err| err.to_string())?;
        session
            .put_graph(graph)
            .await
            .map_err(|err| err.to_string())?;
        elapsed_ns(started)
    }
    .await;
    finish_sail_preparation(session, result).await
}

#[cfg(feature = "sail")]
async fn prepare_sail_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let config = SailConfig {
        endpoint: env::var("SAIL_ENDPOINT").unwrap_or_else(|_| "http://sail:50051".to_string()),
        batch_size: 10_000,
        ..SailConfig::default()
    };
    let store = SailGraphStore::connect(config.clone())
        .await
        .map_err(|_| "connect to Sail failed".to_string())?;
    let started = Instant::now();
    let session = sail_session::Session::owned(store, config);
    let result = async {
        session.bootstrap().await.map_err(|err| err.to_string())?;
        session.clear().await.map_err(|err| err.to_string())?;
        put_projected_chunks(&*session, chunks).await?;
        elapsed_ns(started)
    }
    .await;
    finish_sail_preparation(session, result).await
}

#[cfg(feature = "sail")]
async fn finish_sail_preparation(
    session: sail_session::Session,
    result: Result<u64, String>,
) -> Result<PreparedBackend, String> {
    match result {
        Ok(load_ns) => Ok(PreparedBackend {
            inner: Backend::Sail(session),
            load_ns,
        }),
        Err(error) => match session.close().await {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; {cleanup}")),
        },
    }
}

async fn put_projected_chunks<S, I>(store: &S, chunks: I) -> Result<(), String>
where
    S: GraphStore,
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let mut loading_edges = false;
    let mut progress = super::load_progress::LoadProgress::new();
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
        progress.completed(chunk.nodes.len(), chunk.edges.len());
    }
    Ok(())
}

#[cfg(feature = "pggraph")]
async fn prepare_pggraph(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = PgGraphStore::connect(pggraph_config())
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

#[cfg(feature = "pggraph")]
fn pggraph_config() -> PgGraphConfig {
    PgGraphConfig {
        connection_string: worker_postgres_connection(pggraph_base_connection()),
        table_prefix: "lsqb_matrix".to_string(),
        auto_build: true,
        ..PgGraphConfig::default()
    }
}

fn pggraph_base_connection() -> String {
    env::var("PGGRAPH_URL")
        .unwrap_or_else(|_| "host=pggraph user=postgres password=postgres dbname=graph".to_string())
}

#[cfg(feature = "postgres-pgq")]
async fn prepare_postgres_pgq(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = PostgresPgqStore::connect(postgres_pgq_config())
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

#[cfg(feature = "postgres-pgq")]
fn postgres_pgq_config() -> PostgresPgqConfig {
    PostgresPgqConfig {
        connection_string: worker_postgres_connection(postgres_pgq_base_connection()),
        table_prefix: "lsqb_matrix".to_string(),
        graph_name: "lsqb_matrix_graph".to_string(),
        ..PostgresPgqConfig::default()
    }
}

fn postgres_pgq_base_connection() -> String {
    env::var("POSTGRES_PGQ_URL").unwrap_or_else(|_| {
        "host=postgres-pgq user=postgres password=postgres dbname=graph".to_string()
    })
}

fn worker_postgres_connection(base: String) -> String {
    let Some(token) = env::var("GRUST_LSQB_WORKER_TOKEN")
        .ok()
        .filter(|value| worker_identifier(value))
    else {
        return base;
    };
    let Some(timeout_ms) = env::var("GRUST_LSQB_WORKER_QUERY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
    else {
        return base;
    };
    tag_postgres_connection(base, &token, timeout_ms)
}

fn tag_postgres_connection(base: String, token: &str, timeout_ms: u64) -> String {
    if base.starts_with("postgres://") || base.starts_with("postgresql://") {
        let separator = if base.contains('?') { '&' } else { '?' };
        format!(
            "{base}{separator}application_name={token}&options=-c%20statement_timeout%3D{timeout_ms}"
        )
    } else {
        format!("{base} application_name='{token}' options='-c statement_timeout={timeout_ms}'")
    }
}

fn worker_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

async fn wait_for_postgres_session_absence(
    connection_string: &str,
    worker_token: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    if !worker_identifier(worker_token) {
        return Err("invalid PostgreSQL worker identity".to_string());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let (client, connection) = tokio::time::timeout_at(
        deadline,
        tokio_postgres::connect(connection_string, tokio_postgres::NoTls),
    )
    .await
    .map_err(|_| "PostgreSQL recovery connection exceeded its timeout".to_string())?
    .map_err(|_| "PostgreSQL recovery connection failed".to_string())?;
    let mut connection_task = tokio::spawn(connection);
    let probe = async {
        loop {
            let row = client
                .query_one(
                    "SELECT NOT EXISTS (SELECT 1 FROM pg_stat_activity WHERE application_name = $1 AND pid <> pg_backend_pid())",
                    &[&worker_token],
                )
                .await
                .map_err(|_| "PostgreSQL recovery probe failed".to_string())?;
            if row.get::<_, bool>(0) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    let result = tokio::time::timeout_at(deadline, probe).await.map_err(|_| {
        "PostgreSQL worker session did not quiesce within the recovery timeout".to_string()
    });
    drop(client);
    connection_task.abort();
    let _ = (&mut connection_task).await;
    result?
}

#[cfg(feature = "helix")]
async fn prepare_helix(graph: &Graph) -> Result<PreparedBackend, String> {
    let store = HelixHttpGraphStore::connect(helix_config(graph)).map_err(|err| err.to_string())?;
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

#[cfg(feature = "helix")]
fn helix_config(graph: &Graph) -> HelixHttpConfig {
    HelixHttpConfig {
        query_url: env::var("HELIX_QUERY_URL")
            .unwrap_or_else(|_| "http://helix:8080/v1/query".to_string()),
        labels: node_labels(graph),
        ..HelixHttpConfig::default()
    }
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
    // Scoped registry patches move only affected adapters. The internal benchmark
    // runner stays at 0.13.0 with every unchanged adapter, so its own package
    // version is the correct default for the remaining adapter identities.
    match id {
        "sail" => "0.13.2",
        "surreal" | "surreal-sdk" => "0.13.1",
        _ => env!("CARGO_PKG_VERSION"),
    }
}

fn nonempty_env(name: String) -> Option<String> {
    env::var(name).ok().filter(|value| !value.is_empty())
}

fn resource_components(id: &str) -> u32 {
    if matches!(
        id,
        "postgres"
            | "falkor"
            | "surreal"
            | "surreal-sdk"
            | "sail"
            | "pggraph"
            | "postgres-pgq"
            | "helix"
            | "helix-sdk"
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
        "turso" => resident_store_execution_class(case, &TursoReadDialect::new("grust")),
        "postgres" => {
            resident_store_execution_class(case, &PostgresReadDialect::new(&postgres_config()))
        }
        #[cfg(feature = "sail")]
        "sail" => sql_execution_class(case, &grust_cypher::pushdown::SparkDialect),
        _ => Err(format!(
            "backend {id:?} has no compiled portable-query classifier"
        )),
    }
}

fn indexed_count(index: &TypedGraphIndex, case: &QueryCase) -> Result<i64, String> {
    count_from(
        grust_cypher::read::run_read_query_indexed(
            index,
            &case.executable,
            &CypherParameters::new(),
        )
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
        Backend, QueryExecutionError, adapter_version, blocking_count_with_timeout, load_strategy,
        prepare_memory_chunks, put_projected_chunks, recover_after_unacknowledged_exit,
        recovery_contract, resource_components, tag_postgres_connection, worker_query_timeout_ms,
    };
    use crate::report::{LoadStrategyV3, RecoveryContractV3};

    #[tokio::test]
    async fn projected_chunks_build_one_owned_memory_graph_inside_the_load_interval() {
        let delay = Duration::from_millis(5);
        let nodes = Graph::new(
            vec![
                Node::new("Person", "a", Props::new()),
                Node::new("Person", "b", Props::new()),
            ],
            Vec::new(),
        );
        let edges = Graph::new(Vec::new(), vec![Edge::new("KNOWS", "a", "b", Props::new())]);
        let chunks = std::iter::once_with(move || {
            std::thread::sleep(delay);
            Ok(nodes)
        })
        .chain(std::iter::once(Ok(edges)));

        let prepared = prepare_memory_chunks(chunks)
            .await
            .expect("memory chunks load");
        assert!(prepared.load_ns >= u64::try_from(delay.as_nanos()).unwrap());
        let Backend::Memory(graph) = &prepared.inner else {
            panic!("memory preparation must retain the reference graph");
        };
        assert_eq!(Arc::strong_count(graph), 1);
        assert_eq!(graph.graph().nodes.len(), 2);
        assert_eq!(graph.graph().edges.len(), 1);
    }

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
        assert_eq!(adapter_version("sail"), "0.13.2");
        assert_eq!(adapter_version("surreal"), "0.13.1");
        assert_eq!(adapter_version("surreal-sdk"), "0.13.1");
        assert_eq!(adapter_version("memory"), "0.13.0");
    }

    #[test]
    fn sdk_lanes_attach_and_fail_closed_without_remote_recovery_proof() {
        for id in ["helix-sdk", "surreal-sdk"] {
            assert_eq!(load_strategy(id, true), LoadStrategyV3::OnceWorkerAttach);
            assert_eq!(resource_components(id), 2);
            assert_eq!(recovery_contract(id, true), RecoveryContractV3::FailClosed);
            assert_eq!(load_strategy(id, false), LoadStrategyV3::NotExecuted);
        }
    }

    #[test]
    fn backend_lifecycle_contracts_are_explicit() {
        assert_eq!(
            load_strategy("memory", true),
            LoadStrategyV3::PerObservationWorkerReload
        );
        assert_eq!(
            recovery_contract("memory", true),
            RecoveryContractV3::ProcessGroupAbsent
        );
        assert_eq!(
            load_strategy("postgres", true),
            LoadStrategyV3::OnceWorkerAttach
        );
        assert_eq!(
            recovery_contract("postgres", true),
            RecoveryContractV3::PostgresSessionAbsent
        );
        assert_eq!(
            recovery_contract("helix", true),
            RecoveryContractV3::FailClosed
        );
        assert_eq!(
            recovery_contract("memory", false),
            RecoveryContractV3::NotApplicable
        );
    }

    #[test]
    fn postgres_workers_receive_a_unique_deadline_and_session_identity() {
        let key_value = tag_postgres_connection(
            "host=db password=sentinel-secret".to_string(),
            "g123-m-1-2",
            42,
        );
        assert!(key_value.contains("application_name='g123-m-1-2'"));
        assert!(key_value.contains("statement_timeout=42"));
        let url = tag_postgres_connection(
            "postgres://user:sentinel-secret@db/graph?sslmode=disable".to_string(),
            "g123-m-1-2",
            42,
        );
        assert!(url.contains("&application_name=g123-m-1-2"));
        assert!(url.contains("statement_timeout%3D42"));
    }

    #[test]
    fn falkor_native_timeout_reserves_a_bounded_acknowledgement_window() {
        assert_eq!(worker_query_timeout_ms("memory", 30_000), 30_000);
        assert_eq!(worker_query_timeout_ms("falkor", 30_000), 27_000);
        assert_eq!(worker_query_timeout_ms("falkor", 60_000), 55_000);
        assert_eq!(worker_query_timeout_ms("falkor", 600_000), 595_000);
        assert_eq!(worker_query_timeout_ms("falkor", 10), 9);
        assert_eq!(worker_query_timeout_ms("falkor", 1), 1);
    }

    #[tokio::test]
    async fn remote_backends_without_recovery_proof_fail_closed() {
        for backend in ["falkor", "sail", "surreal", "helix"] {
            let error = recover_after_unacknowledged_exit(backend, "g123-m-1-2", 1)
                .await
                .unwrap_err();
            assert!(
                error.contains("quiescence proof")
                    || error.contains("cannot prove server-side quiescence"),
                "{backend}: {error}"
            );
            assert!(!error.contains("sentinel-secret"));
        }
    }
}
