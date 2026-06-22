# grust Architectural Review — Fable/Codex Task List

Status: active review backlog with stale baseline metadata. This review was
written against `v0.6.2`; some findings have since been fixed elsewhere, but
unmarked items should be revalidated against the current workspace before being
implemented.

**Codebase**: `~/src/grust` — v0.6.2, 10-crate workspace, Cargo edition 2024 / resolver 3  
**Review date**: 2026-06-12  
**Reviewer**: Fable (claude-sonnet-4-6)

This document follows the same format as `typesec/docs/fable-review-2.md`. Tasks are grouped into Rust quality (Q), design/protocol (D), and testing (T). Each task has a priority tag: **[CRITICAL]**, **[HIGH]**, or **[MEDIUM]**, and enough context for Codex to implement the change without additional archaeology.

---

## Part 1 — Rust Quality

### Q-1 `relationship_type` is copy-pasted across three crates [HIGH]

**File**: `grust-falkor/src/lib.rs:342–358`, `grust-helix/src/lib.rs:767–783`, `grust-surreal/src/lib.rs:1011–1027`

The function `fn relationship_type(value: &str) -> String` (normalises an edge label to UPPER_SNAKE_CASE for databases that require it) is identical in all three crates. Add a `pub fn relationship_type(value: &str) -> String` to `grust-core/src/lib.rs` (or a new `grust_core::util` module), export it from the prelude, and delete the three local copies.

---

### Q-2 `schema_identifier` is copy-pasted across four crates [HIGH]

**File**: `grust-falkor/src/lib.rs:381–388`, `grust-lancedb/src/lib.rs:839–851`, `grust-pggraph/src/lib.rs:649–662`, `grust-sail/src/lib.rs:1008–1030`

Four slightly diverging implementations of "turn arbitrary text into a safe SQL/backend identifier". They differ in case handling (lower vs upper vs preserve) in subtle ways. Extract a canonical `pub fn schema_identifier(value: &str) -> Result<String>` into grust-core that normalises to lower_snake_case, export it, and replace all four with the shared version. Audit each call site for the case-folding assumption.

---

### Q-3 `edge_key` using `\u{1f}` is duplicated and undocumented [HIGH]

**File**: `grust-lancedb/src/lib.rs:787–799`, `grust-sail/src/lib.rs:993–1006`, `grust-cocoindex/src/lib.rs:110–123`

The composite edge key `format!("{}\u{1f}{}\u{1f}{}", from, label, to)` is copy-pasted verbatim. If any node ID or label ever contains the Unicode Unit Separator (U+001F) the key is ambiguous. Move `fn edge_key(edge: &Edge) -> String` to grust-core (or onto `Edge` as a method `pub fn composite_key(&self) -> String`), add a doc comment explaining the delimiter choice and its constraint, and replace all three with the shared version.

---

### Q-4 `FalkorGraphStore` opens a new TCP connection per operation [HIGH]

**File**: `grust-falkor/src/lib.rs:38–59`

`fn connection(&self) -> Result<redis::Connection>` creates a brand-new TCP connection on every single `put_node`, `put_edge`, `apply_schema`, and `clear` call. For bulk loads this means hundreds of connection setups and teardowns.

Replace the raw `redis::Client` with a connection pool using `r2d2` + `r2d2-redis` (or `deadpool-redis`). Store the pool in `FalkorGraphStore` instead of a bare `FalkorConfig`, and acquire a connection from the pool at the start of each method.

---

### Q-5 `SurrealSdkGraphStore` issues a `USE NS … USE DB …` round-trip on every operation [HIGH]

**File**: `grust-surreal/src/lib.rs:267–278`

`async fn select_database(&self) -> Result<()>` sends a `USE NS … USE DB …` command on every `put_node`, `put_edge`, `get_node`, `get_edges`, and `traverse` call. The SurrealDB Rust SDK keeps connection state: call `select_database` once in `connect()` and remove all subsequent calls.

---

### Q-6 `traverse_steps_with_store` issues N×M `get_node` round-trips [HIGH]

**File**: `grust-surreal/src/lib.rs:919–968`, and mirrored in `grust-lancedb/src/lib.rs:252–300`

