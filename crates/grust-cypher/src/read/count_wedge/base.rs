//! One grouped-neighbor pass for the non-anti wedge count.

use super::*;

#[derive(Debug, Default, PartialEq)]
struct CenterTotals {
    // For one center b, the exact sum over c is degree_a * sum_C(m * L)
    // minus sum_A_and_C(m * m * L).
    // Incident T multiplicities partition at most E_T physical slots. With
    // distinct T/U types and the index's u32 edge bound, sum(m * L) is at most
    // E_T * E_U < 2^62. Only overlap and final products need wider arithmetic.
    degree_a: u32,
    weighted_leaves: u64,
    overlap: u128,
}

impl CenterTotals {
    fn add_group(&mut self, multiplicity: u32, matches_a: bool, leaves: u64) -> Result<()> {
        if matches_a {
            self.degree_a = self
                .degree_a
                .checked_add(multiplicity)
                .ok_or_else(|| gql_execution("count wedge degree exceeds the index edge bound"))?;
        }
        if leaves == 0 {
            return Ok(());
        }
        let weighted = u64::from(multiplicity)
            .checked_mul(leaves)
            .ok_or_else(weighted_leaves_overflow)?;
        self.weighted_leaves = self
            .weighted_leaves
            .checked_add(weighted)
            .ok_or_else(weighted_leaves_overflow)?;
        if matches_a {
            let overlap = u128::from(weighted)
                .checked_mul(u128::from(multiplicity))
                .ok_or_else(arithmetic_overflow)?;
            self.overlap = self
                .overlap
                .checked_add(overlap)
                .ok_or_else(arithmetic_overflow)?;
        }
        Ok(())
    }

    fn contribution(&self) -> Result<u128> {
        u128::from(self.degree_a)
            .checked_mul(u128::from(self.weighted_leaves))
            .ok_or_else(arithmetic_overflow)?
            .checked_sub(self.overlap)
            .ok_or_else(|| gql_execution("count wedge overlap exceeds exact product"))
    }

    fn add_to(&self, count: &mut u128) -> Result<()> {
        *count = count
            .checked_add(self.contribution()?)
            .ok_or_else(arithmetic_overflow)?;
        Ok(())
    }
}

fn weighted_leaves_overflow() -> GrustError {
    gql_execution("count wedge weighted leaves exceed u64")
}

pub(super) fn count(
    index: &TypedGraphIndex,
    relationship: &str,
    masks: &[u8],
    leaves: &[u64],
    centers: impl Iterator<Item = usize>,
) -> Result<u128> {
    let mut count = 0u128;
    for center in centers {
        read_budget::charge_candidate_work(1, "counting wedge centers")?;
        if masks[center] & 2 == 0 {
            continue;
        }
        let mut totals = CenterTotals::default();
        groups(
            index,
            center as u32,
            relationship,
            |vertex, multiplicity| {
                let vertex = vertex as usize;
                let leaf_count = if masks[vertex] & 4 != 0 {
                    leaves[vertex]
                } else {
                    0
                };
                totals.add_group(multiplicity, masks[vertex] & 1 != 0, leaf_count)
            },
        )?;
        totals.add_to(&mut count)?;
    }
    Ok(count)
}

#[cfg(test)]
mod tests;
