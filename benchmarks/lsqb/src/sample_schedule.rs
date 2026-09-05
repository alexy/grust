//! Shared query rotation for portable and native-engine measurement cohorts.
//! Callers restart the zero-based iteration when moving from warm-up to measurement.
pub fn rotated_index(position: usize, iteration: usize, query_count: usize) -> usize {
    (position + iteration) % query_count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_round_visits_every_query_and_rotates_its_start() {
        for iteration in 0..10 {
            let round: Vec<_> = (0..3)
                .map(|position| rotated_index(position, iteration, 3))
                .collect();
            assert_eq!(round[0], iteration % 3);
            let mut sorted = round;
            sorted.sort_unstable();
            assert_eq!(sorted, vec![0, 1, 2]);
        }
    }
}
