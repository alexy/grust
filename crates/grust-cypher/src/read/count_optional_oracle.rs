use super::*;

#[derive(Clone, Copy)]
pub(super) enum Flow {
    Out,
    In,
    Either,
}

pub(super) struct RawEdge<'a> {
    pub(super) anchor: usize,
    pub(super) leaf: usize,
    pub(super) kind: &'a str,
    pub(super) flow: Flow,
    pub(super) optional: bool,
    pub(super) anchor_label: Option<&'a str>,
    pub(super) leaf_label: Option<&'a str>,
    pub(super) leaf_kind: Option<&'a str>,
}

pub(super) fn raw(
    kind: &str,
    anchor: usize,
    leaf: usize,
    flow: Flow,
    optional: bool,
) -> RawEdge<'_> {
    RawEdge {
        anchor,
        leaf,
        kind,
        flow,
        optional,
        anchor_label: None,
        leaf_label: None,
        leaf_kind: None,
    }
}

pub(super) enum Step<'a> {
    Edge(RawEdge<'a>),
    Keep(&'a [usize]),
}

/// Independent padded-row oracle: raw physical edge scans, explicit bindings,
/// one null-padded row only when a whole optional step finds no match, and
/// bag-preserving projection. No AST, index, degree formula or executor helpers.
pub(super) fn literal_count(graph: &Graph, steps: &[Step<'_>]) -> i64 {
    let mut rows: Vec<_> = (0..graph.nodes.len())
        .map(|vertex| [Some(vertex), None, None, None, None, None])
        .collect();
    for step in steps {
        let Step::Edge(spec) = step else {
            let Step::Keep(keep) = step else {
                unreachable!()
            };
            for row in &mut rows {
                for (slot, binding) in row.iter_mut().enumerate() {
                    if !keep.contains(&slot) {
                        *binding = None;
                    }
                }
            }
            continue;
        };
        let mut next = Vec::new();
        for row in rows {
            let before = next.len();
            let anchor = &graph.nodes[row[spec.anchor].expect("mandatory anchor")];
            for edge in &graph.edges {
                if edge.label.as_str() != spec.kind
                    || spec
                        .anchor_label
                        .is_some_and(|label| anchor.label.as_str() != label)
                {
                    continue;
                }
                let endpoint = match spec.flow {
                    Flow::Out if edge.from == anchor.id => Some(&edge.to),
                    Flow::In if edge.to == anchor.id => Some(&edge.from),
                    Flow::Either if edge.from == anchor.id => Some(&edge.to),
                    Flow::Either if edge.to == anchor.id => Some(&edge.from),
                    _ => None,
                };
                let Some(endpoint) = endpoint else { continue };
                let vertex = graph
                    .nodes
                    .iter()
                    .position(|node| &node.id == endpoint)
                    .unwrap();
                let leaf = &graph.nodes[vertex];
                if spec
                    .leaf_label
                    .is_some_and(|label| leaf.label.as_str() != label)
                    || spec.leaf_kind.is_some_and(|kind| {
                        leaf.props.get("kind") != Some(&Value::String(kind.into()))
                    })
                {
                    continue;
                }
                let mut extended = row;
                extended[spec.leaf] = Some(vertex);
                next.push(extended);
            }
            if spec.optional && next.len() == before {
                let mut padded = row;
                padded[spec.leaf] = None;
                next.push(padded);
            }
        }
        rows = next;
    }
    rows.len() as i64
}
