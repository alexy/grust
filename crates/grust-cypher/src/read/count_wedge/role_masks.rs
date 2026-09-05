//! Prepare role bits without inspecting vertices excluded by a required label.

use super::*;
use crate::read::count_predicate;
use std::{ops::Range, slice};

#[derive(Debug)]
pub(super) struct Prepared<'index> {
    masks: Vec<u8>,
    b_candidates: Option<&'index [u32]>,
    c_candidates: Option<&'index [u32]>,
}

enum CandidateVertices<'a> {
    All(Range<usize>),
    Labelled(slice::Iter<'a, u32>),
}

impl Iterator for CandidateVertices<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::All(vertices) => vertices.next(),
            Self::Labelled(vertices) => vertices.next().map(|&vertex| vertex as usize),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::All(vertices) => vertices.size_hint(),
            Self::Labelled(vertices) => vertices.size_hint(),
        }
    }
}

impl ExactSizeIterator for CandidateVertices<'_> {}

impl<'index> Prepared<'index> {
    pub(super) fn masks(&self) -> &[u8] {
        &self.masks
    }

    pub(super) fn b_candidates(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.candidates(self.b_candidates)
    }

    pub(super) fn c_candidates(&self) -> impl ExactSizeIterator<Item = usize> + '_ {
        self.candidates(self.c_candidates)
    }

    fn candidates<'a>(&'a self, labelled: Option<&'a [u32]>) -> CandidateVertices<'a> {
        match labelled {
            Some(vertices) => CandidateVertices::Labelled(vertices.iter()),
            None => CandidateVertices::All(0..self.masks.len()),
        }
    }
}

pub(super) fn prepare<'index>(
    index: &'index TypedGraphIndex,
    wedge: &Wedge<'_>,
) -> Result<Prepared<'index>> {
    let graph = index.graph();
    read_budget::charge_candidate_work(wedge.nodes.len(), "preparing count wedge roles")?;
    // The wedge proof rejects all property maps. An unlabeled role therefore
    // has no vertex predicate, and its bit belongs in every initialized mask.
    let unlabeled_bits = wedge
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, pattern)| pattern.labels.is_empty())
        .fold(0u8, |bits, (role, _)| bits | (1 << role));
    read_budget::charge_candidate_work(graph.nodes.len(), "initializing count wedge node masks")?;
    let mut masks = reserved_vec(graph.nodes.len(), "allocating count wedge node masks")?;
    masks.resize(graph.nodes.len(), unlabeled_bits);
    let mut b_candidates = None;
    let mut c_candidates = None;
    let params = CypherParameters::new();
    for (role, pattern) in wedge.nodes.iter().enumerate() {
        let Some(label) = pattern.labels.first() else {
            continue;
        };
        // Borrowed hash lookup still examines label bytes, including on misses.
        read_budget::charge_candidate_work(
            label.len().saturating_mul(2).saturating_add(1),
            "looking up count wedge candidate labels",
        )?;
        let candidates = index.vertices_with_label(label);
        match role {
            1 => b_candidates = Some(candidates),
            2 => c_candidates = Some(candidates),
            _ => {}
        }
        for &vertex in candidates {
            read_budget::charge_candidate_work(1, "filtering count wedge vertices")?;
            // A seed label narrows candidates; it never replaces the remaining
            // conjuncts or their per-label and string-comparison accounting.
            if count_predicate::node_matches(&graph.nodes[vertex as usize], pattern, &params)? {
                masks[vertex as usize] |= 1 << role;
            }
        }
    }
    read_budget::checkpoint()?;
    Ok(Prepared {
        masks,
        b_candidates,
        c_candidates,
    })
}

#[cfg(test)]
mod tests;
