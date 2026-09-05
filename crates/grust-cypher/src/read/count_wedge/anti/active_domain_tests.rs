//! Chunked mask scans preserve domain contents and cumulative work charges.

use super::*;
use std::time::{Duration, Instant};

const LENGTHS: [usize; 5] = [0, 1, 255, 256, 257];

fn limits(work: usize) -> read_budget::ReadExecutionBudgetLimits {
    read_budget::ReadExecutionBudgetLimits {
        max_candidate_work: work,
        max_intermediate_bytes: usize::MAX,
        max_range_items: 1,
        deadline: Instant::now() + Duration::from_secs(5),
    }
}

#[test]
fn mask_patterns_and_chunk_boundaries_preserve_domains_and_exact_work() {
    for len in LENGTHS {
        for pattern in [&[0][..], &[8], &[A | B | C], &[0, A, 8, B, C, A | B | C]] {
            let masks: Vec<_> = pattern.iter().copied().cycle().take(len).collect();
            let expected_vertices: Vec<_> = masks
                .iter()
                .enumerate()
                .filter(|(_, mask)| **mask & (A | B | C) != 0)
                .map(|(vertex, _)| vertex as u32)
                .collect();
            let mut expected_slots = vec![u32::MAX; len];
            for (slot, &vertex) in expected_vertices.iter().enumerate() {
                expected_slots[vertex as usize] = slot as u32;
            }
            // Both scans charge len, as does inverse-map initialization.
            // Preserve charges already incurred by the enclosing executor.
            let actual = read_budget::with_budget(limits(7 + 3 * len), || {
                read_budget::charge_candidate_work(7, "work before active-domain scans")?;
                let actual = active_domain(&masks)?;
                let error = read_budget::charge_candidate_work(1, "probing remaining work")
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("candidate-work units"), "{error}");
                assert!(error.ends_with("while probing remaining work"), "{error}");
                Ok(actual)
            })
            .unwrap();
            assert_eq!(actual, (expected_vertices, expected_slots), "len={len}");
        }
    }
}

#[test]
fn insufficient_work_refuses_the_next_chunk_or_inverse_initialization() {
    let masks = [A | B | C; 257];
    for (work, spent, context) in [
        (0, 0, "sizing anti-wedge active vertices"),
        (255, 0, "sizing anti-wedge active vertices"),
        (256, 256, "sizing anti-wedge active vertices"),
        (257, 257, "initializing anti-wedge inverse map"),
        (513, 257, "initializing anti-wedge inverse map"),
        (514, 514, "indexing anti-wedge active vertices"),
        (769, 514, "indexing anti-wedge active vertices"),
        (770, 770, "indexing anti-wedge active vertices"),
    ] {
        let error = read_budget::with_budget(limits(work), || {
            let error = active_domain(&masks).unwrap_err().to_string();
            // A refused chunk consumes no partial scan work. Probe the exact
            // remainder to distinguish precharging from per-entry charging.
            read_budget::charge_candidate_work(work - spent, "probing refused chunk remainder")?;
            assert!(read_budget::charge_candidate_work(1, "probing exhausted work").is_err());
            Ok(error)
        })
        .unwrap();
        assert!(error.contains("candidate-work units"), "{error}");
        assert!(error.ends_with(&format!("while {context}")), "{error}");
    }
    for len in LENGTHS.into_iter().filter(|&len| len > 0) {
        let error = read_budget::with_budget(limits(3 * len - 1), || active_domain(&masks[..len]))
            .unwrap_err()
            .to_string();
        assert!(
            error.ends_with("while indexing anti-wedge active vertices"),
            "len={len}: {error}"
        );
    }
}

#[test]
fn expired_deadlines_refuse_even_an_empty_domain() {
    for len in LENGTHS {
        let masks = vec![A; len];
        let mut budget = limits(usize::MAX);
        budget.deadline = Instant::now() - Duration::from_secs(1);
        let error = read_budget::with_budget(budget, || active_domain(&masks))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("bounded read execution timed out"),
            "{error}"
        );
    }
}
