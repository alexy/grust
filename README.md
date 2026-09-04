# Grust

Grust is a modern property graph API for Rust.

It gives Rust applications one small, backend-neutral way to build, validate,
traverse, and eventually persist graph data. The core model is intentionally
plain:

```text
Graph = nodes + edges
Node  = id + label + properties
Edge  = optional id + from + to + label + properties
```

That shape is expressive enough for persistent graph databases such as
SurrealDB and HelixDB, but small enough to use in tests, import/export tools,
scrapers, knowledge-graph pipelines, and local in-memory workflows.

Grust is early, but the direction is deliberate: keep graph construction and
domain modeling independent from database query languages. Application code
should build a `grust::Graph`; backend crates should decide how to write or
query that graph.

## Why Grust?

Rust has excellent in-memory graph libraries, especially `petgraph`, but many
applications need a property graph abstraction that maps naturally to graph
databases:

- stable application IDs
- node labels and edge labels
- typed node and edge properties
- backend-neutral graph construction
- optional schema metadata
- traversal expressed as an IR rather than a database query string
- an async store trait for persistence backends

Grust focuses on that persistent property-graph layer. It is not trying to
replace `petgraph` for graph algorithms. A Grust memory backend can use simple
maps today and could use `petgraph` internally later where that helps.

## Current Workspace

```text
crates/
  grust/          Public facade package (`grust-graph`) and prelude
  grust-cocoindex/ CocoIndex-style graph target-state export adapter
  grust-core/     Core model, builder, schema, traversal IR, GraphStore trait
  grust-cypher/   Portable GQL/Cypher parser, planner, and reference executor
  grust-falkor/   FalkorDB writer using Redis GRAPH.QUERY
  grust-helix/    HelixDB writer using HTTP or the Rust SDK
  grust-ladybug/  Embedded LadybugDB store using the Rust lbug crate
  grust-lancedb/  LanceDB store using the Rust SDK
  grust-memory/   Deterministic in-memory store for tests and local use
  grust-postgres/ Generic PostgreSQL store over universal graph tables
  grust-postgres-core/ Shared PostgreSQL table and SQL lowering implementation
  grust-postgres-pgq/ PostgreSQL 19 SQL/PGQ wrapper over the PostgreSQL store
  grust-pggraph/  pgGraph extension wrapper over the PostgreSQL store
  grust-sail/     Sail SparkConnect backend using Spark DataFrames
  grust-sql-core/ Shared SQL generation helpers for SQL table backends
  grust-surreal/  SurrealDB writer using HTTP or the Rust SDK
  grust-turso/    Turso store using the Rust SDK over SQLite-compatible tables
  querygraph-memory/ Private TypeSec-governed memory integration
```

The backend crates expose reads and traversal as they mature behind the same
`GraphStore` APIs instead of leaking backend query languages into application
code.

Shared backend-lowering helpers such as `relationship_type`,
`schema_identifier`, and `edge_key` live in `grust-core` so database adapters do
not drift on relationship names, typed table identifiers, or structural edge
keys.

Adapters that persist or export the compatibility `edge_key` use
`checked_edge_key`. It rejects U+001F in an edge's source ID, relationship
label, target ID, or explicit ID before delimiter-based structural identities
can alias. `GraphValue` has a separate in-memory deduplication format: its
relationship identity components are length-framed, so arbitrary payload text
does not require a reserved delimiter. Ladybug's internal metadata index also
uses U+001F framing, so that adapter rejects the delimiter in node IDs before
the node can alias a metadata entry.

`validate_physical_identifier_claims` gives schema-lowering backends one
namespace-aware collision check. FalkorDB, Helix, LadybugDB, LanceDB, and Sail
use it to reject both different logical names that lower to the same physical
object and exact duplicate declarations before any schema operation is sent.

`GraphIndex` also lives in `grust-core`. It is the shared dense adjacency layer
for local analytics, backend planning, and adapter crates that need validated
edge endpoints without rebuilding their own node-id maps.

`grust-cocoindex` is intentionally different: it exports Grust graphs as
CocoIndex-style node and relationship target state so an incremental indexing
flow can propagate changes into a downstream graph or table backend.

## Backend Integration Tests

Fast unit tests stay self-contained:

```sh
cargo test --workspace --all-features
```

Backend integration tests are explicit and fail if their service is missing.
Run them through the launcher. For a first contributor run, use the Docker
profile:

```sh
scripts/integration-test.sh doctor --profile docker --mode docker
scripts/integration-test.sh --profile docker --mode docker
```

The Docker profile starts the Docker-backed services in
`docker-compose.integration.yml` and runs the local LanceDB and CocoIndex
integration checks. The full maintainer matrix is:

```sh
scripts/integration-test.sh --profile all
```

The launcher reads `integration/backends.conf`. In `auto` mode it prefers
already-running services, then configured local source checkouts such as
`/Users/alexy/src/sail`, `/Users/alexy/src/SurrealDB`,
`/Users/alexy/src/FalkorDB`, and `/Users/alexy/src/HelixDB`, then Docker
Compose where a service is available.

Run a single backend with:

