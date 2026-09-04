//! Governed LakeCat source proof and explicit ingestion field mapping.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::CognitionError;
use super::bounds::{is_canonical_projection, is_canonical_text};

/// Hash-bound proof of the LakeCat snapshot and governed Sail scan used by a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GovernedLakeCatSnapshot {
    /// LakeCat catalog URI or id.
    pub catalog: String,
    /// Iceberg namespace.
    pub namespace: String,
    /// Iceberg table.
    pub table: String,
    /// Immutable Iceberg snapshot id.
    pub snapshot_id: i64,
    /// LakeCat-issued digest of the complete governed scan proof.
    pub governed_scan_digest: String,
    /// Immutable table/snapshot version digest revalidated at application.
    pub snapshot_digest: String,
    /// Digest of the opaque LakeCat/Sail plan-task token.
    pub plan_task_digest: String,
    /// Cryptographically verified TypeDID subject.
    pub subject: String,
    /// Purpose bound into authorization and scan planning.
    pub purpose: String,
    /// Projection LakeCat allowed after policy narrowing.
    pub effective_projection: Vec<String>,
    /// Digest of LakeCat's original authorization grant receipt.
    pub authorization_receipt_digest: String,
}

impl GovernedLakeCatSnapshot {
    /// Validate and return a stable proof digest.
    pub fn digest(&self) -> Result<String, CognitionError> {
        if self.snapshot_id < 0 {
            return Err(CognitionError::InvalidSnapshot("snapshot id"));
        }
        for (name, value) in [
            ("catalog", self.catalog.as_str()),
            ("namespace", self.namespace.as_str()),
            ("table", self.table.as_str()),
            ("subject", self.subject.as_str()),
            ("purpose", self.purpose.as_str()),
        ] {
            if !is_canonical_text(value) {
                return Err(CognitionError::InvalidSnapshot(name));
            }
        }
        if !is_canonical_projection(&self.effective_projection) {
            return Err(CognitionError::InvalidSnapshot("effective projection"));
        }
        for (name, digest) in [
            ("governed scan digest", self.governed_scan_digest.as_str()),
            ("snapshot digest", self.snapshot_digest.as_str()),
            ("plan task digest", self.plan_task_digest.as_str()),
            (
                "authorization receipt digest",
                self.authorization_receipt_digest.as_str(),
            ),
        ] {
            if !super::graph::is_sha256(digest) {
                return Err(CognitionError::InvalidSnapshot(name));
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|_| CognitionError::Serialization("snapshot encoding failed"))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

/// LakeCat source columns used by governed ingestion to derive TypeSec memory.
///
/// This mapping is authorization evidence, not the live Sail staging schema.
/// Sail receives bounded fields derived from the already-authorized memory
/// views and never reads raw LakeCat rows through this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitionFieldMapping {
    /// Stable source identifier column.
    pub id: String,
    /// Source text column used by the selected cognition operation.
    pub text: String,
    /// Source validity-time column used for deterministic precedence.
    pub valid_from: String,
}

impl CognitionFieldMapping {
    pub(super) fn validate(&self, projection: &[String]) -> Result<(), CognitionError> {
        let fields = [
            self.id.as_str(),
            self.text.as_str(),
            self.valid_from.as_str(),
        ];
        if !is_canonical_projection(projection) {
            return Err(CognitionError::InvalidSnapshot("effective projection"));
        }
        if fields.iter().any(|field| !is_canonical_text(field)) {
            return Err(CognitionError::InvalidSnapshot("cognition field mapping"));
        }
        if self.id == self.text || self.id == self.valid_from || self.text == self.valid_from {
            return Err(CognitionError::InvalidSnapshot(
                "duplicate cognition field mapping",
            ));
        }
        for field in fields {
            if !projection.iter().any(|allowed| allowed == field) {
                return Err(CognitionError::ProjectionDenied);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
