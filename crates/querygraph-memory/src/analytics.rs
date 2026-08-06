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
//! recall. The live Sail path computes candidate rows remotely, then requires
//! exact agreement with these same canonical planning functions before it
//! returns a plan. The durable contract remains recalled views in, an inert
//! plan out, and application only through the vault.

use typesec_memory::{ConsolidationPlan, MemoryId, MemoryKind, RecalledMemory};

pub(crate) mod planning;

/// Propose a plan that supersedes exact-duplicate memories (same text, case-
/// and whitespace-insensitive) with a single canonical record. Groups and
/// members have a stable order; validity is compared at Sail's staged
/// microsecond precision and then by memory id.
///
/// Duplicates are the cheapest, safest consolidation: no information is lost,
/// and the vault's join keeps the survivor at the max label of the group.
pub fn dedup_plan(memories: &[RecalledMemory]) -> ConsolidationPlan {
    planning::deduplicate(memories).plan
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
/// history (the vault invalidates, it does not destroy). Equal staged validity
/// times are ordered by memory id, matching the distributed Sail plan.
pub fn contradiction_plan(memories: &[RecalledMemory]) -> (Vec<Contradiction>, ConsolidationPlan) {
    let planning = planning::reconcile(memories);
    let found = planning
        .pairs
        .iter()
        .map(|(newer, older)| Contradiction {
            newer: MemoryId::from_string(newer.clone()),
            older: MemoryId::from_string(older.clone()),
        })
        .collect();
    (found, planning.plan)
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

#[cfg(test)]
mod tests;
