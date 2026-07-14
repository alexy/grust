//! Durable local storage for QueryGraph memory through Turso/libSQL.
//!
//! [`TursoMemoryStore::open`] creates the Turso connection and bootstraps the
//! Grust universal tables on the same runtime that drives subsequent
//! synchronous [`typesec_memory::MemoryStore`] calls. Callers should provide a
//! filesystem path for persistence; Turso's `:memory:` path remains useful for
//! tests through [`TursoMemoryStore::open_with_config`] but is intentionally
//! ephemeral.

use std::path::Path;

use grust_core::prelude::GraphAdminStore;
use grust_turso::TursoGraphStore;
pub use grust_turso::{TursoConfig, TursoJournalMode};
use typesec_memory::StoreError;

use crate::{Bridge, GraphStoreMemoryStore};

/// The v1 durable QueryGraph memory store.
pub type TursoMemoryStore = GraphStoreMemoryStore<TursoGraphStore>;

impl GraphStoreMemoryStore<TursoGraphStore> {
    /// Open and bootstrap a file-backed Turso memory store.
    ///
    /// The default table prefix is `querygraph_memory`. Use
    /// [`open_with_config`](Self::open_with_config) when sharing a database
    /// with another Grust graph or selecting a non-default journal mode.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                StoreError::Backend(format!(
                    "creating Turso memory database directory {}: {error}",
                    parent.display()
                ))
            })?;
        }
        let path = path.to_str().ok_or_else(|| {
            StoreError::Backend("Turso memory database path is not valid UTF-8".to_string())
        })?;
        Self::open_with_config(TursoConfig {
            path: path.to_string(),
            table_prefix: "querygraph_memory".to_string(),
            ..TursoConfig::default()
        })
    }

    /// Open and bootstrap a Turso memory store with explicit backend options.
    ///
    /// Construction is synchronous because TypeSec's `MemoryStore` contract is
    /// synchronous. The Turso async connection is created on this store's
    /// bridge runtime, which also owns the I/O and time drivers used by later
    /// operations.
    pub fn open_with_config(config: TursoConfig) -> Result<Self, StoreError> {
        let bridge = Bridge::new();
        let graph = bridge
            .run(async move {
                let graph = TursoGraphStore::connect(config).await?;
                graph.bootstrap().await?;
                Ok::<_, grust_core::prelude::GrustError>(graph)
            })
            .map_err(|err| StoreError::Backend(err.to_string()))?;
        Ok(Self { graph, bridge })
    }
}
