//! Necessary typed-adjacency checks before property-bearing branch predicates.
//!
//! Every mandatory incident atom must have an edge in its slot-relative
//! direction. Nonempty adjacency does not prove neighbor labels or any
//! property predicate: the original predicates and forest DP remain required.
//! Optional leaves never participate, because their absence pads a row.

use super::*;
use grust_core::TypedAdjacencyView;

struct Required<'index> {
    adjacency: TypedAdjacencyView<'index>,
    direction: ast::Direction,
}

pub(super) struct Prepared<'index> {
    atoms: Vec<Required<'index>>,
}

pub(super) fn enabled(forest: &Forest<'_>, slot: usize) -> bool {
    forest.adjacency[slot].len() >= 2
        && forest.nodes[slot].iter().any(|pattern| {
            pattern
                .properties
                .as_ref()
                .is_some_and(|properties| !properties.entries.is_empty())
        })
}

/// Resolve each required type once for this role, not once per candidate.
/// Disabled roles retain their allocation-free path. The views borrow the
/// immutable index; neither relationship strings nor adjacency slots are copied.
pub(super) fn prepare<'index>(
    index: &'index TypedGraphIndex,
    forest: &Forest<'_>,
    slot: usize,
) -> Result<Option<Prepared<'index>>> {
    if !enabled(forest, slot) {
        return Ok(None);
    }
    let count = forest.adjacency[slot].len();
    read_budget::charge_intermediate_bytes(
        count.saturating_mul(std::mem::size_of::<Required<'_>>()),
        "preparing mandatory count adjacency",
    )?;
    let mut atoms = Vec::with_capacity(count);
    for &(_, edge) in &forest.adjacency[slot] {
        let atom = &forest.atoms[edge];
        let relationship = atom.relationship;
        let kind = &relationship.types[0];
        read_budget::charge_candidate_work(
            kind.len().saturating_mul(2).saturating_add(1),
            "resolving mandatory count adjacency",
        )?;
        let direction = if slot == atom.from {
            relationship.direction
        } else {
            match relationship.direction {
                ast::Direction::Outgoing => ast::Direction::Incoming,
                ast::Direction::Incoming => ast::Direction::Outgoing,
                ast::Direction::Undirected => ast::Direction::Undirected,
            }
        };
        atoms.push(Required {
            adjacency: index.adjacency(kind),
            direction,
        });
    }
    Ok(Some(Prepared { atoms }))
}

impl Required<'_> {
    fn has_adjacency(&self, vertex: u32, incoming: bool) -> Result<bool> {
        // Type hashing was paid during preparation. Each actual row probe
        // still checkpoints the budget, including absent types and empty rows.
        read_budget::charge_candidate_work(1, "probing mandatory count adjacency")?;
        let neighbors = if incoming {
            self.adjacency.incoming(vertex)
        } else {
            self.adjacency.outgoing(vertex)
        };
        Ok(!neighbors.is_empty())
    }
}

impl<'index> Prepared<'index> {
    /// Every valid binding must occur in each mandatory directed row set.
    /// Borrow a smaller sparse set when available; dense and undirected rows
    /// do not offer a cheap source slice. Labels and predicates still apply.
    pub(super) fn narrow_candidates(
        &self,
        mut candidates: Option<&'index [u32]>,
        vertex_count: usize,
    ) -> Result<Option<&'index [u32]>> {
        for atom in &self.atoms {
            read_budget::charge_candidate_work(1, "selecting mandatory count candidates")?;
            let sources = match atom.direction {
                ast::Direction::Outgoing => atom.adjacency.sparse_outgoing_sources(),
                ast::Direction::Incoming => atom.adjacency.sparse_incoming_sources(),
                ast::Direction::Undirected => None,
            };
            if let Some(sources) = sources
                && sources.len() < candidates.map_or(vertex_count, |vertices| vertices.len())
            {
                candidates = Some(sources);
            }
        }
        Ok(candidates)
    }

    pub(super) fn accepts(&self, vertex: u32) -> Result<bool> {
        for atom in &self.atoms {
            let present = match atom.direction {
                ast::Direction::Outgoing => atom.has_adjacency(vertex, false)?,
                ast::Direction::Incoming => atom.has_adjacency(vertex, true)?,
                ast::Direction::Undirected => {
                    atom.has_adjacency(vertex, false)? || atom.has_adjacency(vertex, true)?
                }
            };
            if !present {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
#[path = "mandatory_adjacency_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mandatory_adjacency_budget_tests.rs"]
mod budget_tests;

#[cfg(test)]
#[path = "mandatory_candidate_tests.rs"]
mod candidate_tests;
