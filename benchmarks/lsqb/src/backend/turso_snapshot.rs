//! Turso's per-observation state as a prebuilt database file.
//!
//! Turso is process-owned, so every observation worker must hold its own
//! store: process exit is the recovery proof. Reloading the CSVs in each
//! worker cost 67 s per observation at SF0.1 (2,080,404 edges) against a
//! 5 s read-back and index build. The coordinator now loads the dataset once
//! into a file-backed store, and each worker copies that file (0.24 s for
//! 553 MB at SF0.1) into a private path, opens the copy, and builds its
//! resident index before READY. The copy, like the load, stays outside the
//! query boundary; the cell's lifecycle names the strategy.

use std::path::{Path, PathBuf};
use std::time::Instant;

use grust_core::{Graph, GraphAdminStore};
use grust_turso::{TursoConfig, TursoGraphStore};

use super::{Backend, PreparedBackend, elapsed_ns, put_projected_chunks};

/// Environment variable through which the coordinator hands each worker the
/// path of the prebuilt store to copy.
pub const ENV_SNAPSHOT: &str = "GRUST_LSQB_WORKER_TURSO_SNAPSHOT";

/// A database file this process created and owns; the file and its WAL and
/// shared-memory siblings are removed when the owner is dropped.
#[derive(Debug)]
pub struct OwnedDatabaseFile {
    path: PathBuf,
}

impl OwnedDatabaseFile {
    /// A fresh, private path under the process temp directory. The name
    /// carries the process id and a nanosecond stamp, and creation refuses an
    /// existing file rather than reuse one.
    fn create(label: &str) -> Result<Self, String> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "grust-lsqb-turso-{label}-{}-{stamp}.db",
            std::process::id()
        ));
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!("cannot create Turso store file {}: {error}", path.display())
            })?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn config(&self) -> TursoConfig {
        TursoConfig {
            path: self.path.to_string_lossy().into_owned(),
            ..TursoConfig::default()
        }
    }
}

impl Drop for OwnedDatabaseFile {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut sibling = self.path.clone().into_os_string();
            sibling.push(suffix);
            let _ = std::fs::remove_file(sibling);
        }
    }
}

/// The coordinator's load: the dataset into a fresh file-backed store, then
/// the resident index. The file outlives the load for the workers to copy
/// and is removed with the returned backend.
pub async fn prepare_from_chunks<I>(chunks: I) -> Result<PreparedBackend, String>
where
    I: IntoIterator<Item = Result<Graph, String>>,
{
    let file = OwnedDatabaseFile::create("coordinator")?;
    let store = TursoGraphStore::connect(file.config())
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
        inner: Backend::Turso {
            store,
            resident,
            file: Some(file),
        },
        load_ns: elapsed_ns(started)?,
    })
}

/// An observation worker's state: a private copy of the coordinator's file,
/// opened and indexed. `load_ns` covers the copy, the open and the build.
pub async fn prepare_from_copy(source: &Path) -> Result<PreparedBackend, String> {
    let started = Instant::now();
    let file = OwnedDatabaseFile::create("worker")?;
    std::fs::copy(source, file.path()).map_err(|error| {
        format!(
            "cannot copy the coordinator's Turso store {}: {error}",
            source.display()
        )
    })?;
    let store = TursoGraphStore::connect(file.config())
        .await
        .map_err(|err| err.to_string())?;
    let resident = store
        .indexed_snapshot()
        .await
        .map_err(|err| format!("resident index: {err}"))?;
    Ok(PreparedBackend {
        inner: Backend::Turso {
            store,
            resident,
            file: Some(file),
        },
        load_ns: elapsed_ns(started)?,
    })
}

/// The path the coordinator hands its workers, when this backend owns one.
pub(super) fn snapshot_path(backend: &Backend) -> Option<&Path> {
    match backend {
        Backend::Turso {
            file: Some(file), ..
        } => Some(file.path()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_file_is_private_and_removed_with_its_siblings() {
        let file = OwnedDatabaseFile::create("test").unwrap();
        let path = file.path().to_path_buf();
        assert!(path.is_file());
        let mut wal = path.clone().into_os_string();
        wal.push("-wal");
        std::fs::write(&wal, b"").unwrap();
        drop(file);
        assert!(!path.exists());
        assert!(!Path::new(&wal).exists());
    }

    #[tokio::test]
    async fn a_worker_copy_serves_the_coordinator_graph_and_leaves_no_file_behind() {
        use grust_core::{Edge, Node};
        let graph = Graph::new(
            vec![
                Node::new("Person", "a", grust_core::Props::default()),
                Node::new("Person", "b", grust_core::Props::default()),
            ],
            vec![Edge::new("KNOWS", "a", "b", grust_core::Props::default())],
        );
        let coordinator = prepare_from_chunks(std::iter::once(Ok(graph)))
            .await
            .unwrap();
        let source = snapshot_path(&coordinator.inner).unwrap().to_path_buf();
        assert!(source.is_file());
        let worker = prepare_from_copy(&source).await.unwrap();
        let copy = snapshot_path(&worker.inner).unwrap().to_path_buf();
        assert_ne!(copy, source);
        let Backend::Turso { resident, .. } = &worker.inner else {
            panic!("not a Turso backend");
        };
        assert_eq!(resident.graph().nodes.len(), 2);
        assert_eq!(resident.graph().edges.len(), 1);
        drop(worker);
        assert!(!copy.exists(), "the worker's copy is removed with it");
        assert!(
            source.is_file(),
            "the coordinator's file outlives the worker"
        );
        drop(coordinator);
        assert!(!source.exists());
    }
}
