# Grust Review & Improvement Plan (OPUS1)

Historical review of `~/src/grust` at branch `codex/cypher-write`. Findings are
ordered by their severity at that checkpoint; line numbers and implementation
claims are not current.

Status: **archived, not an active backlog.** Later releases addressed additional
items—including static Sail SQL allocation and Sail batch `get_nodes`—and
versioned path dependencies are intentionally required for crates.io packaging.
Revalidate any remaining observation against current code and open fresh work
instead of following the status labels below.

## Verification notes from the original review

- At that checkpoint, `cargo build --workspace --all-features` failed on a C++
  toolchain mismatch (`cxx-1.0.138` could not find `<algorithm>`), pulled in
  transitively by native backends. This recorded an environment/SDK issue, not
  a durable statement about the present workspace.
- At that checkpoint, `get_nodes` was overridden by `grust-memory`,
  `grust-pggraph`, `grust-ladybug`, `grust-lancedb`, and `grust-surreal`; the
  review's backend list must not be used as a current capability matrix.
- The reviewed WHERE operator table (`parse_where_predicate`, then in Sail)
  was correctly ordered (`>=`,`<=`,`<>`,`!=` before `=`,`>`,`<`) and was not a
  bug.

---

## 1. Bug — `contains("->")` is quote-blind (HIGH)

**Status:** fixed. Edge-vs-node pattern classification now uses the
quote-aware scanner, and `grust-cypher` owns tests for node `CREATE`/`MERGE`/
`DELETE` with `->` inside string literals plus edge properties containing
`->`.

**File:** `crates/grust-sail/src/lib.rs` — lines 404, 453, 503, 636, 738, 947

Edge-vs-node pattern classification uses a raw substring test:

```rust
if pattern.contains("->") { /* treat as edge pattern */ }
```

A node pattern whose property value contains `->` is misclassified as an edge
pattern and fails with a confusing error:

```cypher
CREATE (n:Server {id: "prod->primary"})
```

**Fix:** the file already has a quote-aware scanner. Replace each site:

```rust
if find_unquoted_sequence(pattern, "->").is_some() { ... }
// and the negated site at 636:
if find_unquoted_sequence(edge_pattern, "->").is_none() { ... }
```

**Test:** add a planner test that a node CREATE/MERGE/DELETE with `->` inside a
string property plans as a node op, and an edge pattern with `->` inside a
property value still parses as an edge.

---

## 2. Design — `MemoryGraphStore` silently drops parallel edges (MEDIUM)

**Status:** fixed. `MemoryGraphStore` now keys stored edges by endpoint,
label, and optional explicit edge ID, preserving id-bearing parallel edges.

**File:** `crates/grust-memory/src/lib.rs` — line 17

```rust
edges: BTreeMap<(NodeId, Label, NodeId), Edge>,
```

A `GraphBuilder` built with `EdgePolicy::AllowDuplicates`, or two edges with
distinct `EdgeId`s between the same endpoints, collapse to one on `put_graph` /
`put_edge` with no error or report signal. `LoadReport.edges` will also
over-count relative to what is stored.

**Fix options (pick one and document it):**
- Document explicitly that `MemoryGraphStore` enforces `DedupeByFromLabelTo`
  semantics regardless of builder policy, and verify `LoadReport` counts reflect
  stored (not attempted) edges; OR
- Key edges by `(NodeId, Label, NodeId, Option<EdgeId>)` so id-bearing parallel
  edges coexist, matching the reference in-memory store to the data model.

Recommended: the second option, since `MemoryGraphStore` is the reference/test
backend and should be the most faithful to the model.

---

## 3. Cleanup — static SQL builders allocate a fresh `String` per call (LOW)

**Status:** still open. Some static SQL helpers still allocate owned strings.

**File:** `crates/grust-sail/src/lib.rs` — lines 3494, 3498, 3502, 3511, 3523

`sail_out_degrees_sql`, `sail_in_degrees_sql`, `sail_degrees_sql`,
`sail_degree_pairs_sql`, and `sail_triplets_sql` return fully static content via
`.to_string()`.

**Fix:** change the return type to `&'static str` and drop `.to_string()`.
`sail_triplets_sql_for_direction` legitimately returns `String` (the
`Undirected` branch uses `format!`) and stays as-is; `sail_triplets_sql` can call
it or return the `Outgoing` constant directly.

---

## 4. Clarify — `GraphMutationReport::record` undercounts for matched ops (LOW/DOCS)

**Status:** fixed. `GraphMutationReport` documentation now describes the
difference between resolved single-identity counters and matched/bulk operation
counters.

**File:** `crates/grust-core/src/lib.rs` — lines 2484–2560

Resolved single-element ops (`PatchNode`, `PatchEdge`, `RemoveNodeProps`, …)
increment the granular counters (`node_patches`, `changed_nodes`, …). The
matched variants (`PatchMatchingNodes`, `PatchMatchingEdges`,
`RemoveMatchingNodeProps`, `UpdateMatchingNodeProperty`, `DeleteMatchingNodes`,
`DeleteMatchingEdges`) increment only the coarse counter (`patches` /
`deletes`). This is defensible — match cardinality is unknown at plan time — but
it is undocumented, so a caller summing `changed_nodes` will silently undercount.

