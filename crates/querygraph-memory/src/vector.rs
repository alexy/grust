//! `VectorIndex` — an embedding-backed [`SemanticIndex`] with the
//! embedding-privacy rule **enforced by construction**.
//!
//! typesec-memory's `SemanticIndex` documents a contract: content labeled
//! above `Internal` must never be sent to a *remote* embedder (vectors leak
//! content). Here that contract is not merely documented — it is a type-level
//! obligation on the [`Embedder`] and checked at index time:
//!
//! - An [`Embedder`] declares [`Embedder::is_local`]. Only a local embedder
//!   may receive `Sensitive`/`Secret` text.
//! - [`VectorIndex::index`] routes above-`Internal` records to the embedder
//!   only when it is local; otherwise it **declines to index** (returns `Ok`
//!   without storing a vector). The record stays fully recallable through
//!   ordinary `StoreQuery`s — it just does not participate in vector ranking
//!   through a remote embedder. Fail closed: the content never egresses.
//!
//! Search returns the cosine-nearest record ids; the vault still applies the
//! label gate, so ranking is never an authorization path. A hybrid graph
//! re-rank ([`VectorIndex::with_neighbors`]) can boost ids that co-mention an
//! entity, Zep-style — but that is a *reordering* of already-authorized
//! candidates, never a widening.

use std::collections::HashMap;
use std::sync::RwLock;

use typesec_memory::{IndexError, Label, MemoryId, SemanticIndex};

/// Turns text into a dense vector. Implementations declare whether they run
/// locally (on-box, no egress) so the index can honor the embedding-privacy
/// rule structurally.
pub trait Embedder: Send + Sync {
    /// Embed `text` into a fixed-width vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>, IndexError>;

    /// Whether embedding happens entirely on-box (no network egress). A
    /// remote embedder (a hosted embedding API) returns `false` and will
    /// never be handed `Sensitive`/`Secret` content by [`VectorIndex`].
    fn is_local(&self) -> bool;
}

/// A cosine-similarity vector index over an [`Embedder`].
pub struct VectorIndex<E: Embedder> {
    embedder: E,
    vectors: RwLock<HashMap<MemoryId, Vec<f32>>>,
    // id -> entity names it mentions, for the optional hybrid graph re-rank.
    entities: RwLock<HashMap<MemoryId, Vec<String>>>,
}

impl<E: Embedder> VectorIndex<E> {
    /// Build a vector index over `embedder`.
    pub fn new(embedder: E) -> Self {
        Self {
            embedder,
            vectors: RwLock::new(HashMap::new()),
            entities: RwLock::new(HashMap::new()),
        }
    }

    /// Record which entities `id` mentions, enabling the hybrid graph re-rank
    /// in [`search_hybrid`](Self::search_hybrid). Optional; a caller wiring
    /// this from the graph store keeps ranking honest without changing what
    /// is *authorized*.
    pub fn note_entities(&self, id: &MemoryId, entities: impl IntoIterator<Item = String>) {
        self.entities
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), entities.into_iter().collect());
    }

    /// May this label's content be sent to the embedder? Above `Internal`
    /// requires a local embedder.
    fn may_embed(&self, label: Label) -> bool {
        label <= Label::Internal || self.embedder.is_local()
    }

    fn ranked(&self, query_vec: &[f32], limit: usize, boost: Option<&[String]>) -> Vec<MemoryId> {
        let vectors = self
            .vectors
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entities = self
            .entities
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut scored: Vec<(f32, MemoryId)> = vectors
            .iter()
            .map(|(id, vec)| {
                let mut score = cosine(query_vec, vec);
                if let Some(boost) = boost
                    && let Some(mentions) = entities.get(id)
                    && mentions.iter().any(|e| boost.contains(e))
                {
                    // A small, bounded re-rank nudge — reordering, not gating.
                    score += 0.1;
                }
                (score, id.clone())
            })
            // Drop orthogonal (no-signal) records: a zero cosine is not a hit.
            .filter(|(score, _)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().take(limit).map(|(_, id)| id).collect()
    }

    /// Hybrid search: cosine ranking with a bounded boost for records that
    /// mention any of `co_entities` (call [`note_entities`](Self::note_entities)
    /// first). Purely a reordering of vector candidates.
    pub fn search_hybrid(
        &self,
        query: &str,
        limit: usize,
        co_entities: &[String],
    ) -> Result<Vec<MemoryId>, IndexError> {
        let query_vec = self.embedder.embed(query)?;
        Ok(self.ranked(&query_vec, limit, Some(co_entities)))
    }
}

impl<E: Embedder> SemanticIndex for VectorIndex<E> {
    fn index(&self, id: &MemoryId, label: Label, text: &str) -> Result<(), IndexError> {
        if !self.may_embed(label) {
            // Fail closed: decline to index rather than send hot content to a
            // remote embedder. The record stays recallable by ordinary query.
            tracing::debug!(
                %id, label = label.name(),
                "vector index: skipping remote-embed of above-Internal content"
            );
            return Ok(());
        }
        let vector = self.embedder.embed(text)?;
        self.vectors
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), vector);
        Ok(())
    }

    fn remove(&self, id: &MemoryId) -> Result<(), IndexError> {
        self.vectors
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        self.entities
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        Ok(())
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryId>, IndexError> {
        let query_vec = self.embedder.embed(query)?;
        Ok(self.ranked(&query_vec, limit, None))
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests;