The shared traversal helper calls `store.get_node(target_id).await?` inside a loop over edges, per traversal step. For a step that fans out to 50 target nodes this is 50 serial round-trips. Replace with a batch-read: collect all target IDs into a `Vec`, then issue a single `WHERE id IN (…)` query (SurrealDB: `SELECT * FROM … WHERE id IN [type::record…]`; LanceDB: `id IN ('a','b','c')`). Add a `get_nodes(ids: &[NodeId]) -> Result<Vec<Node>>` default-implemented helper to `GraphStore`.

---

### Q-7 `validate_json_key` in grust-pggraph allows injection-risk characters [HIGH]

**File**: `grust-pggraph/src/lib.rs:640–647`

```rust
fn validate_json_key(value: &str) -> Result<()> {
    if value.contains('\0') || value.is_empty() { … }
```

A key like `'} OR 1=1--` passes this check and is interpolated into the JSONB path expression `props #>> ARRAY['{key}', 'value']`. The array notation makes literal SQL injection unlikely, but the key still lands unquoted in a `CREATE INDEX` context in `pggraph_prop_expr`. Tighten validation to alphanumeric + underscore only, matching `validate_identifier`.

---

### Q-8 `SurrealHttpGraphStore` manually builds HTTP Basic auth instead of using reqwest's helper [MEDIUM]

**File**: `grust-surreal/src/lib.rs:61–64, 83–86, 104–107, 128–131`

```rust
let auth = general_purpose::STANDARD.encode(format!("{}:{}", self.config.user, self.config.pass));
.header("Authorization", format!("Basic {auth}"))
```

`reqwest::RequestBuilder` has `.basic_auth(username, Some(password))`. Replace all four copy-pastes. This also removes the `base64` import from grust-surreal's Cargo.toml.

---

### Q-9 `PutOutcome::Inserted` / `Updated` are never returned by remote backends [MEDIUM]

**File**: `grust-core/src/lib.rs` (`PutOutcome` definition) and all backend `put_node`/`put_edge` impls

Every remote backend returns `PutOutcome::Upserted` unconditionally because upsert semantics don't distinguish insert from update without a SELECT first. The trait contract implies callers can differentiate but they cannot for remote stores.

Options (pick one and document it):
1. Change `GraphStore::put_node` / `put_edge` to return `()` and move `PutOutcome` to `MemoryGraphStore` only.
2. Keep `PutOutcome` but document that remote backends always return `Upserted` and callers must not rely on `Inserted`/`Updated` across backends.

Whichever approach is chosen, update all doc comments and backend implementations to be consistent.

---

### Q-10 `Value::DateTime` can be constructed without validation [MEDIUM]

**File**: `grust-core/src/lib.rs`

`Value::DateTime(String)` is a public variant. Anyone can write `Value::DateTime("not-a-date".to_string())` and bypass the RFC 3339 guard in `Value::datetime()`. Make the `DateTime` variant hold a newtype `struct RfcDate(String)` whose only constructor is `RfcDate::parse(s)` which calls `is_rfc3339_datetime`. Alternatively, replace the inner `String` with `chrono::DateTime<chrono::FixedOffset>` and parse on construction. The former is lower-churn.

---

### Q-11 `grust` facade re-exports `MemoryGraphStore` only; other backends export everything [MEDIUM]

**File**: `crates/grust/src/lib.rs:16–17`

```rust
#[cfg(feature = "memory")]
pub use grust_memory::MemoryGraphStore;
```

All other backends are `pub use crate::*`. This inconsistency means `MemoryGraphConfig` (if it existed) would not be re-exported. Either re-export `grust_memory::*` (consistent with other backends) or document why MemoryGraphStore has a bespoke export.

---

### Q-12 `#[must_use]` missing on builder `build()` and `finish()` methods [MEDIUM]

**File**: `grust-core/src/lib.rs` — `GraphBuilder::build()`, `NodeBuilder::finish()`, `EdgeBuilder::finish()`

A caller could write `builder.node("Person", "a").finish();` intending to chain but actually discarding the node — the compiler won't warn. Add `#[must_use = "discarding this means the node/edge was not added to the builder"]` to all three.

---

### Q-13 `LanceDbGraphStore` node property search uses fragile `LIKE '%json_fragment%'` [HIGH]

**File**: `grust-lancedb/src/lib.rs:755–768`

