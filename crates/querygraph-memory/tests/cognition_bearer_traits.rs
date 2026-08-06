use querygraph_memory::cognition::{CognitionLease, CognitionOutboxClaim};

static_assertions::assert_not_impl_any!(
    CognitionLease:
        std::fmt::Debug,
        Clone,
        serde::Serialize,
        serde::de::DeserializeOwned
);
static_assertions::assert_not_impl_any!(
    CognitionOutboxClaim:
        std::fmt::Debug,
        Clone,
        serde::Serialize,
        serde::de::DeserializeOwned
);

#[test]
fn bearer_types_have_no_ambient_logging_copy_or_serde_traits() {}
