//! Location-weighted triangle arithmetic over the shared support topology.

use super::{
    add,
    location::{Locations, cube, product3, product21},
    multiply,
};
use crate::read::count_support::{OrientedSupport, WeightedEdge};
use crate::{Result, read_budget};

pub(super) fn add_distinct(
    support: &OrientedSupport,
    locations: &Locations,
    total: &mut u128,
) -> Result<()> {
    support.visit_triangles(|triangle| {
        let location = product3(
            locations.at(triangle.x),
            locations.at(triangle.y),
            locations.at(triangle.z),
        )?;
        if location != 0 {
            let mut contribution = multiply(
                triangle.xy,
                triangle.xz,
                "multiplying triangle edge multiplicities",
            )?;
            contribution = multiply(
                contribution,
                triangle.yz,
                "multiplying triangle edge multiplicities",
            )?;
            contribution = multiply(
                contribution,
                location,
                "multiplying triangle location multiplicity",
            )?;
            contribution = multiply(contribution, 6, "ordering distinct triangle vertices")?;
            *total = add(*total, contribution, "summing distinct triangles")?;
        }
        Ok(())
    })
}

pub(super) fn add_repeated(
    edges: &[WeightedEdge],
    loops: &[u128],
    locations: &Locations,
    total: &mut u128,
) -> Result<()> {
    // Exactly two equal vertices: choose one loop at the repeated endpoint and
    // an ordered pair of distinct physical cross-edge slots. Three placements
    // put the singleton in each triangle position.
    for edge in edges {
        read_budget::charge_candidate_work(1, "counting repeated triangle vertices")?;
        let multiplicity = u128::from(edge.multiplicity);
        if multiplicity < 2 {
            continue;
        }
        let pair = multiply(
            multiplicity,
            multiplicity - 1,
            "choosing distinct cross edges",
        )?;
        for (repeated, single) in [
            (edge.a as usize, edge.b as usize),
            (edge.b as usize, edge.a as usize),
        ] {
            read_budget::charge_candidate_work(1, "placing repeated triangle vertices")?;
            if loops[repeated] == 0 {
                continue;
            }
            let location = product21(locations.at(repeated), locations.at(single))?;
            if location == 0 {
                continue;
            }
            let mut contribution =
                multiply(loops[repeated], pair, "choosing loop and cross edges")?;
            contribution = multiply(
                contribution,
                location,
                "multiplying repeated-vertex locations",
            )?;
            contribution = multiply(contribution, 3, "placing the repeated triangle vertex")?;
            *total = add(*total, contribution, "summing repeated-vertex triangles")?;
        }
    }

    // All vertices equal: the three path slots are an ordered injection from
    // the self-loop slots, while the three location MATCH clauses are
    // independent and therefore contribute the cubed location weight.
    for (person, &self_loops) in loops.iter().enumerate() {
        read_budget::charge_candidate_work(1, "counting all-equal triangle vertices")?;
        if self_loops < 3 {
            continue;
        }
        let choices = multiply(self_loops, self_loops - 1, "choosing triangle self-loops")?;
        let choices = multiply(choices, self_loops - 2, "choosing triangle self-loops")?;
        let location = cube(locations.at(person))?;
        if location != 0 {
            let contribution = multiply(choices, location, "multiplying all-equal locations")?;
            *total = add(*total, contribution, "summing all-equal triangles")?;
        }
    }
    Ok(())
}
