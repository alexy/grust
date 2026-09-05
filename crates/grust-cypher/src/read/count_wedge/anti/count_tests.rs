use super::*;

fn add(total: &mut u128, term: Result<u128>) {
    *total = total.checked_add(term.unwrap()).unwrap();
}

#[test]
fn triangle_subtraction_uses_only_the_two_incident_arm_weights() {
    let masks = [A | B | C; 3];
    let leaves = [1, 2, 4];
    let weights = [[0, 2, 3], [2, 0, 5], [3, 5, 0]];
    let degrees = [5, 7, 8];
    let mut base = 0;
    for (b, row) in weights.iter().enumerate() {
        for (c, &multiplicity) in row.iter().enumerate() {
            if b != c {
                add(
                    &mut base,
                    base_direction(&masks, &leaves, degrees[b], b, c, multiplicity),
                );
            }
        }
    }
    let mut excluded = 0;
    for [a, b, c] in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        add(
            &mut excluded,
            triangle_placement(&masks, &leaves, a, b, c, weights[a][b], weights[b][c]),
        );
    }
    // Multiplying the 2*3*5 closure product by six would be 180 before
    // leaves. The exact two-arm terms instead cancel at 131.
    assert_eq!((base, excluded, base - excluded), (131, 131, 0));
}

#[test]
fn cancellation_happens_in_u128_without_an_intermediate_i64_cap() {
    let masks = [A | B | C; 3];
    let leaves = [2_000_000; 3];
    let multiplicity = 2_000_000;
    let degree = 2 * multiplicity;
    let mut base = 0;
    let mut excluded = 0;
    for [a, b, c] in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        add(
            &mut base,
            base_direction(&masks, &leaves, degree, b, c, multiplicity),
        );
        add(
            &mut excluded,
            triangle_placement(&masks, &leaves, a, b, c, multiplicity, multiplicity),
        );
    }
    assert_eq!(base, 48_000_000_000_000_000_000);
    assert!(base > i64::MAX as u128);
    assert_eq!(base.checked_sub(excluded), Some(0));
    assert!(triangle_placement(&masks, &leaves, 0, 1, 2, u128::MAX, 2).is_err());
}
