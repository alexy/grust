use super::*;

#[derive(Clone, Copy)]
pub(super) struct Filters {
    pub nodes: [fn(&Node) -> bool; 4],
    pub edges: [fn(&Edge) -> bool; 4], // R, H(c,u), H(p,v), K
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            nodes: [
                |n| n.props.get("kind") == Some(&Value::String("C".into())),
                |n| n.props.get("kind") == Some(&Value::String("P".into())),
                |_| true,
                |_| true,
            ],
            edges: [|_| true; 4],
        }
    }
}

/// Independent physical four-edge enumeration. The directed edges determine
/// all node bindings; the undirected final edge is checked once, including
/// self-loops. No index, grouped multiplicity, or planner predicates are reused.
pub(super) fn literal_count(graph: &Graph, kinds: [&str; 3], filters: Filters) -> i64 {
    let node = |id: &grust_core::NodeId| graph.nodes.iter().find(|n| &n.id == id).unwrap();
    let mut count = 0;
    for reply in &graph.edges {
        if reply.label.as_str() != kinds[0] || !(filters.edges[0])(reply) {
            continue;
        }
        let (c, p) = (node(&reply.from), node(&reply.to));
        if !(filters.nodes[0])(c) || !(filters.nodes[1])(p) {
            continue;
        }
        for (ci, ch) in graph.edges.iter().enumerate() {
            if ch.label.as_str() != kinds[1] || ch.from != c.id || !(filters.edges[1])(ch) {
                continue;
            }
            let u = node(&ch.to);
            if !(filters.nodes[2])(u) {
                continue;
            }
            for (pi, ph) in graph.edges.iter().enumerate() {
                if pi == ci
                    || ph.label.as_str() != kinds[1]
                    || ph.from != p.id
                    || !(filters.edges[2])(ph)
                {
                    continue;
                }
                let v = node(&ph.to);
                if !(filters.nodes[3])(v) {
                    continue;
                }
                for knows in &graph.edges {
                    if knows.label.as_str() == kinds[2]
                        && (filters.edges[3])(knows)
                        && ((knows.from == u.id && knows.to == v.id)
                            || (knows.to == u.id && knows.from == v.id))
                    {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}
