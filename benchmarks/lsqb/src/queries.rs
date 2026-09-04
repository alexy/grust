use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

pub const LSQB_COMMIT: &str = "242cb2fd31340ca688954cb94794d74c0d5b6f92";
pub const LSQB_TREE: &str = "d99fab28d47791dbc0e7173abc4c66d8aadc64ca";
pub const LSQB_QUERY_TREE: &str = "50937f3d075245e2abd4c00a36c4b3c236766265";
pub const LSQB_EXAMPLE_DATA_TREE: &str = "45181e6b274d014f8626038e1d398fa1b9e4c19d";

const QUERY_SHA256: [&str; 9] = [
    "e08571bf8c877508cd3745ee7dcc8c061259c3b4d5a3fb17a952fbac4f9145ed",
    "49df895df6b98037f6ce5972d9113ae7a3f263ec009e4fece9ec75bb47410b32",
    "5eb0a515fb894e91ecbc1f40656e757884a234953bf87565aec5fcc85007e5be",
    "1675c38f7cc117ba9be4de8fc0c1d7073901a03cfc5a174f2241e14c602c9a9f",
    "d836a819c7c96819f5dfa14d4e0097bcfb2222eb1561d3ffda22f15067cd8179",
    "b8c550aa3c452a99fe65d22ae23e7461ee2705b8be45e96a1d2d1d6143880c93",
    "3cb1b31a3f2d05efcaaebaea5ef70231bff30dcfe85aca21cb011ee99175ef0f",
    "73fbce9474f0059cfc620c17c5016d19b0974cdbbc8f26637f51b86ea4f24b91",
    "f0e3402d9bf03cf2e01bf5d812399a4a2f2d40d4558a6ac04d09f0a77afbc8e4",
];

#[derive(Clone, Debug)]
pub struct QueryCase {
    pub id: String,
    pub executable: String,
    pub source_sha256: String,
    pub expected_count: i64,
    pub claim: String,
}

pub fn load_baseline(lsqb_root: &Path) -> Result<Vec<QueryCase>, String> {
    let expected = [8, 3, 6, 8, 3, 8, 11, 2, 4];
    (1..=9)
        .map(|number| {
            let path = lsqb_root.join(format!("cypher/q{number}.cypher"));
            let source = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            let actual_hash = sha256(source.as_bytes());
            let wanted_hash = QUERY_SHA256[number - 1];
            if actual_hash != wanted_hash {
                return Err(format!(
                    "q{number} differs from LSQB {LSQB_COMMIT}: expected {wanted_hash}, got {actual_hash}"
                ));
            }
            Ok(QueryCase {
                id: format!("q{number}"),
                executable: adapt_message_inheritance(&source),
                source_sha256: actual_hash,
                expected_count: expected[number - 1],
                claim: "unchanged LSQB source; Grust adapter lowers Post/Comment inheritance and names q8/q9 anti-join edges"
                    .to_string(),
            })
        })
        .collect()
}

pub fn load_adversarial(directory: &Path) -> Result<Vec<QueryCase>, String> {
    let manifest = [
        (
            "a1-reversed-chain",
            8,
            "equivalent q1 with the entire chain reversed",
        ),
        (
            "a2-reordered-join",
            3,
            "equivalent q2 with reordered join atoms",
        ),
        (
            "a3-split-match",
            8,
            "equivalent q4 split across MATCH clauses",
        ),
        (
            "a4-optional-fanout",
            11,
            "q7 optional fanout with explicit WITH boundary",
        ),
        (
            "a5-negated-pattern",
            2,
            "q8 anti-join with reordered predicates",
        ),
        (
            "a6-range-expansion",
            10_000,
            "bounded scalar range and UNWIND amplification",
        ),
        (
            "a7-cartesian-count",
            125,
            "three-way Cartesian product cardinality",
        ),
        (
            "a8-union-dedup",
            5,
            "UNION deduplication of identical aggregate rows",
        ),
    ];
    manifest
        .iter()
        .map(|(id, expected_count, claim)| {
            let path = directory.join(format!("{id}.cypher"));
            let source = fs::read_to_string(&path)
                .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
            Ok(QueryCase {
                id: (*id).to_string(),
                executable: source.clone(),
                source_sha256: sha256(source.as_bytes()),
                expected_count: *expected_count,
                claim: (*claim).to_string(),
            })
        })
        .collect()
}

fn adapt_message_inheritance(source: &str) -> String {
    source
        .replace(":Post", ":Message {kind: 'Post'}")
        .replace(":Comment", ":Message {kind: 'Comment'}")
        // Grust does not parse openCypher's abbreviated `NOT (a)-[:T]->(b)`
        // pattern predicate yet. Lower the two LSQB anti-joins to the
        // equivalent OPTIONAL MATCH + NULL test, while retaining the exact
        // upstream source and its digest in the report.
        .replace(
            "WHERE NOT (comment)-[:HAS_TAG]->(tag1)\n  AND tag1 <> tag2",
            "OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1)\nWITH tag1, tag2, h\nWHERE h IS NULL AND tag1 <> tag2",
        )
        .replace(
            "WHERE NOT (person1)-[:KNOWS]-(person3)\n  AND person1 <> person3",
            "OPTIONAL MATCH (person1)-[k:KNOWS]-(person3)\nWITH person1, person3, tag, k\nWHERE k IS NULL AND person1 <> person3",
        )
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_only_lowers_inherited_message_types() {
        let source = "MATCH (c:Comment)-[:REPLY_OF]->(p:Post), (m:Message) RETURN count(*)";
        assert_eq!(
            adapt_message_inheritance(source),
            "MATCH (c:Message {kind: 'Comment'})-[:REPLY_OF]->(p:Message {kind: 'Post'}), (m:Message) RETURN count(*)"
        );
    }

    #[test]
    fn adapter_lowers_negated_patterns() {
        let query = "WHERE NOT (comment)-[:HAS_TAG]->(tag1)\n  AND tag1 <> tag2";
        assert!(adapt_message_inheritance(query).contains("WHERE h IS NULL"));
    }
}
