use crate::dataset::DatasetFingerprint;
use crate::queries::DatasetStats;
use crate::report::DatasetIdentityV2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct KnownDataset {
    scale: &'static str,
    source_url: &'static str,
    archive_sha256: Option<&'static str>,
    archive_bytes: Option<u64>,
    extracted_manifest_sha256: &'static str,
    csv_files: usize,
    csv_bytes: u64,
    nodes: usize,
    edges: usize,
    person_nodes: usize,
}

const DATASETS: [KnownDataset; 3] = [
    KnownDataset {
        scale: "example",
        source_url: "https://github.com/ldbc/lsqb/tree/242cb2fd31340ca688954cb94794d74c0d5b6f92/data/social-network-sfexample-projected-fk",
        archive_sha256: None,
        archive_bytes: None,
        extracted_manifest_sha256: "e47d935e186ccda58147fc2609d3db1a6f0e218b92384cf63a7161e2c2974def",
        csv_files: 36,
        csv_bytes: 1_361,
        nodes: 28,
        edges: 72,
        person_nodes: 5,
    },
    KnownDataset {
        scale: "0.1",
        source_url: "https://datasets.ldbcouncil.org/lsqb/social-network-sf0.1-projected-fk.tar.zst",
        archive_sha256: Some("20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005"),
        archive_bytes: Some(6_362_514),
        extracted_manifest_sha256: "c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085",
        csv_files: 36,
        csv_bytes: 53_863_509,
        nodes: 432_235,
        edges: 2_080_404,
        person_nodes: 1_700,
    },
    KnownDataset {
        scale: "0.3",
        source_url: "https://datasets.ldbcouncil.org/lsqb/social-network-sf0.3-projected-fk.tar.zst",
        archive_sha256: Some("4aad6e31047a356d40e8c315916c3fe35a77911024136d69868b39b16f8ccf33"),
        archive_bytes: Some(19_134_337),
        extracted_manifest_sha256: "aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3",
        csv_files: 36,
        csv_bytes: 160_662_563,
        nodes: 1_179_535,
        edges: 6_183_839,
        person_nodes: 3_900,
    },
];

/// Verifies the complete extracted CSV manifest before attaching the identity
/// of an official archive to a benchmark report.
pub fn lsqb_dataset_identity(
    scale: &str,
    stats: DatasetStats,
    fingerprint: &DatasetFingerprint,
) -> Result<DatasetIdentityV2, String> {
    let known = DATASETS
        .iter()
        .find(|dataset| dataset.scale == scale)
        .ok_or_else(|| {
            format!(
                "no pinned extracted-manifest identity for LSQB scale {scale:?}; supported scales: example, 0.1, 0.3"
            )
        })?;

    verify_field(
        "extracted manifest SHA-256",
        known.extracted_manifest_sha256,
        &fingerprint.sha256,
    )?;
    verify_field("CSV file count", known.csv_files, fingerprint.csv_files)?;
    verify_field("CSV byte count", known.csv_bytes, fingerprint.csv_bytes)?;
    verify_field("node count", known.nodes, stats.nodes)?;
    verify_field("edge count", known.edges, stats.edges)?;
    verify_field("Person node count", known.person_nodes, stats.person_nodes)?;

    Ok(DatasetIdentityV2 {
        scale_factor: scale.to_string(),
        model: "LSQB projected foreign-key CSV adapted to Grust labels".to_string(),
        source_url: known.source_url.to_string(),
        archive_sha256: known.archive_sha256.map(str::to_string),
        archive_bytes: known.archive_bytes,
        extracted_manifest_sha256: Some(fingerprint.sha256.clone()),
        csv_files: fingerprint.csv_files,
        csv_bytes: fingerprint.csv_bytes,
        nodes: stats.nodes,
        edges: stats.edges,
        person_nodes: stats.person_nodes,
    })
}

fn verify_field<T>(name: &str, expected: T, actual: T) -> Result<(), String>
where
    T: std::fmt::Display + PartialEq,
{
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "LSQB dataset {name} mismatch: expected {expected}, got {actual}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_fingerprint() -> DatasetFingerprint {
        DatasetFingerprint {
            sha256: DATASETS[0].extracted_manifest_sha256.to_string(),
            csv_files: DATASETS[0].csv_files,
            csv_bytes: DATASETS[0].csv_bytes,
        }
    }

    fn example_stats() -> DatasetStats {
        DatasetStats {
            nodes: DATASETS[0].nodes,
            edges: DATASETS[0].edges,
            person_nodes: DATASETS[0].person_nodes,
        }
    }

    #[test]
    fn verifies_before_attaching_archive_provenance() {
        let identity = lsqb_dataset_identity(
            "0.1",
            DatasetStats {
                nodes: DATASETS[1].nodes,
                edges: DATASETS[1].edges,
                person_nodes: DATASETS[1].person_nodes,
            },
            &DatasetFingerprint {
                sha256: DATASETS[1].extracted_manifest_sha256.to_string(),
                csv_files: DATASETS[1].csv_files,
                csv_bytes: DATASETS[1].csv_bytes,
            },
        )
        .unwrap();
        assert_eq!(
            identity.archive_sha256.as_deref(),
            DATASETS[1].archive_sha256
        );
        assert_eq!(identity.archive_bytes, DATASETS[1].archive_bytes);
    }

    #[test]
    fn rejects_a_directory_named_like_an_official_scale_with_different_bytes() {
        let mut fingerprint = example_fingerprint();
        fingerprint.sha256 = "0".repeat(64);
        let error = lsqb_dataset_identity("example", example_stats(), &fingerprint).unwrap_err();
        assert!(error.contains("manifest SHA-256 mismatch"));
    }

    #[test]
    fn rejects_unknown_scales_without_claiming_archive_provenance() {
        let error =
            lsqb_dataset_identity("1000", example_stats(), &example_fingerprint()).unwrap_err();
        assert!(error.contains("no pinned extracted-manifest identity"));
    }
}