**Fix:** add a doc comment to `GraphMutationReport` stating that granular
`changed_*` / `*_patches` / `*_deletes` counters reflect only resolved
single-identity operations, and that matched/bulk operations contribute solely
to the coarse `patches` / `deletes` / `merges` / `creates` totals (plus
`matched_rows` at execution time). Alternatively, introduce explicit
`matched_node_patches` / `matched_edge_patches` counters if call sites need them.

---

## 5. Performance — `get_nodes` sequential fallback in Falkor and Sail (LOW)

**Files:** `crates/grust-falkor/src/lib.rs`, `crates/grust-sail/src/lib.rs`

Both fall back to the default `get_nodes`, which issues N sequential `get_node`
round trips. Both backends can express a batched read:
- FalkorDB: a single `GRAPH.QUERY` with `WHERE n.id IN [...]`.
- Sail: a single `SELECT ... FROM grust_nodes WHERE id IN (...)` staged like the
  existing query paths.

**Fix:** override `get_nodes` in each to a single round trip; preserve input
order and skip missing ids (matching the documented contract).

---

## 6. Refactor — extract a `grust-cypher` crate (MEDIUM, architectural)

**Status:** fixed. The parser, planner, DDL helpers, constraint registry,
restricted returning evaluator, and generic returning executor now live in
`grust-cypher`; `grust-sail` keeps Sail-specific SQL/Spark execution and
compatibility wrappers.

**Files:** parser in `crates/grust-sail/src/lib.rs` (~350–2218); trait
`CypherMutationExecutor` in `crates/grust-core/src/lib.rs:2690`.

The Cypher-text → `GraphMutationPlan` planner lives entirely in `grust-sail`, but
the execution trait is in core and `grust-memory` already consumes it. Any third
backend wanting `sail_cypher_mutation_plan` would have to depend on `grust-sail`
purely for the parser, which is backwards.

**Fix:** move the parser, `CypherMutationOptions`, `CypherCreateMode`,
`CypherNodeIdPolicy`, `CypherNullAssignment`, and the planner into a new
`grust-cypher` crate depending only on `grust-core`. `grust-sail` re-exports for
back-compat. Do this when a second consumer of the parser appears; until then it
is a documented known boundary.

---

## 7. Refactor — `GraphMutation` / `GraphMutationPlanOp` duplication (LOW)

**File:** `crates/grust-core/src/lib.rs` — lines 2116–2192, 2341–2431, 2563–2682

The two enums are structurally parallel; `GraphMutationPlanOp` adds `kind` /
`cardinality`, and `From<GraphMutationPlanOp> for GraphMutation` is ~120 lines of
mechanical field-forwarding that drops that metadata. Every new operation must be
edited in three places.

**Fix (optional, larger):** consider having backends operate directly on
`GraphMutationPlanOp` and removing `GraphMutation`, or carrying the extra
metadata as an `Option<PlanMeta>` on a single enum. Defer unless the trio of
edits becomes a recurring maintenance cost.

---

## 8. Cleanup — redundant `version` on path deps (LOW)

**Status:** still open. `grust-sail` still carries versioned path dependency
entries for local Grust crates.

**File:** `crates/grust-sail/Cargo.toml` — lines 14, 26

```toml
grust-core   = { version = "0.8.4", path = "../grust-core" }
grust-memory = { version = "0.8.4", path = "../grust-memory" }
```

The pinned `version` is redundant for workspace path deps and will drift on every
workspace version bump. Other crates already use `path`-only or workspace
aliases.

**Fix:** drop the `version` field (publish still works via path+version
resolution at publish time), or add workspace dep aliases and use
`workspace = true` consistently across all crates.

---

## 9. Cleanup — triplicated quote-scanner loops (LOW)

**Status:** fixed. Writable Cypher scanners share the quote-aware scanning
path.

**File:** `crates/grust-sail/src/lib.rs` — lines 2127, 2154, 2189

`find_unquoted`, `find_unquoted_keyword`, and `find_unquoted_sequence` share an
identical quote/escape state machine, differing only in the match predicate at an
unquoted position.

**Fix:** extract one private scanner (e.g. an iterator over unquoted byte
offsets, or a closure-parameterized inner loop) and define the three in terms of
it. Low priority; purely DRY.

---

## Suggested order of work

1. **#1** quote-blind `->` — correctness, small, self-contained.
2. **#2** MemoryGraphStore parallel edges — correctness of the reference store.
3. **#3** `&'static str` SQL — trivial.
4. **#4** report-counter docs — trivial, prevents misuse.
5. **#5** batched `get_nodes` for Falkor/Sail — perf, needs integration env.
6. **#6** `grust-cypher` extraction — when a second parser consumer lands.
7. **#7 / #8 / #9** — opportunistic cleanups.
