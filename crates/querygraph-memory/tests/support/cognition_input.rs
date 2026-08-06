use typesec_core::policy::{MintOptions, RequestContext, mint_capability_for_id};
use typesec_core::{CanRead, Capability, Resource};
use typesec_memory::{
    AuthorizedCognitionInput, GovernedSourceScope, InMemoryStore, Label, MemoryId, MemorySpace,
    MemoryStore, MemoryVault, StoredRecord,
};

const POLICY: &str = r#"
roles:
  - name: cognition-reader
    permissions: [read]
    resources: ["memory/user:alice/**"]
assignments:
  - subject: "agent:cognition-test"
    roles: [cognition-reader]
"#;

/// Build the same opaque manifest and plaintext views from one vault read.
pub fn authorized_input(records: Vec<StoredRecord>) -> AuthorizedCognitionInput {
    authorized_input_for(records, "research")
}

pub fn authorized_input_for(records: Vec<StoredRecord>, purpose: &str) -> AuthorizedCognitionInput {
    authorized_input_for_scope(records, purpose, None)
}

pub fn governed_authorized_input_for(
    records: Vec<StoredRecord>,
    purpose: &str,
    scope: &GovernedSourceScope,
) -> AuthorizedCognitionInput {
    authorized_input_for_scope(records, purpose, Some(scope))
}

fn authorized_input_for_scope(
    records: Vec<StoredRecord>,
    purpose: &str,
    scope: Option<&GovernedSourceScope>,
) -> AuthorizedCognitionInput {
    assert!(!records.is_empty(), "cognition fixture needs a source");
    let store = InMemoryStore::new();
    let ids: Vec<MemoryId> = records.iter().map(|record| record.id.clone()).collect();
    for record in records {
        store.put(record).expect("persist cognition fixture");
    }
    let vault = MemoryVault::new(store);
    let space = MemorySpace::new("user:alice", "semantic");
    let policy = typesec_rbac::RbacEngine::from_yaml(POLICY).expect("fixture policy");
    let capability: Capability<CanRead, MemorySpace> = mint_capability_for_id(
        &policy,
        "agent:cognition-test",
        space.resource_id(),
        &MintOptions::default(),
    )
    .expect("fixture read capability");
    let context = RequestContext::new().with_purpose(purpose);
    match scope {
        Some(scope) => vault.governed_cognition_input_at(
            &space,
            &capability,
            &ids,
            &context,
            Label::Secret,
            scope,
        ),
        None => vault.cognition_input_at(&space, &capability, &ids, &context, Label::Secret),
    }
    .expect("authorized cognition input")
}
