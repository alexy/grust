# QueryGraph Memory Goal

Status: durable memory v1 is consumed by qg-rust. The generic governed
cognition substrate is implemented in Grust and under final local verification
as of 2026-08-05; standalone Marciana orchestration and the qg-rust cognition
cutover remain pending.

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

## Current completion contract

| Surface | Current state | Evidence |
| --- | --- | --- |
| TypeSec `MemoryStore` compatibility | Complete | Full versioned TypeSec conformance corpus, including graph reachability |
| Durable local persistence | Complete | File-backed Turso close/reopen integration test |
| Database initialization | Complete | `TursoMemoryStore` creates parent directories, connects, and calls Grust `bootstrap` before returning |
| Transactional consolidation | Complete on Turso | Turso reports `Transactional`; supersede-and-replace persists as one mutation batch |
| Sync/async bridge | Complete for v1 | Dedicated runtime has I/O/time drivers; nested-runtime calls and async-context drop are tested |
| Tenant authorization | Complete at the vault boundary | One shared store/two vaults test proves capabilities cannot cross spaces |
| Semantic ranking reference | Complete | Privacy-aware in-process `VectorIndex` implements `SemanticIndex` |
| Reference cognition plans | Complete | Deduplication, contradiction, and importance functions emit `ConsolidationPlan`s |
| Governed cognition planning substrate | Implemented in Grust | Host-selected reference or live Sail engines accept only TypeSec-authorized input bound by proposal schema v4 to its optional governed source scope, distinct LakeCat grant and snapshot identities, projection, TypeDID evidence, and a deterministic typed effect; both implement the explicit per-operation v2 semantic contract |
| Durable cognition application substrate | Implemented on guarded Turso | Leased digest-only jobs, exact source CAS, typed memory effect, exact ID-only index outbox, versioned audit and outcome evidence, and job completion share one transaction; no-change commits durable evidence without fabricating a mutation or outbox row |
| Cognition retry and recovery substrate | Implemented in Grust | Concurrent apply, commit-response-loss, retry, and reopen tests recover one cross-validated byte-stable outcome without a probe write or duplicate mutation |
| Bounded Spark execution | Implemented in Grust | Shared 16 MiB Arrow payload/17 MiB Spark message limit, row/work limits, finite operation/abort/cleanup deadlines, and preflight before Arrow allocation |
| QueryGraph runtime/API wiring | Memory v1 complete; cognition cutover pending | Signed-only remember/recall/forget routes, exact `did:key` RBAC, body-subject spoof test, and server reopen proof are present; native `improve` moves through standalone Marciana after extraction |

The sibling qg-rust application opens this store behind TypeSec's
`ToolCallGuard`, `MemoryToolRouter`, and `MemoryVault`. qg-python's Pydantic AI
v2 demo carries separate TypeDID credential and memory capabilities, writes as
one authorized DID, restarts qg-rust, recalls as another authorized DID, and
records the outsider denial receipt.

## Storage and security boundary

The store adapter persists `StoredRecord` as an opaque JSON value and returns
it whole. It does not inspect content to make authorization decisions. The
TypeSec vault remains the only component that rehydrates and releases content,
checks capabilities, applies clearance ceilings, enforces quarantine, joins
labels during consolidation, and records audit events. Cognition can process
only the transient authorized view the vault has already released, and it
returns an inert proposal to that vault. Grust preserves the optional canonical
source scope in proposal and audit evidence, while TypeSec alone selects scoped
records and rechecks the scope and full-record preconditions at application.

Live Sail receives content-derived normalized and contradiction keys. Those
keys are content-bearing rather than anonymized, so the endpoint must be
deployed inside the processing boundary authorized for the protected cognition
input.

Scheduler and outbox methods are storage primitives for Marciana's
authenticated scheduler and trusted worker pool. Submitter and worker
identities may differ so leases can be recovered, and canonical owner text or a
scoped job key is not authentication. Once a lease or outbox claim is issued,
its unpersisted token is the sole bearer credential for worker transitions.
Marciana must authenticate acquisition and cancellation and keep bearer tokens
confidential.

Durable job `transitionedAt` is the caller-supplied logical transition time. A
completed job binds it to TypeSec's audited `preparedAt`; it is never presented
as backend commit time. The authoritative `committedAt` exists only in the
commit outcome and product receipt, must be canonical RFC 3339, and must not
predate preparation. Malformed or regressive backend time fails closed during
both initial return and recovery rather than borrowing another phase's clock.
The terminal `completionDigest` is exactly TypeSec's canonical prepared digest,
not the resulting memory version; this keeps a no-change decision distinct from
the prior state it intentionally retained. Recovery checks wire schema versions
before decoding and rejects incompatible historical audit and outcome layouts.

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

## Remaining product integration

The Grust substrate is not the Marciana product boundary. Marciana must still
be extracted with history, own authenticated job acquisition and lease
renewal, compose LakeCat evidence with its ingestion profile, run post-engine
LakeCat and TypeSec reauthorization, issue the versioned commit-bound receipt,
and expose one opaque `improve` operation to
qg-rust. QueryGraph must then switch to that standalone implementation and pass
its route, reopen, retry, and recovery compatibility gates.

## Later scale work

These are useful scale improvements, not blockers for the durable v1 contract:

1. **LanceDB ANN:** add a persistent `SemanticIndex` implementation and an
   honest Grust vector-query surface. `grust-lancedb` currently implements
   graph storage but not `GraphMutationStore` or a reusable ANN API.
2. **GQL pushdown:** project query metadata and lower point-in-time, lineage,
   ordering, and limit predicates without changing `StoreQuery::matches`
   semantics. Space pushdown already prevents a scoped query from scanning
   other spaces.
3. **Lineage model:** preserve multiple assertions of the same relationship
   and make tombstoning one fact leave other assertions intact.
4. **Hosted service:** add operational tenancy, quotas, migrations, deletion
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
