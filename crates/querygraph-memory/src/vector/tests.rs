use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A deterministic local embedder: a tiny bag-of-known-words vector. Good
/// enough to rank by term overlap without a model.
struct BagEmbedder {
    vocab: Vec<&'static str>,
    local: bool,
    calls: AtomicUsize,
}

impl BagEmbedder {
    fn new(local: bool) -> Self {
        Self {
            vocab: vec!["alice", "venice", "acme", "coffee", "tea", "secret", "code"],
            local,
            calls: AtomicUsize::new(0),
        }
    }
}

impl Embedder for BagEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, IndexError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let lower = text.to_lowercase();
        Ok(self
            .vocab
            .iter()
            .map(|w| if lower.contains(w) { 1.0 } else { 0.0 })
            .collect())
    }
    fn is_local(&self) -> bool {
        self.local
    }
}

fn id(s: &str) -> MemoryId {
    MemoryId::from_string(s)
}

#[test]
fn ranks_by_cosine_similarity() {
    let index = VectorIndex::new(BagEmbedder::new(true));
    index
        .index(&id("m1"), Label::Public, "Alice visits Venice")
        .unwrap();
    index
        .index(&id("m2"), Label::Public, "Alice drinks coffee")
        .unwrap();
    index
        .index(&id("m3"), Label::Public, "Bob drinks tea")
        .unwrap();

    let hits = index.search("where in Venice is Alice", 10).unwrap();
    assert_eq!(hits[0], id("m1"), "closest vector first");
    assert!(!hits.contains(&id("m3")), "no shared terms, no rank");
}

#[test]
fn remote_embedder_never_sees_above_internal_content() {
    let embedder = BagEmbedder::new(false); // remote
    let index = VectorIndex::new(embedder);

    // Public/Internal: embedded (calls the embedder).
    index
        .index(&id("pub"), Label::Public, "acme coffee")
        .unwrap();
    index
        .index(&id("int"), Label::Internal, "alice venice")
        .unwrap();
    // Sensitive/Secret: declined — no embed call, no vector stored.
    index
        .index(&id("sens"), Label::Sensitive, "secret code")
        .unwrap();
    index
        .index(&id("sec"), Label::Secret, "secret code")
        .unwrap();

    // Two records were embedded (pub, int); the search adds one more call.
    // The sensitive/secret records were never embedded → not in the index.
    let hits = index.search("secret code", 10).unwrap();
    assert!(
        !hits.contains(&id("sens")),
        "sensitive content not indexed remotely"
    );
    assert!(
        !hits.contains(&id("sec")),
        "secret content not indexed remotely"
    );
    assert_eq!(
        index.embedder.calls.load(Ordering::Relaxed),
        3,
        "2 indexed + 1 query, hot content skipped"
    );
}

#[test]
fn local_embedder_may_index_every_label() {
    let index = VectorIndex::new(BagEmbedder::new(true)); // local
    index
        .index(&id("sec"), Label::Secret, "secret code")
        .unwrap();
    let hits = index.search("secret code", 10).unwrap();
    assert_eq!(
        hits,
        vec![id("sec")],
        "local embedder indexes secret content"
    );
}

#[test]
fn hybrid_rerank_boosts_co_mentioned_entities() {
    let index = VectorIndex::new(BagEmbedder::new(true));
    // Two records with identical vectors (same terms) but different entities.
    index.index(&id("m1"), Label::Public, "acme note").unwrap();
    index.index(&id("m2"), Label::Public, "acme note").unwrap();
    index.note_entities(&id("m2"), ["Venice".to_string()]);

    // Plain search: tie broken by id → m1 first.
    assert_eq!(index.search("acme note", 2).unwrap()[0], id("m1"));
    // Hybrid boosting Venice-mentioning records → m2 first.
    let hybrid = index
        .search_hybrid("acme note", 2, &["Venice".to_string()])
        .unwrap();
    assert_eq!(hybrid[0], id("m2"), "the co-mentioning record is boosted");
}

#[test]
fn remove_prunes_vectors_and_entities() {
    let index = VectorIndex::new(BagEmbedder::new(true));
    index
        .index(&id("m1"), Label::Public, "alice venice")
        .unwrap();
    index.note_entities(&id("m1"), ["Venice".to_string()]);
    assert_eq!(index.search("venice", 10).unwrap().len(), 1);
    index.remove(&id("m1")).unwrap();
    assert!(index.search("venice", 10).unwrap().is_empty());
}