```rust
Start::NodesByProperty { label, key, value } => {
    …
    Ok(format!("label = {} AND props LIKE {}",
        sql_str(label.as_str()),
        sql_str(&format!("%{}%", json_property_fragment(…)?))
    ))
}
```

This LIKE-substring match on the serialised JSON props blob will produce false positives (matches on key names, partial values, values of other keys). Replace with `GET_JSON_OBJECT(props, '$.{key}')` (which LanceDB's SQL layer supports through DuckDB's JSON functions) and perform an equality or cast comparison, matching the pattern used in grust-sail's `start_clause`.

---

### Q-14 `SailGraphStore` builds positional SQL args then immediately inlines them [MEDIUM]

**File**: `grust-sail/src/lib.rs:918–940`, `run_command`/`run_query` at lines 186–212

`run_command` and `run_query` both call `inline_sql_args(sql, &args)?` to turn `?` placeholders into literal SQL before sending — discarding the positional args path that Spark Connect's gRPC protocol natively supports. The `command_request` and `query_request` methods accept `args: Vec<expression::Literal>` but the callers always pass `vec![]` after inlining. Remove `inline_sql_args` entirely, pass `args` properly via the gRPC `pos_args` / `pos_arguments` field, and remove the `#[allow(deprecated)]` annotation.

---

### Q-15 `SailGraphStore` is not `Clone` or `Sync` [MEDIUM]

**File**: `grust-sail/src/lib.rs:53–57`

`SparkConnectServiceClient<Channel>` is `Clone` (it wraps a `Channel` which is cheaply cloneable). `RwLock<Option<GraphSchema>>` is `Sync`. Wrapping in `Arc<SailGraphStore>` should be unnecessary — derive or implement `Clone` so the store can be passed around without `Arc`, consistent with `LanceDbGraphStore: Clone`.

---

## Part 2 — Protocol and Design

### D-1 `Traversal` DSL has no property-filter step [HIGH]

**File**: `grust-core/src/lib.rs` — `Step`, `Traversal`

The builder offers `.out("edge")`, `.in_("edge")`, `.to("Label")`, `.limit(n)`, but there is no `.where_prop(key, value)` or `.filter(|node| …)`. A query like "all Events with `capacity > 50` reachable from this Group" requires fetching all Events and filtering in application code, or hand-writing backend-specific queries.

Add a `Step::filter: Option<(String, Value)>` (or a richer `Vec<Predicate>`) and implement it in at least `MemoryGraphStore` and `PgGraphStore` (WHERE clause on node label and property). Other backends can `return Err(GrustError::Unsupported)` initially.

---

### D-2 `GraphMutationStore::apply_mutations` has no transactional guarantee [HIGH]

**File**: `grust-core/src/lib.rs` — `GraphMutationStore` trait

The default `apply_mutations` iterates mutations one at a time. If the third mutation fails, the first two are already committed. Backends that support transactions (PostgreSQL, SurrealDB) should wrap the mutation batch in `BEGIN … COMMIT` / `TRANSACTION`. Add documentation to the trait that the default impl is not atomic, and add an override in `grust-pggraph` and `grust-surreal` that wraps the full mutation list in a transaction.

---

### D-3 `GraphSchema` validation contract is undocumented and inconsistent across backends [HIGH]

**File**: `grust-core/src/lib.rs` (`GraphStore::apply_schema` and `put_node`), all backend impls

- **Memory**: validates on every `put_node` / `put_edge` after `apply_schema` is called.
- **PgGraph**: `apply_schema` creates typed views and indices, but does NOT enforce required-field constraints — invalid rows can be inserted.
- **Falkor**: `apply_schema` only creates indices, no enforcement at all.
- **Sail**: validates before write (calls `schema.validate_node(node)?`).
- **Helix**: `apply_schema` only calls `validate_helix_schema` (identifier check), no enforcement.

Document the contract clearly in the `GraphStore` trait: which backends enforce it at write time, which only at index creation. For backends that claim enforcement, add integration tests that attempt invalid writes and assert errors.

---

### D-4 `CocoIndexExport` is write-only; no `from_cocoindex_export` [MEDIUM]

**File**: `grust-cocoindex/src/lib.rs`

