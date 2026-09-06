use std::sync::RwLockWriteGuard;

use super::*;

impl MemoryGraphStore {
    /// Returns a reusable typed index owning an immutable graph snapshot.
    ///
    /// The first call after a write clones the stored nodes and edges and builds
    /// their index. Subsequent calls, including calls through cloned stores,
    /// share that index without cloning the graph. Previously returned snapshots
    /// remain valid and unchanged after later writes.
    ///
    /// Like `TypedGraphIndex::new`, this returns an error for dangling edges or
    /// graphs that exceed the index's slot capacity. Existing store reads and
    /// writes retain their semantics. A write attempt invalidates the cache even
    /// if validation subsequently fails.
    pub fn indexed_snapshot(&self) -> Result<Arc<TypedGraphIndex>> {
        // Always acquire the graph lock before the cache lock, on both reads and
        // writes. Holding the read lock through construction makes the snapshot
        // coherent; the cache lock prevents concurrent first callers rebuilding
        // the same graph and publishing different index identities.
        let inner = self.inner.read().expect("memory graph lock poisoned");
        let mut cache = self
            .index_cache
            .lock()
            .expect("memory index cache lock poisoned");
        if let Some(index) = cache.as_ref() {
            return Ok(Arc::clone(index));
        }
        let graph = Arc::new(Self::graph_snapshot(&inner));
        let index = Arc::new(TypedGraphIndex::new(graph)?);
        *cache = Some(Arc::clone(&index));
        Ok(index)
    }

    /// The single entry point for mutable access to the graph.
    ///
    /// Invalidate before exposing mutable state so early errors and mutation
    /// plans that partially apply cannot leave a stale snapshot cached.
    pub(super) fn write_inner(&self) -> RwLockWriteGuard<'_, MemoryGraph> {
        let inner = self.inner.write().expect("memory graph lock poisoned");
        let retired = self
            .index_cache
            .lock()
            .expect("memory index cache lock poisoned")
            .take();
        if let Some(index) = retired {
            release_in_background(index);
        }
        inner
    }
}

/// Free a retired snapshot off the writer's path.
///
/// When the cache held the last reference, dropping it frees every cloned
/// node and edge of the snapshot, tens of milliseconds for a few hundred
/// thousand elements, and that would land on the latency of the one write
/// that invalidated it. A detached thread takes the drop instead; if no
/// thread can be spawned the closure is dropped here, releasing it inline.
fn release_in_background(index: Arc<TypedGraphIndex>) {
    std::thread::Builder::new()
        .name("grust-memory-snapshot-release".into())
        .spawn(move || drop(index))
        .ok();
}

#[cfg(test)]
#[path = "indexed_snapshot_tests.rs"]
mod tests;