```sh
scripts/integration-test.sh --backend sail
scripts/integration-test.sh --backend surreal
scripts/integration-test.sh --backend falkor
scripts/integration-test.sh --backend helix
scripts/integration-test.sh --backend ladybug
scripts/integration-test.sh --backend lancedb
scripts/integration-test.sh --backend cocoindex
scripts/integration-test.sh --backend pggraph
scripts/integration-test.sh --backend postgres-pgq
```

Use `--no-start` to require an already-running service, and `--keep-running` to
leave services up for debugging. See [docs/INTEGRATION.md](docs/INTEGRATION.md)
for profiles, modes, Docker image pins, source-checkout configuration, and the
CI strategy.

The 0.13 qualification pass advances the Redis client to 1.6.0 and the
FalkorDB service to v4.20.4, SurrealDB to 3.2.4 with reqwest 0.13.4, the
SurrealDB service to v3.2.4, pgGraph's service to 1.2.0, tokio-postgres to
0.7.18, and Turso from a prerelease to stable 0.7.2. LanceDB remains at 0.30.0
because the attempted 0.38.0 local-mode build references its
remote-feature-only `Error::Http` variant when `remote` is disabled. The
internal Helix adapter remains on exact `helix-db` 2.0.0 because 3.0.0 removed
the dynamic-query APIs it uses and targets `/v2/query`, while the checked
Helix v3.0.1 server still exposes `/v1/query`. See the
[backend qualification record](benchmarks/lsqb/BACKENDS.md) for the complete
matrix and live-gate evidence.

## Core Benchmarks

The facade crate includes a dependency-free benchmark harness for core graph
operations and `GraphIndex` construction:

```sh
cargo run --release -p grust-graph --example benchmarks
```

It uses the same synthetic graph families as GrustFrames, including
Graph500/GAP-style deterministic R-MAT cases. The harness measures graph
cloning, shared index construction, degree scans, endpoint scans, and structural
edge-key generation.

The separate [LSQB compatibility harness](benchmarks/lsqb/README.md) provides
a Docker-reproducible backend check. Its unchanged upstream baseline pins Graph
Data Council LSQB commit `242cb2fd31340ca688954cb94794d74c0d5b6f92`,
LadybugDB 0.19.0, and a digest-pinned Python 3.12.11 image. Five repetitions of
the 28-node/72-edge `sfexample` fixture match all nine expected query counts,
for 45/45 successful count checks. The clearly separated Grust adapter is
configured to reload that fixture for each repetition on Memory, Turso, and
PostgreSQL 18.6. The adversari.al extension adds eight
exact-count attacks per backend and nine backend-neutral policy-rejection
attacks, for 17 attacks in two explicitly separate tracks. Each storage cell
therefore has 17 count oracles—the nine LSQB-derived queries plus eight
adversarial count queries—while policy is reported once as its own track. Run
the two sides with:

```sh
benchmarks/lsqb/run-upstream.sh
benchmarks/lsqb/run-grust.sh
```

This is a small conformance and reproducibility workload, not a performance
ranking. LSQB is maintained by the Graph Data Council but is not an official
LDBC benchmark.

These are not LDBC Benchmark Results.

