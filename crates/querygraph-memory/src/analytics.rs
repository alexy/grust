//! Batch cognition: analytics that propose [`ConsolidationPlan`]s.
//!
//! Entity resolution, contradiction detection, and decay/importance scoring
//! are batch jobs (in production, over grust-sail). Their **only output is a
//! `ConsolidationPlan`**, applied through the capability-gated vault — never a
//! direct store write. That single rule is what keeps labels, quarantine, and
//! audit intact at scale: analytics can *propose* a merge or a retraction, but
//! the vault decides, mints nothing it shouldn't, and records the SecLib join.
//!
//! These reference analyzers work on the vault-facing views a caller already
//! holds ([`RecalledMemory`]) — they never touch content the caller could not
//! recall. A QueryGraph deployment swaps the bodies for sail jobs; the
//! contract (recalled views in, a plan out, applied through the vault) is the
//! durable part.

use std::collections::HashMap;

use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, MemoryContent, MemoryDraft, MemoryId, MemoryKind,
    Provenance, RecalledMemory,
};

/// Propose a plan that supersedes exact-duplicate memories (same text, case-
/// and whitespace-insensitive) with a single canonical record, keeping the
/// oldest as the survivor's `valid_from` anchor.
///
/// Duplicates are the cheapest, safest consolidation: no information is lost,
/// and the vault's join keeps the survivor at the max label of the group.
pub fn dedup_plan(memories: &[RecalledMemory]) -> ConsolidationPlan {
    let mut groups: HashMap<String, Vec<&RecalledMemory>> = HashMap::new();
    for m in memories {
        let key = normalize(&m.content.text);
        groups.entry(key).or_default().push(m);
    }
    let mut plan = ConsolidationPlan::new();
    for group in groups.values() {
        if group.len() < 2 {
            continue;
        }
        let superseded: Vec<MemoryId> = group.iter().map(|m| m.id.clone()).collect();
        // The canonical text is the first record's; valid_from is the oldest.
        let oldest = group.iter().map(|m| m.valid_from).min().expect("non-empty");
        let replacement = MemoryDraft::new(
            group[0].kind,
            MemoryContent::text(group[0].content.text.clone()),
            Provenance::Operator,
        )
        .valid_from(oldest);
        plan = plan.then(ConsolidationStep::Supersede {
            superseded,
            replacement,
        });
    }
    plan
}

/// A detected contradiction: two memories asserting different values for the
/// same subject+predicate (heuristic: same leading words, different final
/// word), most-recent first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contradiction {
    /// The superseding (newer) memory.
    pub newer: MemoryId,
    /// The superseded (older) memory.
    pub older: MemoryId,
}

/// Detect attribute contradictions and propose invalidating the *older*
/// assertion — the newer belief wins, the older survives as bi-temporal
/// history (the vault invalidates, it does not destroy).
pub fn contradiction_plan(memories: &[RecalledMemory]) -> (Vec<Contradiction>, ConsolidationPlan) {
    let mut found = Vec::new();
    let mut plan = ConsolidationPlan::new();
    for (i, a) in memories.iter().enumerate() {
        for b in &memories[i + 1..] {
            if contradicts(&a.content.text, &b.content.text) {
                // Keep the more recently-valid assertion.
                let (newer, older) = if a.valid_from >= b.valid_from {
                    (a, b)
                } else {
                    (b, a)
                };
                found.push(Contradiction {
                    newer: newer.id.clone(),
                    older: older.id.clone(),
                });
                plan = plan.then(ConsolidationStep::Invalidate {
                    ids: vec![older.id.clone()],
                });
            }
        }
    }
    (found, plan)
}

/// Importance score for decay-based retention: recency-weighted, with a floor
/// for `Profile` memories (durable facts decay slower than episodes). Purely
/// advisory — a caller ranks by this and forgets the tail, always through the
/// vault's `forget`/`reap`.
pub fn importance(memory: &RecalledMemory, now: chrono::DateTime<chrono::Utc>) -> f64 {
    let age_days = (now - memory.valid_from).num_days().max(0) as f64;
    let recency = 1.0 / (1.0 + age_days / 30.0);
    let kind_weight = match memory.kind {
        MemoryKind::Profile => 1.0,
        MemoryKind::Semantic => 0.8,
        MemoryKind::Procedural => 0.7,
        MemoryKind::Episodic => 0.4,
    };
    recency * kind_weight
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Same leading words, different final word — "Alice lives in Rome" vs
/// "Alice lives in Venice".
fn contradicts(a: &str, b: &str) -> bool {
    let aw: Vec<&str> = a.split_whitespace().collect();
    let bw: Vec<&str> = b.split_whitespace().collect();
    if aw.len() < 2 || aw.len() != bw.len() {
        return false;
    }
    let prefix_eq = aw[..aw.len() - 1]
        .iter()
        .zip(&bw[..bw.len() - 1])
        .all(|(x, y)| x.eq_ignore_ascii_case(y));
    prefix_eq && !aw.last().unwrap().eq_ignore_ascii_case(bw.last().unwrap())
}

#[cfg(test)]
mod tests;
