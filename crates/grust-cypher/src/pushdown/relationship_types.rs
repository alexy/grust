//! A portable SQL join cannot infer physical edge identity from nullable IDs.
//! Distinct type sets prove that positions in one MATCH cannot reuse an edge.

use std::collections::HashMap;

pub(super) fn pairwise_disjoint_types<'a>(
    positions: impl IntoIterator<Item = &'a [String]>,
) -> bool {
    let mut positions = positions.into_iter();
    let Some(first) = positions.next() else {
        return true;
    };
    let Some(second) = positions.next() else {
        return true;
    };
    let mut owners = HashMap::new();
    for (position, types) in std::iter::once(first)
        .chain(std::iter::once(second))
        .chain(positions)
        .enumerate()
    {
        if types.is_empty() {
            return false;
        }
        // Repeated alternatives at one position do not bind another edge.
        for kind in types {
            if owners
                .insert(kind.as_str(), position)
                .is_some_and(|owner| owner != position)
            {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pushdown::*;

    #[test]
    fn type_sets_distinguish_positions_from_duplicate_alternatives() {
        for (input, expected) in [
            (vec![], true),
            (vec![vec![]], true),
            (vec![vec![], vec![]], false),
            (vec![vec!["R"], vec![]], false),
            (vec![vec![], vec!["R"]], false),
            (vec![vec!["R"], vec!["R"]], false),
            (vec![vec!["R", "S"], vec!["S", "T"]], false),
            (vec![vec!["R", "R"], vec!["S"]], true),
            (vec![vec!["R"], vec!["r"]], true),
        ] {
            let input: Vec<Vec<String>> = input
                .into_iter()
                .map(|types| types.into_iter().map(str::to_string).collect())
                .collect();
            assert_eq!(
                pairwise_disjoint_types(input.iter().map(Vec::as_slice)),
                expected
            );
        }
    }

    #[test]
    fn both_join_lowerers_and_delegates_decline_overlap() {
        let params = crate::CypherParameters::new();
        for source in [
            "MATCH ()-[:R]->()-[:R]->() RETURN count(*)",
            "MATCH ()-[:R]->()<-[:R]-() RETURN count(*)",
            "MATCH ()-[:R]-()-[:R]-() RETURN count(*)",
            "MATCH ()-[:R|S]->()-[:S|T]->() RETURN count(*)",
            "MATCH ()-[]->()-[:R]->() RETURN count(*)",
            "MATCH ()-[:R]->(), ()-[:R]->() RETURN count(*)",
            "MATCH (a)-[:R]->(b), (a)-[:R]->(b) RETURN count(*)",
            "MATCH ()-[:R]->()-[:R]->() RETURN count(*) UNION ALL MATCH () RETURN count(*)",
            "MATCH ()-[:R]->()-[:R]->() WITH count(*) AS n RETURN n",
            "CALL { MATCH ()-[:R]->()-[:R]->() RETURN count(*) AS n } RETURN n",
        ] {
            assert!(
                plan_segment_read(source, &params).unwrap().is_none(),
                "{source}"
            );
            assert!(
                plan_read(source, &params, &NoTypeHints).unwrap().is_none(),
                "{source}"
            );
        }
        for source in [
            "MATCH ()-[]->() RETURN count(*)",
            "MATCH ()-[:R|R]->()-[:S]->() RETURN count(*)",
            "MATCH ()-[:R]->(), ()-[:S]->() RETURN count(*)",
            "MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN count(*)",
        ] {
            assert!(
                plan_read(source, &params, &NoTypeHints).unwrap().is_some(),
                "{source}"
            );
        }
    }
}
