//! Multi-tenant isolation: one shared Grust store, many vaults, typesec
//! policies as the tenancy boundary. The productizable "memory-as-a-service
//! with provable isolation" claim, as a test.
//!
//! Two tenants share a single backend. Each tenant's subject is scoped to its
//! own `memory/user:<tenant>/**` by the policy; the vault's capability check
//! means tenant A's subject can neither read nor write tenant B's spaces —
//! even though the records live side by side in one graph.

use std::sync::Arc;

use grust_memory::MemoryGraphStore;
use querygraph_memory::GraphStoreMemoryStore;
use typesec_core::policy::{MintOptions, RequestContext, mint_capability_for_id};
use typesec_core::secure_value::Internal;
use typesec_core::{CanRead, CanWrite, Capability, Resource};
use typesec_memory::{
    MemoryContent, MemoryDraft, MemoryError, MemoryKind, MemorySpace, MemoryVault, Provenance,
    RecallQuery,
};

/// One shared policy: each tenant subject may only touch its own namespace.
const POLICY: &str = r#"
roles:
  - name: tenant-a
    permissions: [read, write]
    resources: ["memory/user:alice/**"]
  - name: tenant-b
    permissions: [read, write]
    resources: ["memory/user:bob/**"]
assignments:
  - subject: "agent:a"
    roles: [tenant-a]
  - subject: "agent:b"
    roles: [tenant-b]
"#;

fn cap<P: typesec_core::Permission>(
    engine: &typesec_rbac::RbacEngine,
    subject: &str,
    space: &MemorySpace,
) -> Result<Capability<P, MemorySpace>, typesec_core::policy::CapabilityError> {
    mint_capability_for_id(
        engine,
        subject,
        space.resource_id(),
        &MintOptions::default(),
    )
}

#[test]
fn one_backend_many_vaults_tenants_cannot_cross() {
    let engine = typesec_rbac::RbacEngine::from_yaml(POLICY).expect("policy parses");
    // A single shared Grust store behind two vaults (as a hosted service would
    // share one cluster across tenants).
    let shared = Arc::new(GraphStoreMemoryStore::new(MemoryGraphStore::default()));

    // Each tenant gets a vault over the *same* store.
    let vault_a = MemoryVault::new(SharedStore(shared.clone()));
    let vault_b = MemoryVault::new(SharedStore(shared.clone()));

    let alice = MemorySpace::new("user:alice", "profile");
    let bob = MemorySpace::new("user:bob", "profile");

    let a_write: Capability<CanWrite, _> = cap(&engine, "agent:a", &alice).unwrap();
    let b_write: Capability<CanWrite, _> = cap(&engine, "agent:b", &bob).unwrap();

    vault_a
        .remember(
            &alice,
            &a_write,
            MemoryDraft::new(
                MemoryKind::Profile,
                MemoryContent::text("A's secret"),
                Provenance::Operator,
            ),
        )
        .expect("A writes A's space");
    vault_b
        .remember(
            &bob,
            &b_write,
            MemoryDraft::new(
                MemoryKind::Profile,
                MemoryContent::text("B's secret"),
                Provenance::Operator,
            ),
        )
        .expect("B writes B's space");

    // Tenant A cannot mint a capability for B's space at all.
    assert!(
        cap::<CanRead>(&engine, "agent:a", &bob).is_err(),
        "policy denies A a capability over B's space"
    );

    // Even holding A's own read cap, calling it against B's space is a
    // SpaceMismatch — the capability is bound to the space it was minted for.
    let a_read_own: Capability<CanRead, _> = cap(&engine, "agent:a", &alice).unwrap();
    let cross = vault_a.recall::<Internal>(
        &bob,
        &a_read_own,
        RecallQuery::all(),
        &RequestContext::default(),
    );
    assert!(
        matches!(cross, Err(MemoryError::SpaceMismatch { .. })),
        "A's capability cannot be pointed at B's space"
    );

    // Each tenant sees exactly its own record — despite one shared backend.
    let a_view = vault_a
        .recall::<Internal>(
            &alice,
            &a_read_own,
            RecallQuery::all(),
            &RequestContext::default(),
        )
        .expect("A reads A");
    assert_eq!(a_view.hits.len(), 1);
    assert_eq!(a_view.hits[0].content.text, "A's secret");

    let b_read: Capability<CanRead, _> = cap(&engine, "agent:b", &bob).unwrap();
    let b_view = vault_b
        .recall::<Internal>(
            &bob,
            &b_read,
            RecallQuery::all(),
            &RequestContext::default(),
        )
        .expect("B reads B");
    assert_eq!(b_view.hits.len(), 1);
    assert_eq!(b_view.hits[0].content.text, "B's secret");
}

/// A thin `MemoryStore` newtype that shares one backing store by `Arc`, so
/// multiple vaults can front the same graph (a hosted service pattern).
struct SharedStore(Arc<GraphStoreMemoryStore<MemoryGraphStore>>);

impl typesec_memory::MemoryStore for SharedStore {
    fn put(&self, record: typesec_memory::StoredRecord) -> Result<(), typesec_memory::StoreError> {
        self.0.put(record)
    }
    fn get(
        &self,
        id: &typesec_memory::MemoryId,
    ) -> Result<Option<typesec_memory::StoredRecord>, typesec_memory::StoreError> {
        self.0.get(id)
    }
    fn query(
        &self,
        q: &typesec_memory::StoreQuery,
    ) -> Result<Vec<typesec_memory::StoredRecord>, typesec_memory::StoreError> {
        self.0.query(q)
    }
    fn invalidate(
        &self,
        id: &typesec_memory::MemoryId,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), typesec_memory::StoreError> {
        self.0.invalidate(id, at)
    }
    fn tombstone(&self, id: &typesec_memory::MemoryId) -> Result<bool, typesec_memory::StoreError> {
        self.0.tombstone(id)
    }
    fn apply_batch(
        &self,
        ops: Vec<typesec_memory::StoreBatchOp>,
    ) -> Result<(), typesec_memory::StoreError> {
        self.0.apply_batch(ops)
    }
    fn link(
        &self,
        from: &str,
        rel: &str,
        to: &str,
        record: &typesec_memory::MemoryId,
    ) -> Result<(), typesec_memory::StoreError> {
        self.0.link(from, rel, to, record)
    }
    fn neighborhood(
        &self,
        entity: &str,
        hops: u8,
    ) -> Result<Vec<typesec_memory::MemoryId>, typesec_memory::StoreError> {
        self.0.neighborhood(entity, hops)
    }
}
