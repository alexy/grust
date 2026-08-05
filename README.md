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
```

The backend crates expose reads and traversal as they mature behind the same
`GraphStore` APIs instead of leaking backend query languages into application
code.

Shared backend-lowering helpers such as `relationship_type`,
`schema_identifier`, and `edge_key` live in `grust-core` so database adapters do
not drift on relationship names, typed table identifiers, or structural edge
keys.

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
    StringArray(Vec<String>),
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
grust = { package = "grust-graph", version = "0.10.0", features = ["memory"] }
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

Backends implement `GraphStore`:

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

## Backend Stores

Backend crates are optional facade features:

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.10.0", features = ["falkor", "helix", "ladybug", "lancedb", "postgres", "pggraph", "sail", "surreal", "turso"] }
```

For Arrow-native data sources, enable `ladybug-arrow` to use embedded
LadybugDB over Arrow IPC tables, or enable `sail` to stage Arrow IPC streams as
Spark temp views. See
[docs/Arrow.md](https://github.com/querygraph/grust/blob/main/docs/Arrow.md)
for the full contract and the Arrow-version compatibility rationale.

`grust-falkor` writes nodes and edges through Redis/FalkorDB Cypher queries and
supports graph replacement with `GRAPH.DELETE`.

`grust-helix` provides both `HelixHttpGraphStore` and `HelixSdkGraphStore`.
Both batch node and edge writes, preserve supported scalar and array properties,
and use configured labels for replacement.

`grust-ladybug` embeds LadybugDB directly through the Rust `lbug` crate. It
creates Grust-managed Ladybug node and relationship tables from graph labels,
persists label/table metadata for readback, writes graph loads in transactions,
and exposes backend-neutral reads and bounded traversal without starting a
daemon.
The default `LadybugGraphMode::Untyped` accepts ordinary Grust graphs and
creates the needed Ladybug tables from labels on write. `LadybugGraphMode::Typed`
requires `apply_schema` or `put_typed_graph` before writes and validates later
writes against the applied `GraphSchema`.
With the `ladybug-arrow` facade feature, the backend can also register Arrow
IPC node, relationship, and CSR relationship tables directly with Ladybug and
return query results as Arrow IPC chunks.

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
`grust-postgres-core` remains the PostgreSQL-specific execution and connection
layer reused by `grust-postgres`, `grust-postgres-pgq`, and `grust-pggraph`.
Sail is intentionally outside this shared SQL core because its lowering targets
Spark Connect, Arrow IPC staging, and distributed Spark SQL rather than direct
row-store SQL.

`grust-sail` stores graphs as Spark DataFrames through Sail's SparkConnect
server, lowers traversal IR to Spark SQL joins, and can mirror schema-labeled
rows into typed Delta tables. SQL filters bind user values through Spark
Connect named arguments; delete mutations stage their values as Arrow temp views
before running argument-free SQL commands.
Connecting also sets and reads back `spark.sql.warehouse.dir` in the same Spark
Connect session. `SailConfig::default()` chooses a fresh absolute temporary
warehouse for a co-located development server. Remote or durable deployments
must set `warehouse_dir` to a stable absolute path visible at the same location
to the Sail server; a client-local path is not portable across that boundary.
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
Writable Cypher is implemented in `grust-cypher` as a strict, backend-neutral
planning surface with Sail compatibility wrappers:
`sail_cypher_mutation_plan` and the generic `cypher_mutation_plan` parse
explicit-ID node `CREATE`/`MERGE`, resolved endpoint edge `CREATE`/`MERGE`, and
resolved node/edge `DELETE` into Grust mutation plans. They also accept ordered
multi-statement batches and local node variables bound from explicit-ID node
patterns, plus ID-resolved
`MATCH ... DELETE`, edge `MATCH ... CREATE` / `MATCH ... MERGE`, and
cardinality-aware broad node
`MATCH ... DELETE` / `MATCH ... SET n += { ... }` / `MATCH ... SET n.key = value`
/ `MATCH ... REMOVE n.key` forms plus ID-resolved edge
`MATCH ... SET e += { ... }`, literal property assignment, and explicit
property `REMOVE` for resolved node or edge identities, row-producing edge
`MATCH ... CREATE` and `MATCH ... MERGE` when both endpoints come from matched
node variables, plus broad relationship `MATCH ... DELETE` / `MATCH ... SET e += { ... }` /
`MATCH ... SET e.key = value` / `MATCH ... REMOVE e.key` over endpoint
predicates plus relationship-row deletes such as `DELETE e, a` that can delete
matched relationship rows and endpoint nodes from one captured row set, while
`SailGraphStore::execute_cypher_mutation` executes those plans through
`GraphMutationStore` and the existing Sail staging and `MERGE INTO` paths.
For stricter Cypher compatibility, callers can use
`execute_cypher_mutation_with_options` with
`CypherCreateMode::ErrorIfExists` to make `CREATE` perform a read-before-write
existence check instead of following the default upsert-compatible path; it
also rejects duplicate concrete node or edge `CREATE` identities inside the
same planned batch before any writes run.
Generated node IDs are also opt-in: `CypherNodeIdPolicy::GenerateForCreate`
allows node `CREATE` without an `id`, and
`execute_cypher_mutation_result_with_options` returns the generated IDs in
`CypherMutationResult::generated_node_ids` while leaving
`CypherMutationReport` count-oriented. Callers can also set
`CypherMutationOptions::collect_written_node_identities` and
`CypherMutationOptions::collect_written_edge_identities` to collect accepted
node and edge write identities in
`CypherMutationResult::written_node_identities` and
`CypherMutationResult::written_edge_identities`; edge payloads cover both
resolved and row-producing edge writes. These identities describe accepted
writes, not exact insert-versus-update outcomes on upsert backends.
`MERGE` and edge endpoint patterns still require resolved IDs before writing.
For the first table-returning write slice,
`execute_cypher_mutation_returning_with_options` accepts a final `RETURN`
containing element or property projections over node variables and concrete
relationship variables already resolved by the write plan, including concrete
edge upserts and edge patches. Sail and the backend-neutral Memory/Sail helper
can also return rows for relationship variables produced by restricted
row-producing `MATCH ... CREATE/MERGE` edge writes, plus portable broad node
rows for restricted `MATCH ... SET/REMOVE` forms such as
`MATCH (n:Person {status: 'active'}) SET n.seen = true RETURN n.id, n.seen`
and portable broad relationship rows for restricted `MATCH ... SET/REMOVE`
forms such as `MATCH (a)-[e:KNOWS]->(b) SET e.seen = true RETURN e.id, e.seen`.
The physical `id` and `label` fields are supported alongside stored
properties, and whole elements are returned as `Value::Json` in the Grust
`Node` / `Edge` serde shape. Examples include
`RETURN n.id, n.label, n.seen`, `RETURN e.id, e.label, e.weight`,
`RETURN n AS node, e AS relationship`, and `RETURN e.label, e.source` after a
row-producing edge write. It returns a
`CypherMutationTableResult` with the usual mutation result plus a
`CypherResultTable`; aggregation, paths, ordering, limiting, arbitrary
read-query features, unrestricted broad row materialization, and path-style
row projections remain rejected. Scalar projections and aggregate bodies share
one restricted return-target parser for the supported literal, map/list,
path-helper, introspection, string, numeric, conversion, `coalesce`, and `CASE`
forms. Restricted aggregate bodies also share the scalar projection
materializer for literal, map/list, introspection, string, numeric, conversion,
`coalesce`, `CASE`, and list-helper targets, while `*`, whole elements,
properties, and path functions keep their aggregate-specific paths. Restricted
`COUNT(...)` scalar targets use the same projection-materializer
classification before applying non-null and `DISTINCT` count semantics.
Grouped aggregate row materialization reuses that same scalar evaluator for
classifier-covered targets while keeping aggregate-specific shapes explicit.
Internally, writable `RETURN` target materialization is classified into star,
whole-element, direct-property, scalar-projection, element-function, and
path-function paths before aggregate and count routing. Scalar projection
evaluation also classifies restricted target shapes into literal, map, list,
conditional, coalesce, introspection, list-access, list-predicate, numeric,
conversion, string, element-function, and path-function kinds before routing
special cases. The scalar evaluator now routes through a small internal scalar
AST over those restricted shapes rather than matching the public return-target
enum directly, and list-helper expressions have a dedicated
internal evaluator boundary. String-helper expressions now use the same kind
of dedicated internal evaluator boundary, as do numeric and conversion helper
expressions. Literal/composite, `CASE`/`coalesce`, and introspection scalar
expressions use dedicated evaluator boundaries too, and binding plus
element/path wrapper scalar routes now sit behind the same expression-family
dispatcher style. The top-level scalar dispatcher also uses an internal
scalar AST-family classifier before routing to binding, wrapper, value,
control, introspection, list, numeric, conversion, or string evaluators.
Nested `coalesce(...)` arguments can reuse those already-supported restricted
scalar targets while keeping coalesce arguments on one variable and rejecting
nested list/map composites. Restricted `CASE` branch values use the same
scalar AST path for same-variable properties, literals, and already-supported
scalar helper targets while keeping CASE predicates equality-only. Restricted
list predicate equality values use that scalar AST path while keeping haystacks
property-only and item predicates equality-only. Restricted list projection
items use the same scalar AST path for direct properties, literals, and
already-supported scalar helper targets while still rejecting nested list/map
composites and cross-variable lists. Restricted map projection values use that
scalar AST path for same-variable properties, literals, and already-supported
scalar helper targets while still rejecting nested list/map composites and
cross-variable values. `toLower(...)` and `toUpper(...)` now use the same
bounded scalar argument path, so they can wrap direct properties, literals, or
already-supported restricted scalar targets without enabling general expression
evaluation. `trim(...)`, `lTrim(...)`, and `rTrim(...)` use that same bounded
scalar argument path while preserving existing string-only trim semantics.
`reverse(...)` uses the same bounded scalar argument path while preserving
existing string-or-array reverse semantics. `isEmpty(...)` also uses the
bounded scalar argument path while preserving existing string, array, and JSON
collection emptiness semantics. `split(...)` uses the same bounded first
argument path while keeping delimiters literal-or-parameter only and preserving
string-only split semantics. `substring(...)` uses the same bounded first
argument path while keeping offsets literal-or-parameter only and preserving
string-only substring semantics. `left(...)` and `right(...)` use the same
bounded first argument path while keeping lengths literal-or-parameter only and
preserving string-only slice semantics. `startsWith(...)`, `endsWith(...)`,
and `contains(...)` use that same bounded first argument path while keeping
needles literal-or-parameter only and preserving string-only predicate
semantics. `replace(...)` uses the same bounded first argument path while
keeping search and replacement strings literal-or-parameter only and preserving
string-only replacement semantics. `toString(...)` also uses the bounded
argument path while preserving scalar-only string conversion semantics.
`abs(...)` uses that same bounded argument path while preserving numeric-only
absolute-value semantics. `ceil(...)` and `floor(...)` use that same bounded
argument path while preserving numeric-only rounding semantics. `sign(...)`
uses that same bounded argument path while preserving finite numeric sign
semantics. `toInteger(...)` and `toFloat(...)` use that same bounded argument
path while preserving numeric and numeric-string conversion semantics.
`toBoolean(...)` uses that same bounded argument path while preserving boolean
and boolean-string conversion semantics. `head(...)`, `last(...)`, and
`tail(...)` use that same bounded argument path while preserving array-only
list access semantics. List index expressions and slice bounds use that same
bounded argument path while preserving non-negative-integer subscript
semantics. `toStringList(...)`, `toIntegerList(...)`, `toFloatList(...)`, and
`toBooleanList(...)` use that same bounded argument path while preserving
array-only list conversion semantics.
`CypherMutationOptions::parameters` lets callers bind Grust `Value`s to
`$name` placeholders in literal positions such as IDs, property maps, and
literal property assignments; quoted `'$name'` remains ordinary string text.
Mutating `MATCH` clauses can also use a small `WHERE` predicate grammar:
property comparisons against literals or parameters joined with `AND`, such as
`WHERE n.status = 'inactive' AND n.score >= $min`. Predicates lower to
backend-neutral `GraphPropertyPredicate` values, so Memory evaluates the same
plan that Sail lowers to SQL. Missing properties never match; `null` only
matches equality or inequality against `Value::Null`, and ordered comparisons
are limited to numbers or strings. The bounded grammar also supports null
checks, string predicates, scalar membership, same-property `OR` folds, and
restricted `AND` / `OR` groups parsed through an internal boolean AST. The AST
lowerer accepts factored `OR` branches whose `AND` groups share common
predicates and differ by one foldable same-property predicate, including
unparenthesized factored groups that lower to the same backend-neutral shape.
Common terms inside those factored branches may themselves be foldable `OR`
groups, and nested parenthesized foldable `OR` terms flatten into the same
grouped predicates when every leaf stays bounded. Exact duplicate bounded
predicates are de-duplicated after parsing and folding, including inside each
factored `OR` branch before branch comparison. Factored branch `AND` groups
also run the same bounded-predicate canonicalization pipeline as top-level
`AND`, so branch-local equality, membership, inequality, and range
combinations can expose a flat foldable shape. Impossible factored `OR`
branches are pruned after canonicalization, and all-impossible factored groups
collapse to the existing empty `IN` no-match predicate. Branches subsumed by a
broader sibling branch are pruned too, so `(A AND B) OR A` lowers to `A`.
Conservative same-property predicate implication also prunes narrower branches
covered by broader sibling membership, exclusion, inequality, or ordered-bound
predicates, including stricter same-direction range bounds and ordered bounds
that exclude a sibling inequality value or every value in a sibling grouped
exclusion. Simple bounded `OR` terms that cannot use the direct same-property
fold can also collapse through that same conservative branch-subsumption path,
for example when a broader `IS NOT NULL` sibling covers a narrower string
predicate. Negated simple `OR` terms can use that path too, but only when the
positive disjunction first collapses to one backend-neutral predicate that can
then be inverted. Negated factored `OR` groups can do the same only when the
positive factored group collapses all the way to one non-empty bounded
predicate. Negated `AND` groups can also reuse conservative branch subsumption
when the disjunction of negated terms collapses to one non-empty bounded
predicate. Negated same-property `OR` groups with a null branch can lower to a
bounded conjunction of `IS NOT NULL` plus negated equality or membership
terms. Folded `OR` value lists also drop exact duplicate alternatives while
preserving first-seen order, and repeated same-property membership predicates
are canonicalized when they can still be represented as one backend-neutral
membership predicate.
Empty positive membership intersections lower to an empty `IN` predicate,
which matches no rows. Same-property equality and membership combinations
collapse to equality, narrowed `IN`, or empty `IN` no-match predicates when
that preserves the `AND` semantics. Double negation over an otherwise bounded
predicate collapses back to the positive bounded predicate. Negated foldable
`AND` groups such as `NOT (n.status <> 'active' AND n.status <> 'pending')`
lower to the same grouped membership path when each negated term stays on one
property, and matching string exclusions such as
`NOT (NOT n.name STARTS WITH 'Ad' AND NOT n.name STARTS WITH 'Gr')` lower to
the grouped string path. Duplicate negated `AND` terms such as
`NOT (n.status = 'blocked' AND n.status = 'blocked')` collapse to the single
negated bounded predicate they represent. Nested negated string `AND` groups
can merge an already-grouped string predicate with another matching
same-property string predicate without adding a general boolean evaluator.
Factored `OR` branch pruning also recognizes exact string predicates covered
by sibling grouped string predicates over the same property, and negated
string branches covered by sibling grouped negated string predicates over the
same property. It also recognizes bounded predicates that imply broader
`IS NOT NULL` branches, plus exact-null predicates that imply `IS NULL`
without reversing missing-property semantics. Exact inequality branches can
also be pruned when a sibling branch already accepts the equivalent singleton
leading-`NOT` membership exclusion, and singleton membership branches can be
pruned when a sibling branch already accepts the equivalent exact equality;
equivalent singleton membership/equality branches keep the equality form.
Ordered-bound branches can also be pruned when the bound excludes the sibling
inequality value.
Scalar inequality combinations can similarly narrow `IN`, widen `NOT IN`, or
collapse contradictions to empty `IN`. Same-property ordered bounds keep the
stricter lower or upper bound and
collapse impossible ranges to empty `IN`. Equality combined with ordered
bounds collapses to equality when the value is inside the range, or empty
`IN` when it is outside. Positive `IN` lists combined with ordered bounds are
filtered to the surviving values, or to empty `IN` when no value remains.
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
transaction wrappers, currently pgGraph and SurrealDB, can report
`Transactional`.
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
execution remains Sail-specific over backend-neutral mutation plans.

`grust-surreal` provides both `SurrealHttpGraphStore` and
`SurrealSdkGraphStore`. It bootstraps namespaces/databases, maps labels and
relationships to Surreal tables, upserts nodes, and relates edges through
relation tables. Reads and traversal batch target-node lookups where possible.
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

Grust is pre-release.

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
- CocoIndex-style graph export adapter
- in-memory backend
- FalkorDB, HelixDB, LadybugDB, LanceDB, pgGraph, Sail, and SurrealDB backend
  crates

Planned:

- richer validation in `GraphBuilder`
- import/export helpers
- backend-specific schema lowering
- more traversal result shapes
- query and index helpers

## Development

Run the full test suite:

```sh
cargo test
```

Format the workspace:

```sh
cargo fmt
```

Run checks for all crates:

```sh
cargo check --workspace --all-targets
```

## License

Grust is dual-licensed under either of:

- Apache License, Version 2.0
- MIT license

Choose either license when using, modifying, or distributing Grust.
