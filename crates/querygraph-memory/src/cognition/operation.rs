//! Canonical cognition operation identities and strict parsing.

/// Semantic contract version for canonical deduplication.
///
/// This is deliberately independent of the crate/package version. It changes
/// only when inputs can produce a different canonical plan.
pub const DEDUPLICATE_ALGORITHM_SPEC_VERSION: &str = "2";

/// Semantic contract version for canonical contradiction reconciliation.
///
/// This is deliberately independent of the crate/package version. It changes
/// only when inputs can produce a different canonical plan.
pub const RECONCILE_ALGORITHM_SPEC_VERSION: &str = "2";

/// Native cognition operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitionOperation {
    /// Supersede exact duplicates.
    Deduplicate,
    /// Detect contradictions and propose invalidating obsolete assertions.
    Reconcile,
}

impl CognitionOperation {
    /// Canonical operation identity bound into TypeDID and proposal evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deduplicate => "marciana.deduplicate",
            Self::Reconcile => "marciana.reconcile",
        }
    }

    /// Exact semantic contract version bound into TypeDID and proposals.
    ///
    /// Reference and Sail implementations of one operation share this version
    /// because both must produce the same canonical plan. Package and build
    /// versions are implementation metadata, never mutation authority.
    pub const fn algorithm_spec_version(self) -> &'static str {
        match self {
            Self::Deduplicate => DEDUPLICATE_ALGORITHM_SPEC_VERSION,
            Self::Reconcile => RECONCILE_ALGORITHM_SPEC_VERSION,
        }
    }

    pub(super) const fn native_reference_algorithm(self) -> &'static str {
        match self {
            Self::Deduplicate => "marciana.deduplicate.reference",
            Self::Reconcile => "marciana.reconcile.reference",
        }
    }

    pub(super) const fn native_sail_algorithm(self) -> &'static str {
        match self {
            Self::Deduplicate => "marciana.deduplicate.sail",
            Self::Reconcile => "marciana.reconcile.sail",
        }
    }

    /// Check an exact QueryGraph-native executor identity and version.
    ///
    /// Reference and Sail implementations must claim the operation's exact
    /// semantic contract version. The trusted host binds the implementation
    /// family separately; a package version cannot grant mutation authority.
    pub fn is_native_algorithm(self, algorithm: &str, version: &str) -> bool {
        version == self.algorithm_spec_version()
            && (algorithm == self.native_reference_algorithm()
                || algorithm == self.native_sail_algorithm())
    }
}

impl std::fmt::Display for CognitionOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for CognitionOperation {
    type Err = ParseCognitionOperationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl TryFrom<&str> for CognitionOperation {
    type Error = ParseCognitionOperationError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "marciana.deduplicate" => Ok(Self::Deduplicate),
            "marciana.reconcile" => Ok(Self::Reconcile),
            _ => Err(ParseCognitionOperationError),
        }
    }
}

/// Strict parsing failure for a canonical cognition operation identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unsupported cognition operation")]
pub struct ParseCognitionOperationError;