The crate can convert `Graph → CocoIndexGraphExport` but not the reverse. Add `pub fn cocoindex_export_to_graph(export: CocoIndexGraphExport) -> Result<Graph>` so that graphs emitted by CocoIndex pipelines (which use the same JSON format) can be loaded back into grust.

---

### D-5 `GraphMutation::DeleteEdge` carries redundant `id: Option<EdgeId>` [MEDIUM]

**File**: `grust-core/src/lib.rs` — `GraphMutation` enum

```rust
GraphMutation::DeleteEdge {
    id: None,          // always None in all call sites
    from: NodeId,
    label: Label,
    to: NodeId,
}
```

The `id` field is set to `None` in every call site found in the codebase. No backend uses it; the delete is always by `(from, label, to)`. Either remove the `id` field (breaking change, intentional cleanup) or split into `DeleteEdgeById(EdgeId)` and `DeleteEdge { from, label, to }` so both paths are usable.

---

### D-6 `SurrealConfig` requires caller to supply `labels` and `relationships` for generic reads [HIGH]

**File**: `grust-surreal/src/lib.rs:20–41`, `surreal_get_edges_query`, `surreal_node_tables_for_id`

Without `config.labels` and `config.relationships`, `get_edges` returns an empty result and `get_node` only searches the `record` table. This is a silent correctness bug: callers who use `SurrealConfig::default()` and skip the field population get empty read results with no error. 

Fix options:
1. During `put_node`, track seen labels in a persistent table (`grust_labels`) and query that during reads — no caller-supplied list needed.
2. At minimum: return `Err(GrustError::Backend("config.labels is empty; reads will return no results. Populate config.labels with all node type names."))` when labels is empty and a read is attempted.

---

### D-7 Helix backends silently drop non-string properties [CRITICAL]

**File**: `grust-helix/src/lib.rs:298–307, 407–420`

```rust
fn helix_http_properties(node: &Node) -> Vec<serde_json::Value> {
    node.props.iter().filter_map(|(key, value)| match (key.as_str(), value) {
        ("labels", _) => None,
        (_, Value::String(value)) => Some(…),
        (_, _) => None,   // ← Int, Float, Bool, Array, Json all silently dropped
    })
```

Any node or edge with `Int`, `Float`, `Bool`, `IntArray`, `FloatArray`, or `Json` properties will write those properties to Helix as nothing — the data is silently lost. This is the most critical correctness bug in the codebase.

Fix: Either map each grust `Value` variant to the corresponding Helix property type (`{"I64": n}`, `{"F64": f}`, `{"Boolean": b}`) per the Helix API, or return `Err(GrustError::Unsupported("Helix backend only supports String properties"))` for nodes/edges carrying non-string values. The former is correct; the latter is honest.

---

### D-8 No streaming or cursor pagination in `GraphStore` reads [MEDIUM]

**File**: `grust-core/src/lib.rs` — `GraphStore` trait

`get_edges` and `traverse` return `Vec<Node>` / `Vec<Edge>`, loading the full result into memory. For a graph with 10M edges, `get_edges(EdgeQuery::default())` allocates unboundedly.

Add an optional `list_nodes` cursor API:
```rust
async fn list_nodes(&self, after: Option<&NodeId>, limit: usize) -> Result<Vec<Node>>;
async fn list_edges(&self, after: Option<&EdgeId>, limit: usize) -> Result<Vec<Edge>>;
```
Backends can provide default impls that call `get_edges` and slice; memory/pggraph can implement efficient cursor-based variants.

---

### D-9 `GraphStore::bootstrap` no-op default leads to confusing backend errors [MEDIUM]

**File**: `grust-core/src/lib.rs` — `GraphAdminStore::bootstrap`

The default impl is:
```rust
async fn bootstrap(&self) -> Result<()> { Ok(()) }
```

For backends where `bootstrap()` must be called before first write (PgGraph, LanceDB, Sail), skipping it produces confusing errors like "LanceDB table grust_nodes not found" instead of a clear message. Add a `fn requires_bootstrap() -> bool { false }` to `GraphAdminStore` with an override in backends that require it; `put_graph` should check and auto-call `bootstrap()` if needed, or at minimum return a clear error with a hint.

---

### D-10 `SailConfig::default()` generates a new UUID session ID on every call [MEDIUM]

