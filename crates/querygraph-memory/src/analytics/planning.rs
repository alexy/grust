//! Canonical, backend-neutral cognition planning.

use std::collections::{BTreeMap, BTreeSet};

use typesec_memory::{
    ConsolidationPlan, ConsolidationStep, MemoryContent, MemoryDraft, Provenance, RecalledMemory,
};

pub(crate) struct DedupPlanning {
    pub(crate) plan: ConsolidationPlan,
    #[cfg(feature = "sail")]
    pub(crate) rows: BTreeSet<(String, String)>,
    #[cfg(any(feature = "sail", test))]
    pub(crate) group_count: usize,
}

pub(crate) struct ReconcilePlanning {
    pub(crate) plan: ConsolidationPlan,
    pub(crate) pairs: BTreeSet<(String, String)>,
    #[cfg(any(feature = "sail", test))]
    pub(crate) invalidated_count: usize,
}

pub(crate) fn deduplicate(memories: &[RecalledMemory]) -> DedupPlanning {
    let mut groups: BTreeMap<String, Vec<&RecalledMemory>> = BTreeMap::new();
    for memory in memories {
        groups
            .entry(normalize_text(&memory.content.text))
            .or_default()
            .push(memory);
    }

    let mut plan = ConsolidationPlan::new();
    #[cfg(feature = "sail")]
    let mut rows = BTreeSet::new();
    #[cfg(any(feature = "sail", test))]
    let mut group_count = 0;
    for (_key, mut group) in groups {
        if group.len() < 2 {
            continue;
        }
        #[cfg(any(feature = "sail", test))]
        {
            group_count += 1;
        }
        group.sort_by(|left, right| validity_order(left).cmp(&validity_order(right)));
        #[cfg(feature = "sail")]
        for memory in &group {
            rows.insert((_key.clone(), memory.id.as_str().to_owned()));
        }

        let canonical = group[0];
        let superseded = group.iter().map(|memory| memory.id.clone()).collect();
        let replacement = MemoryDraft::new(
            canonical.kind,
            MemoryContent::text(canonical.content.text.clone()),
            Provenance::Operator,
        )
        .valid_from(canonical.valid_from);
        plan = plan.then(ConsolidationStep::Supersede {
            superseded,
            replacement,
        });
    }

    DedupPlanning {
        plan,
        #[cfg(feature = "sail")]
        rows,
        #[cfg(any(feature = "sail", test))]
        group_count,
    }
}

pub(crate) fn reconcile(memories: &[RecalledMemory]) -> ReconcilePlanning {
    let sources: Vec<_> = memories.iter().map(ReconcileSource::new).collect();
    let mut pairs = BTreeSet::new();
    let mut older_ids = BTreeSet::new();
    for (index, left) in sources.iter().enumerate() {
        for right in &sources[index + 1..] {
            if !left.conflicts_with(right) {
                continue;
            }
            let (newer, older) = if validity_order(left.memory) > validity_order(right.memory) {
                (left, right)
            } else {
                (right, left)
            };
            if newer.memory.id == older.memory.id {
                continue;
            }
            pairs.insert((
                newer.memory.id.as_str().to_owned(),
                older.memory.id.as_str().to_owned(),
            ));
            older_ids.insert(older.memory.id.clone());
        }
    }

    #[cfg(any(feature = "sail", test))]
    let invalidated_count = older_ids.len();
    let plan = older_ids
        .into_iter()
        .fold(ConsolidationPlan::new(), |plan, id| {
            plan.then(ConsolidationStep::Invalidate { ids: vec![id] })
        });
    ReconcilePlanning {
        plan,
        pairs,
        #[cfg(any(feature = "sail", test))]
        invalidated_count,
    }
}

pub(crate) fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(crate) fn assertion_parts(text: &str) -> (String, String) {
    let normalized = normalize_text(text);
    normalized_assertion_parts(&normalized)
}

pub(crate) fn normalized_assertion_parts(normalized: &str) -> (String, String) {
    let Some((prefix, tail)) = normalized.rsplit_once(' ') else {
        return (String::new(), String::new());
    };
    (prefix.to_owned(), tail.to_owned())
}

struct ReconcileSource<'a> {
    memory: &'a RecalledMemory,
    prefix: String,
    tail: String,
}

impl<'a> ReconcileSource<'a> {
    fn new(memory: &'a RecalledMemory) -> Self {
        let (prefix, tail) = assertion_parts(&memory.content.text);
        Self {
            memory,
            prefix,
            tail,
        }
    }

    fn conflicts_with(&self, other: &Self) -> bool {
        !self.prefix.is_empty() && self.prefix == other.prefix && self.tail != other.tail
    }
}

fn validity_order(memory: &RecalledMemory) -> (i64, &typesec_memory::MemoryId) {
    // Sail stages this value as an Int64 at microsecond precision. Keeping the
    // reference on the same key makes sub-microsecond values deterministic too.
    (memory.valid_from.timestamp_micros(), &memory.id)
}

#[cfg(test)]
mod tests;
