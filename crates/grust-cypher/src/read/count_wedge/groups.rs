//! Merge physical adjacency slots with bounded scan-work prepayment.

use super::*;

const SCAN_CHUNK_SIZE: usize = 256;

/// Both rows are consumed completely on success. Prepay only the next bounded
/// chunk, even when all its slots belong to one large parallel-edge group.
/// Successful work totals are exact; a tight budget can refuse a whole chunk
/// before doing the partially affordable work, as with anti-wedge mask scans.
#[inline]
fn charge_scan_chunk(scanned: usize, total: usize) -> Result<()> {
    if scanned.is_multiple_of(SCAN_CHUNK_SIZE) {
        read_budget::charge_candidate_work(
            (total - scanned).min(SCAN_CHUNK_SIZE),
            "scanning count wedge edges",
        )?;
    }
    Ok(())
}

/// A group counts physical T edges once, so the global index edge bound also
/// bounds its multiplicity. Raw scan lengths can be larger because self-loops
/// occupy both rows; keep those lengths and their accounting in usize.
fn compact_multiplicity(outgoing: usize, incoming: usize) -> Result<u32> {
    outgoing
        .checked_add(incoming)
        .and_then(|total| u32::try_from(total).ok())
        .ok_or_else(|| gql_execution("count wedge multiplicity exceeds the index edge bound"))
}

/// Reciprocal and parallel edges remain distinct. Incoming self-loop entries
/// are charged but skipped because their outgoing entries already count them.
/// Every grouped endpoint retains its own charge before invoking the visitor.
pub(super) fn groups(
    index: &TypedGraphIndex,
    center: u32,
    relationship: &str,
    mut visit: impl FnMut(u32, u32) -> Result<()>,
) -> Result<()> {
    let outgoing = index.outgoing(center, relationship);
    let incoming = index.incoming(center, relationship);
    let total = outgoing
        .len()
        .checked_add(incoming.len())
        .ok_or_else(arithmetic_overflow)?;
    let (mut out, mut inc) = (0, 0);
    while out < outgoing.len() || inc < incoming.len() {
        let vertex = match (outgoing.get(out), incoming.get(inc)) {
            (Some(a), Some(b)) => a.vertex.min(b.vertex),
            (Some(a), None) => a.vertex,
            (None, Some(b)) => b.vertex,
            (None, None) => unreachable!(),
        };
        let (out_start, inc_start) = (out, inc);
        while outgoing.get(out).is_some_and(|next| next.vertex == vertex) {
            charge_scan_chunk(out + inc, total)?;
            out += 1;
        }
        while incoming.get(inc).is_some_and(|next| next.vertex == vertex) {
            charge_scan_chunk(out + inc, total)?;
            inc += 1;
        }
        let incoming_count = if vertex == center { 0 } else { inc - inc_start };
        let multiplicity = compact_multiplicity(out - out_start, incoming_count)?;
        read_budget::charge_candidate_work(1, "grouping count wedge neighbors")?;
        visit(vertex, multiplicity)?;
    }
    // Cover empty rows and time spent in the final visitor. No TLS borrow is
    // held across the visitor, which may itself perform budgeted operations.
    read_budget::checkpoint()
}

#[cfg(test)]
mod tests;