**File**: `grust-sail/src/lib.rs:34–43`

```rust
session_id: uuid::Uuid::new_v4().to_string(),
```

`SailConfig::default()` is a pure value; generating a random UUID there violates the principle that `Default::default()` should return a deterministic, inert value. Move `uuid::Uuid::new_v4()` into `SailGraphStore::connect()` — config should carry `session_id: Option<String>` and the store fills it in with a generated UUID if `None`.

---

### D-11 `grust-helix` has two parallel store implementations (HTTP and SDK) with diverging semantics [MEDIUM]

**File**: `grust-helix/src/lib.rs`

`HelixHttpGraphStore` and `HelixSdkGraphStore` exist side-by-side. They share the request-building helpers but have subtle differences:
- `HelixHttpGraphStore::clear()` calls `helix_drop_labels_request`; `HelixSdkGraphStore::clear()` calls `post_helix_sdk_drop_labels`. Different code paths, not obviously equivalent.
- `HelixHttpGraphStore::read()` parses a JSON response through `helix_response_items` with four fallback shapes; `HelixSdkGraphStore` uses `send_helix_sdk_read` which deserialises straight to `serde_json::Value` through the SDK.
- The HTTP store hard-codes `relationship_type(edge.label)` in edge storage but `helix_http_edge_properties` stores `edge.label.as_str()` as-is in the `relationship` property — inconsistency.

Document which store should be preferred, deprecate the other, or unify them behind a single `HelixBackend` enum config.

---

### D-12 `typed` module has no backend integration path [MEDIUM]

**File**: `grust-core/src/lib.rs` — `typed` module (feature `typed-garde`)

`TypedGraphBuilder` validates and builds a `Graph` from typed domain structs. Once a `Graph` is produced, the caller calls `store.put_graph(&graph)` manually. There is no `TypedGraphBuilder::put_into(store)` convenience, nor any typed read path: there's no `TypedNode::from_node(node: &Node) -> Result<Self>` — you can write typed, but you can only read back as untyped `Props`.

Add:
1. `TypedNode::from_node(node: &Node) -> Result<Self>` (deserialise from `Props` through serde/garde).
2. `TypedEdge::from_edge(edge: &Edge) -> Result<Self>`.
3. A doc example showing a full round-trip write → read → typed.

---

## Part 3 — Testing

### T-1 All backend query-generation functions lack unit tests [CRITICAL]

**Files**: `grust-falkor/src/lib.rs`, `grust-helix/src/lib.rs`, `grust-pggraph/src/lib.rs`, `grust-surreal/src/lib.rs`, `grust-sail/src/lib.rs`

The query-building functions in every backend (Cypher, SQL, JSON request bodies) are pure functions of their inputs, yet none have unit tests. Regressions in SQL quoting, identifier escaping, or batch assembly are invisible unless you run an end-to-end test with a live server.

Add a `#[cfg(test)]` block in each crate's `lib.rs` (or a `unit_tests.rs`) covering at minimum:
- Single-node upsert SQL/Cypher string matches snapshot.
- Single-edge upsert with special characters in the label.
- Traversal SQL for a two-hop `.out("A").out("B")` traversal.
- Schema DDL for a node type with all `FieldType` variants.

Use `insta` for snapshot tests or inline `assert_eq!` for concise assertions.

---

### T-2 No cross-backend conformance test harness [CRITICAL]

**Files**: All backend crates, `grust-memory/src/tests.rs` (has the reference behaviour)

`MemoryGraphStore` has 5 well-written tests. None of the other backends have equivalent behavioural tests. Add a shared conformance test function:

```rust
pub async fn run_conformance_tests(store: &(impl GraphStore + GraphAdminStore + GraphMutationStore)) {
    // stores and retrieves a node
    // traverses one hop
    // validates schema enforcement
    // delete_node cascades to edges
    // apply_mutations is atomic
}
```

Put it in `grust-core/src/tests.rs` (behind `#[cfg(test)]`) and call it from each backend's integration test module. For backends requiring a live server, gate behind `#[cfg(feature = "integration-tests")]`.

---

### T-3 `inline_sql_args` in grust-sail is untested [HIGH]

**File**: `grust-sail/src/lib.rs:918–940`

