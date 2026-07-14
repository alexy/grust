# QueryGraph Memory Goal

Status: durable v1 implemented, consumed by qg-rust, and locally verified on
2026-07-14.

This document is the Grust-side source of truth for `querygraph-memory`. It
separates the TypeSec storage contract and delivered application wiring from
later scale optimizations that require new backend surfaces or services.

## Objective

Provide QueryGraph applications with a capability-safe, restart-durable
implementation of `typesec_memory::MemoryStore`, without moving authorization
or content access out of TypeSec's `MemoryVault`.

The v1 production path is local Turso/libSQL:

```rust,ignore
use querygraph_memory::TursoMemoryStore;

let store = TursoMemoryStore::open("data/querygraph-memory.db")?;
```

`open` uses the stable table prefix `querygraph_memory`. Applications sharing
one database with other Grust graphs can use
`TursoMemoryStore::open_with_config(TursoConfig)` to choose a table prefix,
batch size, or journal mode explicitly.

## V1 completion contract

| Surface | V1 state | Evidence |
| --- | --- | --- |
| TypeSec `MemoryStore` compatibility | Complete | Full versioned TypeSec conformance corpus, including graph reachability |
| Durable local persistence | Complete | File-backed Turso close/reopen integration test |
| Database initialization | Complete | `TursoMemoryStore` creates parent directories, connects, and calls Grust `bootstrap` before returning |
| Transactional consolidation | Complete on Turso | Turso reports `Transactional`; supersede-and-replace persists as one mutation batch |
| Sync/async bridge | Complete for v1 | Dedicated runtime has I/O/time drivers; nested-runtime calls and async-context drop are tested |
| Tenant authorization | Complete at the vault boundary | One shared store/two vaults test proves capabilities cannot cross spaces |
| Semantic ranking reference | Complete | Privacy-aware in-process `VectorIndex` implements `SemanticIndex` |
| Reference cognition plans | Complete | Deduplication, contradiction, and importance functions emit `ConsolidationPlan`s |
| QueryGraph runtime/API wiring | Complete in qg-rust | Signed-only remember/recall/forget routes, exact `did:key` RBAC, body-subject spoof test, and server reopen proof |

The sibling qg-rust application opens this store behind TypeSec's
`ToolCallGuard`, `MemoryToolRouter`, and `MemoryVault`. qg-python's Pydantic AI
v2 demo carries separate TypeDID credential and memory capabilities, writes as
one authorized DID, restarts qg-rust, recalls as another authorized DID, and
records the outsider denial receipt.

## Storage and security boundary

`querygraph-memory` persists `StoredRecord` as an opaque JSON value and returns
it whole. It does not reveal record content. The TypeSec vault remains the only
component that rehydrates content, checks capabilities, applies clearance
ceilings, enforces quarantine, joins labels during consolidation, and records
audit events.

Space equality is pushed to Grust with `Start::NodesByProperty`; the remaining
`StoreQuery` fields are evaluated by TypeSec's shared `StoreQuery::matches`.
This is semantically complete but intentionally not described as full query
pushdown.

## V1 limitations

- `GraphStoreMemoryStore::new` is a generic adapter for a backend the caller
  has already initialized. Only the Turso constructor is bootstrapped and
  proven persistent by this crate's integration suite.
- Atomic consolidation is a backend capability. Turso provides it; a generic
  `GraphMutationStore` that reports `OrderedNonAtomic` does not.
- Tenant isolation is authorization at the TypeSec vault boundary, not
  physical per-tenant graph partitioning. Global entity nodes can cause a
  neighborhood traversal to discover record IDs from several spaces; the
  vault rejects records outside the authorized space before revealing them.
- Grust's universal edge identity is `(from, label, to)`. Repeated facts with
  the same endpoints and label cannot yet retain distinct relationship
  lineage merely by changing `fact_id`. Full lineage needs assertion nodes or
  a future multi-edge identity surface.
- `TursoConfig::default()` uses `:memory:`. Durable applications should use
  `TursoMemoryStore::open(path)` or pass a file path to `open_with_config`.

## Post-v1 work

These are useful scale improvements, not blockers for the durable v1 contract:

1. **LanceDB ANN:** add a persistent `SemanticIndex` implementation and an
   honest Grust vector-query surface. `grust-lancedb` currently implements
   graph storage but not `GraphMutationStore` or a reusable ANN API.
2. **Sail cognition:** replace reference analytics bodies with authorized
   Arrow/Spark batch jobs over `grust-sail`. Sail currently requires a live
   Spark Connect service and does not advertise transactional mutation batches.
3. **GQL pushdown:** project query metadata and lower point-in-time, lineage,
   ordering, and limit predicates without changing `StoreQuery::matches`
   semantics. Space pushdown already prevents a scoped query from scanning
   other spaces.
4. **Lineage model:** preserve multiple assertions of the same relationship
   and make tombstoning one fact leave other assertions intact.
5. **Hosted service:** add operational tenancy, quotas, migrations, deletion
   workflows, and service-level integration tests in the QueryGraph
   application. A vault-isolation test is not itself a hosted product.

## Verification gate

Run from the Grust repository root with the sibling TypeSec checkout present:

```sh
cargo fmt --check -p querygraph-memory
cargo clippy -p querygraph-memory --all-features --all-targets --no-deps -- -D warnings
cargo test -p querygraph-memory --all-features
cargo test -p grust-turso --test transaction_atomic --test turso_read_query
git diff --check
```

The focused Clippy gate uses `--no-deps`: Turso brings in publishable Grust
SQL/Cypher crates whose workspace-wide lint baseline is maintained separately.
The test gate still compiles and executes those dependencies.

The targeted GitHub workflow must also be green. It watches
`querygraph-memory`, `grust-turso`, `grust-sql-core`, `grust-cypher`,
`grust-core`, `grust-memory`, and the workspace manifests/lockfile.
