use super::*;

#[derive(Clone, Copy)]
pub(super) struct Filters {
    pub nodes: [fn(&Node) -> bool; 4],
    pub relationships: [fn(&Edge) -> bool; 3],
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            nodes: [|_| true; 4],
            relationships: [|_| true; 3],
        }
    }
}

/// Independent physical-edge tuple enumeration, including relationship reuse
/// rejection and explicit optional edge bindings/null padding. It uses neither
/// AST predicates, indexed adjacency, degree products, nor subtraction.
pub(super) fn literal_count(
    graph: &Graph,
    repeated: &str,
    bridge: &str,
    filters: Filters,
    anti: bool,
) -> i64 {
    let node = |id: &grust_core::NodeId| graph.nodes.iter().find(|node| &node.id == id).unwrap();
    let mut count = 0;
    for link in &graph.edges {
        if link.label.as_str() != bridge || !(filters.relationships[1])(link) {
            continue;
        }
        let (c, m) = (node(&link.from), node(&link.to));
        if !(filters.nodes[2])(c) || !(filters.nodes[1])(m) {
            continue;
        }
        for (left_slot, left) in graph.edges.iter().enumerate() {
            if left.label.as_str() != repeated
                || left.from != m.id
                || !(filters.relationships[0])(left)
            {
                continue;
            }
            let a = node(&left.to);
            if !(filters.nodes[0])(a) {
                continue;
            }
            for (right_slot, right) in graph.edges.iter().enumerate() {
                if right.label.as_str() != repeated
                    || right.from != c.id
                    || left_slot == right_slot
                    || !(filters.relationships[2])(right)
                {
                    continue;
                }
                let b = node(&right.to);
                if !(filters.nodes[3])(b) || a.id == b.id {
                    continue;
                }
                if !anti {
                    count += 1;
                    continue;
                }
                let mut optional_rows: Vec<Option<&Edge>> = graph
                    .edges
                    .iter()
                    .filter(|edge| {
                        edge.label.as_str() == repeated && edge.from == c.id && edge.to == a.id
                    })
                    .map(Some)
                    .collect();
                if optional_rows.is_empty() {
                    optional_rows.push(None);
                }
                // WITH is not DISTINCT: every surviving physical-edge tuple
                // contributes, even when m/c are projected out of its binding.
                for binding in optional_rows {
                    if binding.is_none() {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}
