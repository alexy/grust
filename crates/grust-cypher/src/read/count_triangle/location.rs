use super::*;
use grust_core::{TypedGraphIndex, TypedNeighbor};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
struct Group {
    vertex: u32,
    multiplicity: u128,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocationTerm {
    pub(super) person: u32,
    pub(super) country: u32,
    pub(super) weight: u128,
}

pub(super) struct Locations {
    rows: Vec<LocationTerm>,
    offsets: Vec<usize>,
}

impl Locations {
    pub(super) fn at(&self, person: usize) -> &[LocationTerm] {
        &self.rows[self.offsets[person]..self.offsets[person + 1]]
    }
}

/// Group one sorted directed CSR slice by endpoint. Filtering happens only
/// after all physical edge slots in a group contribute to its multiplicity.
/// The caller reserves at least the raw slice length, so this routine cannot
/// trigger an unaccounted capacity increase.
fn directed_groups(
    neighbors: &[TypedNeighbor],
    graph: &Graph,
    label: &str,
    groups: &mut Vec<Group>,
) -> Result<()> {
    groups.clear();
    let mut cursor = 0;
    while cursor < neighbors.len() {
        let vertex = neighbors[cursor].vertex;
        let mut multiplicity = 0u128;
        while neighbors
            .get(cursor)
            .is_some_and(|neighbor| neighbor.vertex == vertex)
        {
            read_budget::charge_candidate_work(1, "scanning triangle location edges")?;
            multiplicity = add(multiplicity, 1, "grouping physical location edge slots")?;
            cursor += 1;
        }
        read_budget::charge_candidate_work(1, "grouping triangle location endpoints")?;
        if graph.nodes[vertex as usize].label.as_str() == label {
            if groups.len() == groups.capacity() {
                return Err(gql_execution(
                    "count triangle location group exceeded its proven capacity",
                ));
            }
            groups.push(Group {
                vertex,
                multiplicity,
            });
        }
    }
    Ok(())
}

fn group_capacities(
    index: &TypedGraphIndex,
    triangle: &Triangle<'_>,
    cities: &[u32],
) -> Result<(usize, usize)> {
    let (mut incoming, mut outgoing) = (0, 0);
    for &city in cities {
        // Charge the city even when both typed degrees are zero.
        read_budget::charge_candidate_work(1, "sizing triangle location groups")?;
        incoming = incoming.max(index.incoming(city, triangle.located_type).len());
        outgoing = outgoing.max(index.outgoing(city, triangle.part_type).len());
    }
    Ok((incoming, outgoing))
}

fn regroup_city(
    index: &TypedGraphIndex,
    triangle: &Triangle<'_>,
    city: u32,
    incoming_people: &mut Vec<Group>,
    outgoing_countries: &mut Vec<Group>,
) -> Result<()> {
    directed_groups(
        index.incoming(city, triangle.located_type),
        index.graph(),
        triangle.person_label,
        incoming_people,
    )?;
    directed_groups(
        index.outgoing(city, triangle.part_type),
        index.graph(),
        triangle.country_label,
        outgoing_countries,
    )
}

fn count_terms(
    index: &TypedGraphIndex,
    triangle: &Triangle<'_>,
    cities: &[u32],
    person_slot: &[u32],
    incoming_people: &mut Vec<Group>,
    outgoing_countries: &mut Vec<Group>,
) -> Result<usize> {
    let mut count = 0usize;
    for &city in cities {
        // `regroup_city` may see no typed edges, so account the outer visit.
        read_budget::charge_candidate_work(1, "sizing triangle location joins")?;
        regroup_city(index, triangle, city, incoming_people, outgoing_countries)?;
        for person in incoming_people.iter() {
            read_budget::charge_candidate_work(1, "sizing triangle person groups")?;
            if person_slot[person.vertex as usize] != u32::MAX {
                count = count.checked_add(outgoing_countries.len()).ok_or_else(|| {
                    gql_execution("count triangle location term count overflowed")
                })?;
            }
        }
    }
    Ok(count)
}

fn fill_terms(
    index: &TypedGraphIndex,
    triangle: &Triangle<'_>,
    cities: &[u32],
    person_slot: &[u32],
    incoming_people: &mut Vec<Group>,
    outgoing_countries: &mut Vec<Group>,
    terms: &mut Vec<LocationTerm>,
) -> Result<()> {
    for &city in cities {
        read_budget::charge_candidate_work(1, "visiting triangle location joins")?;
        regroup_city(index, triangle, city, incoming_people, outgoing_countries)?;
        for person in incoming_people.iter() {
            let ordinal = person_slot[person.vertex as usize];
            if ordinal == u32::MAX {
                continue;
            }
            for country in outgoing_countries.iter() {
                read_budget::charge_candidate_work(1, "joining triangle location groups")?;
                let weight = multiply(
                    person.multiplicity,
                    country.multiplicity,
                    "multiplying location-path multiplicities",
                )?;
                if terms.len() == terms.capacity() {
                    return Err(gql_execution(
                        "count triangle location terms exceeded their proven capacity",
                    ));
                }
                terms.push(LocationTerm {
                    person: ordinal,
                    country: country.vertex,
                    weight,
                });
            }
        }
    }
    Ok(())
}

fn coalesce(mut terms: Vec<LocationTerm>) -> Result<Vec<LocationTerm>> {
    read_budget::checkpoint()?;
    read_budget::charge_candidate_work(
        sort_work(terms.len()),
        "sorting triangle location weights",
    )?;
    terms.sort_unstable_by_key(|term| (term.person, term.country));
    read_budget::checkpoint()?;

    // LocationTerm is Copy, so compact the reserved term buffer in place and
    // retain its already-accounted capacity instead of allocating a second
    // result vector.
    let mut written = 0usize;
    for read in 0..terms.len() {
        read_budget::charge_candidate_work(1, "coalescing triangle location weights")?;
        let term = terms[read];
        if written != 0
            && terms[written - 1].person == term.person
            && terms[written - 1].country == term.country
        {
            terms[written - 1].weight = add(
                terms[written - 1].weight,
                term.weight,
                "adding location paths",
            )?;
        } else {
            terms[written] = term;
            written += 1;
        }
    }
    terms.truncate(written);
    Ok(terms)
}

pub(super) fn build(
    index: &TypedGraphIndex,
    triangle: &Triangle<'_>,
    person_slot: &[u32],
    person_count: usize,
) -> Result<Locations> {
    let cities = index.vertices_with_label(triangle.city_label);
    let (incoming_capacity, outgoing_capacity) = group_capacities(index, triangle, cities)?;
    let mut incoming_people = reserved_vec(
        incoming_capacity,
        "allocating triangle incoming-location groups",
    )?;
    let mut outgoing_countries = reserved_vec(
        outgoing_capacity,
        "allocating triangle outgoing-location groups",
    )?;
    let term_count = count_terms(
        index,
        triangle,
        cities,
        person_slot,
        &mut incoming_people,
        &mut outgoing_countries,
    )?;
    let mut terms = reserved_vec(term_count, "allocating triangle location terms")?;
    fill_terms(
        index,
        triangle,
        cities,
        person_slot,
        &mut incoming_people,
        &mut outgoing_countries,
        &mut terms,
    )?;
    if terms.len() != term_count {
        return Err(gql_execution(
            "count triangle location terms changed between sizing and fill",
        ));
    }
    let rows = coalesce(terms)?;

    let offset_count = person_count
        .checked_add(1)
        .ok_or_else(|| gql_execution("count triangle person offset count overflowed"))?;
    read_budget::charge_candidate_work(offset_count, "initializing triangle location offsets")?;
    let mut offsets = reserved_vec(offset_count, "allocating triangle location offsets")?;
    offsets.resize(offset_count, 0usize);
    for row in &rows {
        read_budget::charge_candidate_work(1, "counting triangle location offsets")?;
        let slot = row.person as usize + 1;
        offsets[slot] = offsets[slot]
            .checked_add(1)
            .ok_or_else(|| gql_execution("count triangle location offset overflowed"))?;
    }
    for slot in 1..offsets.len() {
        read_budget::charge_candidate_work(1, "prefixing triangle location offsets")?;
        offsets[slot] = offsets[slot]
            .checked_add(offsets[slot - 1])
            .ok_or_else(|| gql_execution("count triangle location offset overflowed"))?;
    }
    Ok(Locations { rows, offsets })
}

pub(super) fn product3(
    first: &[LocationTerm],
    second: &[LocationTerm],
    third: &[LocationTerm],
) -> Result<u128> {
    let (mut a, mut b, mut c) = (0, 0, 0);
    let mut total = 0;
    while a < first.len() && b < second.len() && c < third.len() {
        read_budget::charge_candidate_work(1, "intersecting three triangle country sets")?;
        let country = first[a]
            .country
            .max(second[b].country)
            .max(third[c].country);
        if first[a].country == country
            && second[b].country == country
            && third[c].country == country
        {
            let weight = multiply(
                first[a].weight,
                second[b].weight,
                "multiplying three location weights",
            )?;
            let weight = multiply(
                weight,
                third[c].weight,
                "multiplying three location weights",
            )?;
            total = add(total, weight, "summing three-way location weights")?;
            a += 1;
            b += 1;
            c += 1;
        } else {
            // Advance at most one entry from each list per charged iteration;
            // a sparse country gap cannot become an unmetered linear walk.
            if first[a].country < country {
                a += 1;
            }
            if second[b].country < country {
                b += 1;
            }
            if third[c].country < country {
                c += 1;
            }
        }
    }
    Ok(total)
}

pub(super) fn product21(repeated: &[LocationTerm], single: &[LocationTerm]) -> Result<u128> {
    let (mut left, mut right) = (0, 0);
    let mut total = 0;
    while left < repeated.len() && right < single.len() {
        read_budget::charge_candidate_work(1, "intersecting two triangle country sets")?;
        match repeated[left].country.cmp(&single[right].country) {
            Ordering::Less => left += 1,
            Ordering::Greater => right += 1,
            Ordering::Equal => {
                let square = multiply(
                    repeated[left].weight,
                    repeated[left].weight,
                    "squaring repeated location weight",
                )?;
                let weight = multiply(
                    square,
                    single[right].weight,
                    "multiplying repeated location weights",
                )?;
                total = add(total, weight, "summing repeated location weights")?;
                left += 1;
                right += 1;
            }
        }
    }
    Ok(total)
}

pub(super) fn cube(rows: &[LocationTerm]) -> Result<u128> {
    let mut total = 0;
    for row in rows {
        read_budget::charge_candidate_work(1, "cubing triangle location weights")?;
        let square = multiply(row.weight, row.weight, "squaring location weight")?;
        let cube = multiply(square, row.weight, "cubing location weight")?;
        total = add(total, cube, "summing cubed location weights")?;
    }
    Ok(total)
}
