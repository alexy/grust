//! Stable engine identities available before authorized input is loaded.

use super::operation::CognitionOperation;

/// Exact algorithm identity selected by a trusted host before an engine
/// receives authorized data.
///
/// A host registry binds this value to an engine independently of the public
/// [`super::CognitionEngine`] implementation. It must never accept a profile
/// reported by the engine itself. Static native factories bind the selected
/// implementation family to an explicit algorithm specification rather than
/// executor output or an unrelated package version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CognitionEngineProfile {
    algorithm: &'static str,
    algorithm_version: &'static str,
}

impl CognitionEngineProfile {
    const fn new(algorithm: &'static str, algorithm_version: &'static str) -> Self {
        Self {
            algorithm,
            algorithm_version,
        }
    }

    /// Select the built-in deterministic reference profile.
    pub const fn reference(operation: CognitionOperation) -> Self {
        Self::new(
            operation.native_reference_algorithm(),
            operation.algorithm_spec_version(),
        )
    }

    /// Select the native Sail profile for the same canonical operation spec.
    pub const fn sail(operation: CognitionOperation) -> Self {
        Self::new(
            operation.native_sail_algorithm(),
            operation.algorithm_spec_version(),
        )
    }

    /// Exact native algorithm identity.
    pub const fn algorithm(self) -> &'static str {
        self.algorithm
    }

    /// Exact native algorithm version.
    pub const fn algorithm_version(self) -> &'static str {
        self.algorithm_version
    }

    /// Compare a signed identity without allocating or invoking the engine.
    pub fn matches(self, algorithm: &str, algorithm_version: &str) -> bool {
        self.algorithm == algorithm && self.algorithm_version == algorithm_version
    }
}