`inline_sql_args` is a security-sensitive function: it substitutes `?` placeholders with literal SQL values. It should be tested for:
- String with single quotes: `O'Brien` → `'O''Brien'`.
- String with backslashes: `a\b` → `'a\\b'` (Spark SQL escape).
- Non-finite float: `f64::INFINITY` → `Err(…)`.
- Mismatched placeholder count → `Err(…)`.
- Empty query with zero args.

---

### T-4 `helix_response_items` multi-path parsing is untested [HIGH]

**File**: `grust-helix/src/lib.rs:611–638`

The function tries four different JSON response shapes. At least one of the shapes is likely dead code (the API has settled). Write unit tests for each documented response shape to verify the parser handles them correctly, and remove shapes that no longer match the live API.

---

### T-5 `is_rfc3339_datetime` hand-rolled validator should be replaced and tested [MEDIUM]

**File**: `grust-core/src/lib.rs` — `is_rfc3339_datetime`

The validator uses a custom character-by-character parser. Known gaps: leap seconds (`23:59:60`), sub-second precision beyond nanoseconds, `Z` vs `+00:00` equivalence, negative zero offset. Replace with `chrono::DateTime::parse_from_rfc3339(s).is_ok()` (already a dev/test dependency) and add tests for:
- Leap second `2023-06-30T23:59:60Z` (should be allowed or explicitly rejected).
- Sub-second `2024-01-01T00:00:00.123456789Z`.
- All valid offset forms: `+05:30`, `-07:00`, `Z`.

---

### T-6 `TypedGraphBuilder` round-trip through a persistent backend is untested [MEDIUM]

**File**: `grust-core/src/lib.rs` — `typed` module

The typed builder has good unit tests in `grust-core/src/tests.rs` for garde validation and zod-rs schema generation, but no test shows `TypedGraphBuilder` → `Graph` → `MemoryGraphStore::put_graph` → `MemoryGraphStore::get_node` and verifying that the props survive the round-trip. Add this as a test in `grust-memory/src/tests.rs`.

---

### T-7 `SurrealDB` response parsing helpers lack unit tests [HIGH]

**File**: `grust-surreal/src/lib.rs` — `surreal_node_from_value`, `surreal_edge_from_value`, `surreal_record_id`

These JSON-to-struct parsers handle multiple SurrealDB record ID formats (string `"table:id"`, object `{"id": {"String": "…"}}`, backtick-quoted strings). Add unit tests covering each ID format variant and verifying that the label, from/to, and props are correctly extracted.

---

### T-8 grust-cocoindex `edge_to_state` error path for missing nodes is tested but the reverse direction is not [MEDIUM]

**File**: `grust-cocoindex/src/lib.rs` — `tests`

The existing test `missing_endpoint_node_is_an_error` covers the "missing target" case. Add:
- Missing *source* node (edge `from` references a node not in the graph).
- Graph with zero edges (no relationships in export).
- Non-finite float in a node property (`value_to_json` returns `Err` for `f64::NAN`).
- Explicit `EdgeId` vs composite key path.

---

## Summary

| Priority | Count | Tasks |
|----------|-------|-------|
| CRITICAL | 3 | D-7, T-1, T-2 |
| HIGH | 17 | Q-1, Q-2, Q-3, Q-4, Q-5, Q-6, Q-7, Q-13, D-1, D-2, D-3, D-6, T-3, T-4, T-7 |
| MEDIUM | 17 | Q-8 through Q-12, Q-14, Q-15, D-4 through D-5, D-8 through D-12, T-5, T-6, T-8 |
| **Total** | **37** | |

### Recommended order for Codex

1. **D-7** (Helix property data loss) — correctness bug, highest impact.
2. **Q-1, Q-2, Q-3** (deduplication) — reduce maintenance surface before adding features.
3. **T-1, T-2** (unit tests for SQL builders + conformance harness) — essential before any refactor.
4. **Q-4, Q-5, Q-6** (connection management) — performance and reliability.
5. **D-1** (traversal property filter) — unlocks real use cases.
6. **D-6** (SurrealDB labels requirement) — silent correctness bug.
7. **Q-7, Q-13** (injection / LIKE fragility) — safety.
8. Remaining MEDIUM items in any order.