The durable benchmark hub is [adversari.al/graph](https://adversari.al/graph).
Tracked evidence currently covers the unchanged upstream run; the Grust matrix
must still be captured before its results are described as completed.

## Core Model

The core types live in `grust-core` and are re-exported by `grust`.

```rust
use grust::prelude::*;

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

pub struct Node {
    pub id: NodeId,
    pub label: Label,
    pub props: Props,
}

pub struct Edge {
    pub id: Option<EdgeId>,
    pub from: NodeId,
    pub to: NodeId,
    pub label: Label,
    pub props: Props,
}
```

Properties are a map of string keys to typed values:

```rust
pub type Props = std::collections::BTreeMap<String, Value>;

pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    DateTime(RfcDate),
    Decimal(Decimal),
    Duration(Duration),
    StringArray(Vec<String>),
    IntArray(Vec<i64>),
    FloatArray(Vec<f64>),
    Path(PathValue),
    Graph(GraphValue),
    Json(serde_json::Value),
}
```

Edge properties are first-class. This matters because modern graph databases
usually store data on relationships as well as on nodes.

## Quick Start

Use the prelude for the common graph-building API:

```rust
use grust::prelude::*;

let mut graph = GraphBuilder::new();

let talk = graph
    .node("Talk", "talk:rust-graph-api")
    .prop("title", "A Modern Graph API for Rust")
    .prop("abstract", "Building backend-neutral property graphs in Rust.")
    .finish();

let speaker = graph
    .node("Person", "person:ada")
    .prop("name", "Ada Example")
    .prop("organization", "Graph Systems Lab")
    .finish();

graph
    .edge("PRESENTED_BY", &talk, &speaker)
    .prop("source", "conference-schedule")
    .finish();

let graph = graph.build();
```

The builder deduplicates nodes by `NodeId` and, by default, deduplicates edges
by `(from, label, to)`. If your domain needs multi-edges, use
`EdgePolicy::AllowDuplicates`. Backends that support explicit edge IDs, such as
`MemoryGraphStore`, can preserve id-bearing parallel edges between the same
endpoints.

```rust
let mut graph = GraphBuilder::new().edge_policy(EdgePolicy::AllowDuplicates);
```

## In-Memory Store

Enable the `memory` feature to use `MemoryGraphStore` from the public facade:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.13.0", features = ["memory"] }
```

The facade re-exports the full `grust-memory` crate surface when the feature is
enabled, matching the other backend feature exports.

Then load and traverse a graph:

```rust
use grust::prelude::*;

# async fn example() -> grust::Result<()> {
let mut builder = GraphBuilder::new();
let talk = builder.node("Talk", "talk:rust-graph-api").finish();
let speaker = builder.node("Person", "person:ada").finish();
builder.edge("PRESENTED_BY", &talk, &speaker).finish();
let graph = builder.build();

let store = MemoryGraphStore::new();
store.put_graph(&graph).await?;

let speakers = store
    .traverse(
        Traversal::from_node("talk:rust-graph-api")
            .out("PRESENTED_BY")
            .to("Person"),
    )
    .await?;

assert_eq!(speakers.len(), 1);
# Ok(())
# }
```

## GraphStore

Backends implement `GraphStore` (capability and native-constraint methods are
omitted here for brevity):

```rust
#[async_trait::async_trait]
pub trait GraphStore: Send + Sync {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()>;

    async fn put_node(&self, node: &Node) -> Result<PutOutcome>;
    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome>;
    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport>;
    async fn put_typed_graph(&self, schema: &GraphSchema, graph: &Graph) -> Result<LoadReport>;

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>>;
    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>>;
    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>>;
    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>>;
}
```

`put_graph` borrows the graph instead of consuming it. That makes retries,
validation, comparison, and multi-backend loads easier.
Single-element writes return `PutOutcome`. Memory and builder paths can report
precise inserted/updated/deduped outcomes, while remote upsert-oriented
backends commonly return `Upserted` because they cannot distinguish insert from
update without an extra read. Portable callers should treat all written
outcomes as success rather than depending on inserted-versus-updated.
`put_typed_graph` validates a graph against `GraphSchema`, applies that schema
to the backend, and then writes the graph. `apply_schema` itself is a backend
metadata hook, not a portable promise that every future write is enforced by the
database.
With the optional `typed-garde` feature, `TypedNode::from_node` and
`TypedEdge::from_edge` decode stored graph values back into validated Rust
domain types.

Administrative backends can also implement `GraphAdminStore` for setup and
replacement workflows:

```rust
#[async_trait::async_trait]
pub trait GraphAdminStore: GraphStore {
    async fn bootstrap(&self) -> Result<()> {
        Ok(())
    }

    async fn clear(&self) -> Result<()>;
}
```

## Bounded Reference Reads

With the `cypher` feature, applications that expose a deliberately small read
surface can validate and execute it through `ReadQueryPolicy`:

```rust
use grust::prelude::*;

let policy = ReadQueryPolicy {
    max_result_rows: 25,
    max_candidate_work: 10_000,
    max_intermediate_bytes: 64 * 1024 * 1024,
    max_output_bytes: 256 * 1024,
    ..ReadQueryPolicy::default()
};

let table = run_bounded_read_query(
    &projected_graph,
    "MATCH (n:Person) RETURN n.id LIMIT 25",
    &CypherParameters::new(),
    &policy,
)?;
```

The parser-backed gate rejects updating clauses and unsafe query shapes, then
the in-memory reference executor enforces query, parameter, graph, candidate
work, cumulative-intermediate-byte, result-row, output-byte, range-allocation,
cumulative path-hop, and
cooperative wall-clock limits. Scalar and table-valued ranges also retain the
library-wide `MAX_RANGE_ITEMS` ceiling. Correlated `CALL { ... }` subqueries
also charge each node/adjacency index build, and catalog procedures charge each
graph scan, so repeated per-outer-row work cannot bypass the candidate-work or
cooperative deadline budget.

This is bounded reference execution, not authorization or an operating-system
hard cancellation boundary. Callers still own graph projection, tenancy,
deadlines around remote work, and process-level resource isolation. Backend
pushdown uses each store's separately documented capabilities.

## Backend Stores

Backend crates are optional facade features:

```toml
[dependencies.grust]
package = "grust-graph"
version = "0.13.0"
features = [
  "cocoindex", "cypher", "falkor", "lancedb", "memory", "postgres",
  "postgres-pgq", "pggraph", "sail", "surreal", "turso",
]
```

The internal `grust-helix` and `grust-ladybug` workspace crates are
`publish = false` and deliberately are not facade features. Workspace users can
exercise them directly; crates.io consumers should not request `helix`,
`ladybug`, or `ladybug-arrow` from `grust-graph`.
The additional `turso-sync` feature enables Turso Cloud synchronization and
implies `turso`; `typed-garde` and `typed-zod-rs` enable typed ingestion rather
than storage backends.

For Arrow-native data sources, enable `sail` to stage Arrow IPC streams as
Spark temp views. The internal Ladybug adapter also has an Arrow IPC surface
for workspace testing. See
[docs/Arrow.md](https://github.com/querygraph/grust/blob/main/docs/Arrow.md)
for the full contract and the Arrow-version compatibility rationale.

`grust-falkor` writes nodes and edges through Redis/FalkorDB Cypher queries and
supports graph replacement with `GRAPH.DELETE`. Configurable identity-property
names and generated label, relationship, and property identifiers are checked
before Cypher construction; property names are losslessly quoted where
FalkorDB permits them, normalized-name collisions in schemas and complete graph
loads fail closed, and pool/query errors do not include the configured Redis
URL or credentials.

`grust-helix` provides both `HelixHttpGraphStore` and `HelixSdkGraphStore`.
Both batch node and edge writes, preserve supported scalar and array properties,
and use configured labels for replacement. Both paths reject unsafe schema
names, normalized relationship-name collisions, and attempts to overwrite
structural node or edge metadata before sending a write. Transport errors omit
the configured URL and any embedded credentials or query secrets.

`grust-ladybug` embeds LadybugDB directly through the Rust `lbug` 0.20.2 crate.
It creates Grust-managed Ladybug node and relationship tables from graph labels,
persists label/table metadata for readback, writes graph loads in transactions,
and exposes backend-neutral reads and bounded traversal without starting a
daemon.
The default `LadybugGraphMode::Untyped` accepts ordinary Grust graphs and
creates the needed Ladybug tables from labels on write. `LadybugGraphMode::Typed`
requires `apply_schema` or `put_typed_graph` before writes and validates later
writes against the applied `GraphSchema`. Node IDs containing U+001F are
rejected because Ladybug's managed metadata index reserves that delimiter.
With the internal crate's `arrow` feature, the backend can also register Arrow
IPC node, relationship, and CSR relationship tables directly with Ladybug and
return query results as Arrow IPC chunks for workspace experiments.

`grust-cocoindex` converts `Graph` values into serializable node and
relationship states with stable keys, endpoint labels, and plain JSON
properties, and can load that target-state JSON back into Grust graphs. It is a
sync/import-export adapter rather than a `GraphStore`.

`grust-lancedb` stores graphs in LanceDB tables using the official Rust SDK,
upserts nodes and edges with `merge_insert`, supports backend-neutral reads and
bounded traversal over universal node/edge tables, batches target-node reads
during traversal, performs exact property-start matching over decoded Grust
properties, and can mirror schema-labeled nodes and edges into typed Arrow
tables.

`grust-postgres` stores Grust graphs in universal PostgreSQL tables using
ordinary JSONB and SQL, so it can run on managed PostgreSQL services such as
Neon without requiring extensions. It supports SQL-backed reads/traversal,
schema-derived typed label views and expression indexes, and transactional
mutation batches through the shared `grust-postgres-core` implementation.

`grust-pggraph` wraps the same shared PostgreSQL implementation, registers the
universal tables with the pgGraph extension, and can build a pgGraph projection
for graph-index experiments.

`grust-postgres-pgq` targets PostgreSQL 19's native SQL/PGQ support. It keeps
the same universal PostgreSQL tables as the durable source of truth, creates a
native `PROPERTY GRAPH` over those tables, and executes bounded traversal with
`GRAPH_TABLE`.

`grust-turso` uses the Turso Rust SDK directly and stores Grust graphs in
SQLite-compatible universal node and edge tables with JSON text properties. It
supports local in-process Turso databases by default and exposes an optional
`turso-sync` facade feature for Turso Cloud sync construction. Reads,
schema-derived views, bounded traversal, and mutation batches run through
ordinary SQL over the local Turso connection; synced callers can explicitly
push or pull through the store's Turso sync helpers.

The PostgreSQL, PostgreSQL PGQ, pgGraph, and Turso backends share
backend-neutral SQL graph generation through `grust-sql-core`: universal table
DDL, reads, traversal joins, mutation framing, schema views, indexes,
identifier quoting, and literal escaping. The dialect layer stays narrow and
performance-sensitive: PostgreSQL keeps JSONB operators, `ON CONFLICT`,
`CREATE OR REPLACE VIEW`, and lateral joins, while Turso keeps JSON text,
`json_extract`, `json_patch`, and SQLite-compatible view and join forms.
The dialect contract also exposes an optional generated-identifier byte limit
through `GraphSqlDialect::max_identifier_bytes`. PostgreSQL sets its real
63-byte ceiling, so schema-derived typed views and property indexes fail with
`GrustError::Schema` before server-side truncation can create an ambiguous or
colliding name; other dialects keep no limit unless they declare one.
`grust-postgres-core` remains the PostgreSQL-specific execution and connection
layer reused by `grust-postgres`, `grust-postgres-pgq`, and `grust-pggraph`.
Recursive SQL walk plans encode arbitrary node IDs as hexadecimal tokens before
building their visited sets; dialects without a delimiter-free encoding hook
do not claim that pushdown. PostgreSQL's public raw `execute` method is
autocommit-only and lexically rejects transaction-control batches. Explicit
transaction helpers mark the connection before `BEGIN`, serialize every user
of it, and make the next caller roll back work left uncertain if a future is
cancelled during `BEGIN`, a statement, or `COMMIT`; the PGQ wrapper inherits
the same guard and recovery path.
Sail is intentionally outside this shared SQL core because its lowering targets
Spark Connect, Arrow IPC staging, and distributed Spark SQL rather than direct
row-store SQL.

`grust-sail` stores graphs as Spark DataFrames through Sail's SparkConnect
server, lowers traversal IR to Spark SQL joins, and can mirror schema-labeled
rows into typed Delta tables. SQL filters bind user values through Spark
Connect named arguments; delete mutations stage their values as Arrow temp views
before running argument-free SQL commands.
`SailConfig::default()` leaves `spark.sql.warehouse.dir` under server control,
so a remote client neither injects a client-local path nor changes the
server's persistence identity. `SailWarehouse::LocalSessionScoped` explicitly
chooses a client temporary directory derived from the session ID for a
co-located development server; Grust does not delete it, so callers own
cleanup. `SailWarehouse::ExplicitPath` sets and reads back a stable absolute
path that the server can resolve. Reopening managed tables still depends on
Sail providing a persistent catalog and warehouse; Grust does not infer that
server-side lifecycle from a client path. Sail versions whose unconfigured
warehouse fallback is the relative `spark-warehouse` path must be given an
absolute server setting or one of the explicit Grust overrides before managed
Delta tables are created.
Typed Delta tables retain their declared column names and enforce structural
identity with Delta constraints.
It also exposes Sail's Arrow IPC path directly for staging arbitrary Arrow
streams as session temp views, collecting Spark SQL results as IPC chunks, and
loading Grust-shaped node/edge IPC streams through the graph write path.
`drop_arrow_ipc_view` removes a staged view idempotently, including on worker
failure paths that handled protected input.
For graph analytics over the persisted generic Sail tables, it provides
`read_graph`, `in_degrees`, `out_degrees`, `degrees`, and `degree_pairs`
helpers backed by Spark SQL.
The crate also exposes the generic Sail table and column contract as public
constants plus field-projection helpers, so distributed planners can target the
same `grust_nodes`, `grust_edges`, typed-node, and typed-edge layout that the
backend writes.
Typed-table descriptor helpers and directional triplet SQL helpers cover the
common GrustFrames-style needs of selecting schema-backed Sail tables and
lowering triplet filters, motifs, and aggregate-message passes.
Writable Cypher lives in `grust-cypher`, which parses accepted text into
backend-neutral mutation plans. The supported surface covers explicit and
matched node/relationship `CREATE`, `MERGE`, `DELETE`, `SET`, and `REMOVE`
forms, including bounded row-producing relationship writes. Identity
generation, strict-create behavior, parameters, null assignment, and collection
of accepted write identities are explicit `CypherMutationOptions` choices.

Restricted write-with-`RETURN` operations can project supported element,
property, scalar, aggregate, and path shapes into a `CypherResultTable`.
Arbitrary read-query clauses and unbounded row materialization remain outside
that helper; use the read executor for reads and consult
[the profile statement](docs/GQL_PROFILE_STATEMENT.md) for exact language scope.
The restricted scalar evaluator covers literals, maps/lists, `CASE`,
`coalesce`, introspection, list access and conversion, string helpers, numeric
helpers, type conversion, element functions, and path functions. Aggregates
reuse those scalar families where their row/group semantics permit it. This
surface is intentionally classified through a small internal AST rather than
opening write projection to arbitrary expression evaluation.
`CypherMutationOptions::parameters` lets callers bind Grust `Value`s to
`$name` placeholders in literal positions such as IDs, property maps, and
literal property assignments; quoted `'$name'` remains ordinary string text.
Mutating `MATCH` clauses accept a bounded `WHERE` grammar covering property
comparisons, null and string predicates, scalar membership, and restricted
boolean groups. The planner canonicalizes representable same-property
combinations, removes duplicate or subsumed branches, and collapses
contradictions to a no-match predicate. Shapes that cannot lower without
semantic loss are rejected; missing properties retain Cypher null semantics.
`CypherMutationOptions::null_assignment` defaults to storing
`SET x.key = null` as `Value::Null`, but callers can select
`CypherNullAssignment::RemoveProperty` to lower explicit null assignment to
the same property-removal operations used by `REMOVE`. Map patches such as
`SET x += {key: null}` always store `Value::Null`.
`MATCH ... SET` clauses can contain comma-separated assignments. Each
assignment is lowered as its own ordered plan operation, so repeated property
targets preserve source order while still using only the supported literal,
map patch, remove-on-null, and numeric node update forms.
The first expression form is intentionally small: node property assignments can
read the current value of another property on the same node variable and apply
`+`, `-`, `*`, or `/` with an integer or float literal or parameter, lowering
to an explicit read-modify-write mutation plan shared by Sail and Memory.
Parsing, planning, DDL helpers, and the restricted returning evaluator now live
in `grust-cypher`. Execution of resolved mutation plans is backend-neutral
through `CypherMutationExecutor`, so the same `GraphMutationPlan` can execute on
Sail or on `MemoryGraphStore` for deterministic tests. Backends without support
for a plan operation return structured execution errors instead of ignoring it.
`grust-sail` owns only Sail-specific execution concerns such as SparkConnect,
SQL lowering, Arrow IPC staging, Delta `MERGE INTO`, and registry-table
persistence, while preserving the `sail_cypher_*` names as compatibility
wrappers.
Mutation batch atomicity is explicit through `GraphMutationAtomicity`: the
default mutation path is ordered but not atomic, while backends with proven
transaction wrappers—PostgreSQL, PostgreSQL SQL/PGQ, pgGraph, SurrealDB, and
Turso—report `Transactional` for one `apply_mutations` batch. Higher-level
executors must establish their own whole-statement transaction boundary before
claiming statement atomicity. The PostgreSQL and Turso Cypher executors resolve
their supported non-returning plan first, then execute its operations in source
order inside one isolated transaction. The generic write-with-`RETURN` helper
intentionally preserves intermediate bindings through sequential execution and
is not a whole-statement atomicity boundary; use the explicit
transaction-script API when an atomic supported batch is required.
Writable Cypher also lowers ID-resolved and broad node
`MATCH ... SET n += { ... }` map patches into backend-neutral node patch
mutations; `null` is stored as a graph value rather than interpreted as
property removal.
ID-resolved edge `MATCH ... SET e += { ... }` lowers to backend-neutral edge
patch mutations and reuses the same typed-edge mirror writes as ordinary edge
upserts.
Row-producing edge `MATCH ... CREATE` and `MATCH ... MERGE` materialize the
matched endpoint node pairs before writing, report the matched row count
separately from attempted edge upserts, and reject trailing node creation.
Explicit relationship IDs are accepted only for single-row row-producing
writes, while generated relationship IDs require an explicit caller-selected
policy. Precise insert/update counters are populated where the executor can
distinguish newly inserted rows from rows that already existed.
Literal `SET n.key = value` / `SET e.key = value` lowers to one-key patches,
and explicit `REMOVE n.key` / `REMOVE e.key` lowers to backend-neutral property
remove mutations. Node forms can target either a resolved identity or a broad
node match; edge forms can target either a resolved identity or a broad
relationship match. Broad relationship matches can filter on relationship
property predicates beyond `id`; explicit edge `id` remains a separate
identity filter and can be combined with ordinary relationship predicates.
Same-relationship numeric property updates such as `SET e.weight = e.weight +
1` lower to explicit read-modify-write mutations; cross-variable relationship
expressions and general computed expressions remain deferred.
Existing matched relationship `SET`, `REMOVE`, relationship-only `DELETE`, and
mixed endpoint-deleting `DELETE` rows can bind restricted path variables, so
`MATCH p = (a)-[e:TYPE]->(b) SET e.seen = true RETURN p` and
`MATCH p = (a)-[e:TYPE]->(b) DELETE e RETURN p` return the same JSON path
shape used by row-producing and resolved single-edge write paths. For mixed
forms such as `DELETE e, a`, including explicit-ID endpoints, the returned
path is snapshotted before the relationship and endpoint node are removed.
The mutation report includes matched-row and changed node/edge counts for
broad Sail node and relationship deletes, patches, and property removals, and
the parser accepts top-level mutation keywords case-insensitively while
stripping Cypher comments outside string literals.
Cypher planning and execution failures use structured `GrustError` variants
for syntax, unresolved identity, unsupported cardinality, and execution errors;
concrete executors advertise their own backend-neutral plan support rather than
silently accepting unsupported operations.

`grust-surreal` provides both `SurrealHttpGraphStore` and
`SurrealSdkGraphStore`. It bootstraps namespaces/databases, maps labels and
relationships to Surreal tables, upserts nodes, and relates edges through
relation tables. Reads and traversal batch target-node lookups where possible.
SurrealQL identifiers are quoted without lossy property-name rewriting;
configuration, schema, normalized table claims, and complete graph batches are
validated before I/O. Reserved node/edge storage fields cannot be overwritten,
and optional Grust edge IDs persist separately as `edge_id`. HTTP/WebSocket
errors omit URL userinfo and query material.
Generic edge reads need `SurrealConfig.relationships`; if that list is empty,
the backend returns a configuration error instead of silently scanning no
relation tables. Explicit edge-label reads can still address a known relation
table directly. Node deletes also need configured relationship labels so
incident relation rows can be removed. HTTP and SDK mutation batches are
wrapped in SurrealDB transactions.
`GraphSchema` lowers to Surreal `DEFINE TABLE` and `DEFINE FIELD` statements.

## Traversal IR

Grust does not expose SurrealQL, HQL, Cypher, or SQL in the common layer. It
uses a small traversal IR:

```rust
let traversal = Traversal::from_node("talk:rust-graph-api")
    .out("PRESENTED_BY")
    .to("Person")
    .limit(10);
```

Backends are responsible for lowering that IR into their native query language
or SDK calls.

Conceptually:

```text
Grust:    talk -[PRESENTED_BY]-> Person
Surreal:  talk:id->presented_by->person
Helix:    N<Talk>(id)::Out<PresentedBy>
pgGraph:  SQL over grust_nodes/grust_edges, optionally graph.build()
Turso:    SQL over grust_nodes/grust_edges with SQLite JSON functions
Sail:     Spark SQL joins over grust_nodes/grust_edges
LanceDB:  SDK table filters over grust_nodes/grust_edges
Memory:   adjacency-map lookup
```

## Schema Layer

The schema model is optional. It exists for backends that benefit from
declarations, type generation, indexes, or validation:

```rust
pub struct GraphSchema {
    pub nodes: Vec<NodeType>,
    pub edges: Vec<EdgeType>,
    pub constraints: Vec<GraphConstraint>,
}

pub struct NodeType {
    pub label: Label,
    pub fields: Vec<Field>,
}

pub struct EdgeType {
    pub label: Label,
    pub from: Vec<Label>,
    pub to: Vec<Label>,
    pub fields: Vec<Field>,
    pub directed: bool,
    pub uniqueness: EdgeUniqueness,
}
```

`GraphSchema::builder()` and `Field::required` / `Field::optional` provide a
compact way to declare this structure:

Date-time values are stored as validated `RfcDate` values inside
`Value::DateTime`; use `Value::datetime` or `RfcDate::parse` instead of raw
strings when constructing typed date-time values.

```rust
let schema = GraphSchema::builder()
    .node(
        "Person",
        vec![
            Field::required("name", FieldType::String),
            Field::optional("age", FieldType::Int),
        ],
    )
    .edge(
        "WORKS_ON",
        vec![Label::new("Person")],
        vec![Label::new("Project")],
        vec![Field::required("role", FieldType::String)],
    )
    .required_node_property("Person", "email")
    .unique_node_property("Person", "email")
    .build();
```

Constraint metadata is portable but enforcement is explicit. Required-property
constraints validate through `GraphSchema` before writes on backends that keep
an applied schema. Unique-property constraints validate inside
`GraphSchema::validate_graph`; the memory backend reports validate-before-write
behavior for them. Memory also supports explicit native constraint application
through `GraphStore::apply_native_constraint`, storing backend-owned required
or unique property constraints and enforcing them on later writes without
requiring typed `GraphSchema` metadata. `grust-cypher` exposes
`apply_cypher_native_constraints` for applying parsed `CREATE CONSTRAINT` DDL
through that native-constraint path. Other backends may still report
metadata-only behavior until they add comparable preflight or native
enforcement.

Named DDL metadata remains portable. A `CypherConstraintRegistry` can
materialize a `CypherCatalogSnapshot` for a graph name, and
`cypher_catalog_procedure` returns deterministic catalog rows for `db.graphs`,
`db.graphTypes`, `db.indexes`, and `db.constraints`.
Read queries may select a named graph with `USE <graph>`; the default
single-graph read path accepts `USE default`, while
`run_read_query_on_named_graph` binds a graph snapshot to an explicit name.
Standalone session commands use `CypherSession` / `SessionCommand` for `USE`,
`SET`, and `RESET`; fixed-length path bindings now return `Value::Path`.

The current backends use schema differently:

- SurrealDB can run schemaless, but schema can define record tables, relation
  tables, and typed fields.
- HelixDB validates schema names through the dynamic-query backend while future
  schema-file generation remains backend-specific.
- LadybugDB can run in untyped dynamic mode or typed schema-applied mode; typed
  mode validates writes against the applied `GraphSchema`.
- pgGraph keeps universal tables while exposing typed label views and indexes.
- Sail keeps universal DataFrames while mirroring rows into typed Delta tables.
- LanceDB keeps universal tables while mirroring rows into typed Arrow tables.
- FalkorDB uses schema declarations to create label/property indexes.
- Memory uses schema for validation tests and local conformance.

### Semantic Model Graphs

The facade also provides `semantic_model_graph` for turning a versioned
`SemanticModelProjection` into ordinary Grust nodes and edges. A projection
contains datasets, fields, metrics, named dataset relationships, a positive
model version, and SHA-256 identities for the source artifact and metric
expressions.

The conversion is deterministic and validates names, hashes, references, and
per-scope uniqueness before building anything. Length-prefixed identity
components prevent delimiter collisions, and semantic relationships carry
explicit edge IDs, so two differently named relationships between the same
dataset pair remain distinct in the constructed `Graph`. The result is just a
`Graph`: callers can inspect, query, or persist it through the same
backend-neutral APIs as application data. As with every id-bearing multi-edge,
preserving both relationships after persistence requires a backend that
supports explicit edge IDs; structurally keyed stores collapse edges sharing
the same endpoints and label.

The release proof parses the packaged Apache Ossie TPC-DS YAML from pinned
upstream commit `ddb19f1b135a61c65603f4823a3526e2fab00cf1`, verifies its
SHA-256 before parsing, and checks deterministic projection of its five
datasets, 31 fields, five metrics, and four relationships. The published
`grust-graph` archive carries the upstream `NOTICE` and Apache-2.0 text beside
that fixture, and `scripts/verify-package-attribution.sh` checks the archive
contents during release packaging.

## Backend Mapping

### SurrealDB

SurrealDB maps naturally to Grust's model:

```text
Node label      -> table
Node id         -> record id or stored property
Edge label      -> relation table
Edge properties -> relation record fields
Traversal       -> arrow traversal
```

Example conceptual write:

```text
RELATE talk:rust_graph_api->presented_by->person:ada CONTENT {
  source: "conference-schedule"
}
```

### HelixDB

HelixDB is schema and query oriented:

```text
Node label      -> node type
Edge label      -> edge type
Node properties -> node fields/properties
Edge properties -> edge Properties block
Traversal       -> typed Out/In traversal
```

The Helix backend should hide generated or named queries behind `GraphStore`
so application code remains backend-neutral.

### pgGraph

pgGraph keeps PostgreSQL as the source of truth and builds a derived graph
projection for bounded traversal. The Grust backend starts with universal
tables:

```text
grust_nodes(id, label, props)
grust_edges(id, from_id, to_id, label, props)
```

`PgGraphStore` implements ordinary reads and Grust traversal with SQL over
those tables. `GraphAdminStore::bootstrap()` creates the tables, installs the
`graph` extension, and registers the universal edge table with pgGraph using
the edge `label` column as the dynamic relationship type.

### Sail / SparkConnect

Sail maps Grust's model to two Delta Lake tables and lowers the traversal IR
to multi-JOIN Spark SQL:

```text
Node id / label / props  -> row in grust_nodes
Edge endpoints / type    -> row in grust_edges (with src_label, dst_label)
put_node / put_edge      -> MERGE INTO (Delta upsert)
get_node                 -> SELECT … WHERE id = ? LIMIT 1
traverse                 -> multi-JOIN Spark SQL, one JOIN pair per step
```

Example traversal SQL for `.out("PRESENTED_BY").to("Talk")`:

```text
SELECT n1.id, n1.label, n1.props
FROM   grust_nodes  n0
JOIN   grust_edges  e0  ON  e0.src_id = n0.id
                        AND e0.edge_type = 'PRESENTED_BY'
JOIN   grust_nodes  n1  ON  n1.id = e0.dst_id
                        AND n1.label = 'Talk'
WHERE  n0.id = 'person:ada'
```

`GraphAdminStore::bootstrap()` creates the tables with `USING delta`.
`clear()` issues `DELETE FROM` on both tables.

### LanceDB

LanceDB maps Grust's graph model to two Lance tables using Arrow batches and
the Rust SDK:

```text
Node id / label / props  -> row in grust_nodes
Edge key / endpoints     -> row in grust_edges
put_node / put_edge      -> merge_insert upsert
get_node / get_edges     -> SDK query filters
traverse                 -> edge filters plus batched target-node reads per step
```

`LanceDbGraphStore::connect()` opens a local or remote LanceDB URI,
`GraphAdminStore::bootstrap()` creates empty universal tables when needed, and
property-start traversal compares decoded Grust properties exactly after the
label-filtered read instead of matching serialized JSON fragments.
`clear()` drops and recreates them. Node IDs are the node upsert key. Edges use
an explicit edge ID when present and otherwise use `(from, label, to)` as a
stable key. Properties are stored as JSON text for backend-neutral reads today;
typed property columns and vector indexes can be layered on through schema and
backend-specific extension traits later.

## Design Principles

- Keep graph data independent from database query languages.
- Make IDs explicit and stable.
- Treat edge properties as first-class data.
- Prefer typed values over ad hoc JSON strings.
- Keep schema optional.
- Keep traversal backend-neutral.
- Keep backend-specific capabilities as extension traits when they appear.
- Make the in-memory backend deterministic and boring, especially for tests.

## Status

Grust 0.13.0 "Prawn" is the current release line. The core model and reference
execution paths are stable enough for real use, while backend-native feature
parity remains intentionally explicit rather than implied by the common trait.

Implemented:

- core property graph model
- typed IDs and labels
- typed property values
- graph builder
- schema structs
- traversal structs and fluent helpers
- async `GraphStore` trait
- ordered `GraphMutationStore` trait, with transactional batch overrides where
  the backend can provide them
- parser-backed, resource-bounded in-memory GQL/Cypher reads
- a versioned semantic-model-to-property-graph projection
- the scoped `Full39075` Grust language profile and reference executor
- CocoIndex-style graph export adapter
- in-memory backend
- published FalkorDB, LanceDB, PostgreSQL, PostgreSQL SQL/PGQ, pgGraph, Sail,
  SurrealDB, and Turso adapters
- internal HelixDB and LadybugDB workspace adapters

Active follow-up areas:

- deeper native read/write parity across persistent backends
- streaming or paginated result surfaces for graphs larger than in-memory
  materialization
- persistent vector search for the LanceDB integration
- production hosting, quotas, and the remaining Marciana cognition cutover

See [the GQL profile statement](docs/GQL_PROFILE_STATEMENT.md) for language
scope and [the integration guide](docs/INTEGRATION.md) for backend-specific
live verification.

## Development

Run the full test suite:

```sh
cargo test --workspace --all-features
```

Format the workspace:

```sh
cargo fmt --all -- --check
```

Run checks for all crates:

```sh
cargo check --workspace --all-features --all-targets
```

Release packaging and publication have additional mandatory gates in
[PUBLISH.md](PUBLISH.md).

## License

Grust is dual-licensed under either of:

- Apache License, Version 2.0
- MIT license

Choose either license when using, modifying, or distributing Grust.
