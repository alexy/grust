# Indexed reads and scalar counts

Grust's portable read executor remains available for the full supported read
surface. The indexed entrypoints select exact, non-materializing count
algorithms for proven query shapes and otherwise use that same executor.
Selection is structural: no query IDs, dataset names or expected answers enter
the optimizer.

## Reusable immutable snapshots

`grust_core::TypedGraphIndex::new(Arc<Graph>)` owns the graph snapshot and indexes
node IDs, labels, and typed incoming/outgoing adjacency. Vertex and edge slots
are `u32`; duplicate node IDs, missing edge endpoints and exceeded slot capacity
return errors. Sorted adjacency preserves parallel and reciprocal edges and
self-loops rather than deduplicating away multiplicity.

Dense offsets give constant-time source lookup for sufficiently populated
relationship types; sparse types binary-search their sorted active sources.
This keeps structural auxiliary storage O(V + E), rather than allocating
V-sized offsets for every rare type. Construction still sorts adjacency and
scans the serialized graph once; it is not free load work.

`index.adjacency(relationship)` returns a copyable `TypedAdjacencyView` that
resolves the type once. Its incoming/outgoing methods borrow sorted rows without
rehashing the type. The view and slices borrow the index, not the name string;
absent types and invalid vertex slots return empty rows. Copying a view neither
allocates nor clones the graph or its `Arc`. Dense/sparse lookup behavior is
unchanged, and views do not provide a budget exemption.
The `sparse_outgoing_sources()` and `sparse_incoming_sources()` accessors borrow
sorted unique vertex slots with nonempty rows, without scanning or allocating.
`None` means dense storage has no cheap borrowed source list, not an empty
relationship; an absent type returns `Some(&[])`.

`MemoryGraphStore::indexed_snapshot()` returns an `Arc<TypedGraphIndex>`. The
first read after a write clones the stored graph and constructs the index;
subsequent reads, including through cloned store handles, share it. Every write
attempt invalidates the cache, including attempts that fail validation. Already
returned snapshots remain immutable and usable. Cache construction holds the
store's read lock, so writers wait for that first build. Ordinary `GraphStore`
reads and traversals retain their existing behavior.

```rust
use grust_core::{Graph, GraphStore};
use grust_cypher::{CypherParameters, read::run_read_query_indexed};
use grust_memory::MemoryGraphStore;

async fn count_people(graph: &Graph) -> grust_core::Result<()> {
    let store = MemoryGraphStore::new();
    store.put_graph(graph).await?;
    let index = store.indexed_snapshot()?;
    let result = run_read_query_indexed(
        &index,
        "MATCH (:Person) RETURN count(*) AS people",
        &CypherParameters::new(),
    )?;
    assert_eq!(result.columns, ["people"]);
    Ok(())
}
```

The text entrypoint parses, checks default graph selection, analyzes bindings
and proves the plan on every call. `read::execute_read_query_indexed` accepts an
already-parsed query under the same caller contract as `execute_read_query`.
`read::classify_indexed_read_query` accepts a semantically valid AST and reports
`IndexedReadPlan::{CountFactorized, ClausePipeline}`. Classification alone is
neither execution nor policy validation, and does not cache a plan.

## Exact fast-path scope

The graph-pattern algorithms require a single `RETURN count(*)`, optionally
aliased, with optional nonnegative literal `SKIP` and `LIMIT`. They do not
handle grouping, `DISTINCT`, ordering or named paths. Scalar scans additionally
admit counts of a proven nonnull binding, zero-hop paths and scalar unions, as
described below. Each algorithm proves its own narrow clause and binding scope.

**Pattern forests.** One or more `MATCH` clauses may describe chains, stars or
disconnected trees, sharing node variables without creating a cycle. Each
relationship position must name exactly one type, different from every other
position's type. Incoming, outgoing and undirected edges are supported. Node
labels and inline scalar literal properties on nodes and relationships are
checked using reference semantics. General `WHERE`, parameterized properties,
cycles and repeated relationship types fall back. Bottom-up weighted counts
retain edge multiplicity; disjoint types prove that different positions cannot
reuse the same relationship. Counts use nonnegative capped arithmetic so a
zero-result component still annihilates an overflowing intermediate component.
Mandatory branch combination reuses the predicate pass's borrowed necessary
candidates. Weights outside that seed remain zero, including through
optional padding and earlier branch products. Every candidate still checks
its current weight; initialization, optional scans and root sums retain their
full-domain charges. No extra label lookup or candidate allocation is needed.
Mandatory child-branch edge scans prepay at most 256 physical slots per chunk,
including incoming copies of undirected self-loops that are charged but not
counted twice. Successful scan totals remain exact, with no added allocation.
A tight budget can refuse the next complete chunk before its affordable prefix;
predicate charges remain local and may fail after later slots were prepaid.
Property-free scans checkpoint at most 256 slots apart, while property checks
retain their own budget checkpoints. Optional execution is unchanged.

Before property evaluation, a role with a nonempty literal node-property map and
at least two mandatory incident atoms requires a nonempty typed adjacency row
for every such atom, oriented relative to that role. An undirected atom tries
outgoing first, then incoming only on a miss. The role prepares a charged vector
of borrowed typed views and role-relative directions, resolving each type once.
Each actual row probe is charged separately. Disabled and empty-candidate roles
prepare nothing. Enabled roles inspect each prepared atom's metadata with one
work charge and may replace label/full-domain candidates with a strictly shorter
borrowed sparse source list for a mandatory directed atom. Ties keep the current
seed; dense and undirected atoms supply no seed. No lists are intersected or
copied, and no neighbor scan or candidate allocation is added. This is only
a necessary condition: every original label/property predicate and branch computation still
runs for survivors. Optional adjacency never qualifies a mandatory candidate.
Degree-one and property-free roles do not use this prefilter. The structural
gate is a heuristic, not a selectivity estimate; dense roles can pay extra work.

**Independent optional leaves.** A mandatory forest may be followed by one or
more single-edge `OPTIONAL MATCH` clauses. Each connects a still-live mandatory
variable to a fresh leaf; directions, labels and inline scalar literal
properties are supported. Types remain globally distinct. Each optional leaf
multiplies the anchor's weight by `max(1, matching edges)`, preserving one padded
row when no edge matches or the optional anchor's own predicates fail. It does
not turn those predicates into mandatory filters.

One plain `WITH` may precede the optional suffix if it contains only unique,
unrenamed mandatory variables, without filters, grouping, ordering or
pagination. Dropped bindings still contribute their incoming multiplicity.
Explicit `WHERE`, multi-pattern/multi-hop optional clauses, nullable anchors,
reused leaf variables and later mandatory clauses fall back. Fresh optional
variables may bind the same physical node; undirected self-loops count once.

**Two-hop wedges.** The additional shape is:

```cypher
MATCH (a)-[:T]-(b)-[:T]-(c)-[:U]->(d)
WHERE a <> c
RETURN count(*)
```

`T` and `U` must differ. The four logical node slots must be distinct, with
labels allowed; inline property maps and additional predicates are not yet
supported. The actual graph nodes may coincide except where `a <> c` forbids
it. One grouped adjacency pass per center accumulates the A degree, C leaf
weights `sum_C(m * L)`, and overlap `sum_A_and_C(m * m * L)`. Its contribution
is `degree_a * weighted_leaves - overlap`, exactly excluding equal outer nodes.
Incoming/outgoing merging counts each self-loop once and preserves reciprocal
and parallel edges. Multiplicity and A degree fit checked `u32`; weighted leaf
totals fit checked `u64` because they are at most `E_T * E_U < 2^62` under the
index's global u32 edge bound and distinct T/U types. Overlap, final products,
subtraction and accumulated count use checked `u128`, not the forest's cap;
the final scalar is checked against `i64` only after subtraction. No additional
scratch allocation is needed, and every traversed slot/group remains charged.
Physical-slot scan charges are prepaid in chunks of at most 256 across the two
rows, with a separate charge before each grouped-endpoint callback and a final
deadline check. This bounds unchecked scan work even for a single huge parallel
group. Successful totals are unchanged; a tight budget can refuse a whole chunk
before doing partially affordable work. Callbacks hold no borrowed budget state
and may safely perform other budgeted operations. The anti-join support scan
does not use this helper.
Multiplicity comes from the drained outgoing/incoming span lengths, excluding
the incoming self-loop copy, with a checked u32 conversion once per group.
Raw scan indices/totals remain checked usize because loop copies can require
up to `2 * E_T` scanned slots. The shared leaf array remains `u64`.

The anti-join variant replaces the inequality filter with a bare optional
`(a)-[k:T]-(c)` and an exact variable-only `WITH ... WHERE k IS NULL AND a <> c`.
Every surviving a/b/c triple has distinct vertices: either equality with b
would itself provide the forbidden a–c edge. The count therefore omits T
self-loops, computes weighted unequal-endpoint wedges, and subtracts weighted
support triangles. Each triangle contributes all six role placements, using
the two matched arm multiplicities and the outgoing leaf weight. The closing
edge supplies existence only, never a third multiplicity factor.

Degree-ranked support intersections avoid repeating neighbor probes for every
wedge. They share the location-triangle topology helper and use O(V + M)
additional scratch, where V is the active domain size and M counts distinct
non-loop T endpoint pairs. The helper ranks vertices by `(simple support degree,
stable graph vertex slot)`, not weighted degree, and stores forward targets in
increasing rank order. For a forward edge x–y, every forward neighbor of y has
rank greater than y, so only the strict suffix after y in x's row can intersect
y's row. A suffix in the original ordinal order would not have this guarantee.
Triangle callbacks translate ranks back to original active-domain ordinals,
keeping the existing role masks and path/leaf weights attached to the correct
vertices. The weighted location-triangle and wedge anti-join semantics are
unchanged.

Explicit rank construction costs O(V log V) and allocates two charged V-entry
`u32` maps: rank-to-ordinal and ordinal-to-rank. The latter is dropped after
forward adjacency is filled; the former remains for callbacks. Sorting forward
rows costs O(M log M) in the worst case, and triangle intersections retain their
O(M^(3/2)) bound. Support construction, sorting, comparisons and role
contributions remain charged; memory exhaustion is an error, not a retry
through an unmetered algorithm.
Stored support multiplicities fit `u32` because each counts a subset of the
index's physical edges; checked conversion enforces this storage invariant,
and count products and subtraction still use `u128`. The anti-wedge's two
fixed-cost active-mask scans precharge at most 256 entries per chunk, with
end-of-pass deadline checks. Successful work totals are unchanged; an
insufficient budget may conservatively refuse before a partly affordable chunk.
Wedge role masks set unconditional unlabeled-role bits during initialization
and inspect only required-label candidates for labeled roles. Every label
conjunct remains checked, and roles may overlap. Leaf traversal and non-anti
center traversal reuse the already-borrowed C/B label candidates, respectively,
without additional lookups or allocation. They still check the completed role
masks and charge every candidate visit, even if its remaining predicates fail.
Unlabeled roles retain full-domain scans. Full-size mask/leaf initialization
and arrays remain accounted; the anti-join's active-domain scan is unchanged.

**Tag/reply shapes.** A directed path `(a)<-[:T]-(m)<-[:U]-(c)-[:T]->(b)` with
`a <> b` and distinct T/U types counts the product of the two tag degrees minus
their multiplicity-weighted intersection, multiplied by reply multiplicity.
Labels and scalar literal properties remain part of each role/edge predicate.
Its optional-null anti-join variant excludes a left target if *any* raw T edge
from c reaches it, even if that witness fails the right role's filters. It
does not multiply by the witness count or enumerate matching tuples.

**Scalar scans.** A single node or fixed-edge pattern may count `*` or its
guaranteed-bound node/relationship variable. Label-only node cardinalities and
bare directed-edge cardinalities read the index size directly; filtered scans
retain literal property and direction semantics. A literal `*0..0` path binds
both endpoints to the same node, with both endpoint predicates checked. Simple
node-property `IS [NOT] NULL`, boolean constants and constant string equality
are supported; JSON null wrapped in `Value::Json` remains distinct from
`Value::Null`, as in the reference evaluator.

Literal `UNWIND range(...)` counts use inclusive arithmetic without allocating
the list, while retaining the universal and caller range/work ceilings and
zero-step error. Compatible scalar scan arms may use `UNION`/`UNION ALL`, with
the same column, duplicate and per-arm pagination behavior as the reference.
Named paths, nullable binding counts and unproven expressions fall back.

Scalar literal predicates borrow strings and reject incompatible complex
values without cloning their JSON payloads. Numeric coercion and JSON scalar,
date-time, decimal and duration equality retain the reference rules. Required
scalar formatting is precharged; string comparison work remains budgeted.

**Directed four-cycles.** The pattern `R(c,p), H(c,u), H(p,v), K(u,v)` may use
any comma-path order, with directed R/H edges and undirected K. Distinct R/H/K
types and incompatible same-key string constraints on c/p prove relationship
independence. A required label from any node mention narrows the candidate
list; every repeated mention still contributes its predicates, and unlabeled
roles scan all vertices. Grouped
creator/reply multiplicities and adaptive intersection/binary probes preserve
nonfunctional creators and reciprocal/parallel K edges without tuple buffers.

**Symmetric location triangles.** Three separate, identical two-hop location
`MATCH` arms followed by a single undirected triangle use sparse person/country
path weights and degree-oriented adjacency intersections. Labels and types are
structural parameters, not hard-coded schema names. Location paths are not
assumed functional: their full multiplicity participates independently for each
person role. The triangle counts distinct, two-equal and all-equal vertex cases
separately, choosing physical relationships without replacement within that
`MATCH`. Asymmetric arms, combined clause scopes and unproven filters fall back.
Location-weight construction is proportional to distinct person–city–country
join terms and can grow with nonfunctional fan-out; its allocations and work
remain bounded by the active policy.

These paths return the same scalar shape, empty-match zero and final pagination
as the reference executor. A final count outside the supported nonnegative
`i64` range is an error. Unsupported shapes fall back; execution errors and
budget exhaustion do not silently retry without a budget.

## Relationship identity in the reference executor

One `MATCH` cannot reuse a physical relationship, including across comma paths;
a subsequent `MATCH` starts a new uniqueness scope. Anonymous edges, parallel
edges with identical data, and missing/duplicate application edge IDs are
distinguished by graph slots. Named fixed relationships retain that identity
through `WITH` aliases, bare-variable grouping and `WITH DISTINCT`. Computed
expressions and public result-value equality retain their existing semantics;
internal edge-slot keys never appear in result values. Optional matching still
pads a failed clause once.

Variable-length paths retain their existing node-simple traversal restriction
and now exclude relationships used earlier in the same `MATCH`. Shortest paths
retain their existing selection, then reject reused edges rather than silently
substitute longer paths. Rebinding a variable-length/shortest relationship list
is explicitly unsupported because its value does not carry slot provenance.

## Bounded use

`grust_cypher::run_bounded_read_query_indexed` applies the same
`ReadQueryPolicy` contract as `run_bounded_read_query`: mandatory bounded final
`LIMIT`, syntax/semantic checks, input/output limits, cumulative candidate work
and intermediate bytes, and a cooperative deadline. Fast-path planner work,
count arrays, masks and adjacency scans are charged. Fallback runs within the
same budget, including work already spent trying the optimized plan.

The index computes exact serialized graph size once through a counting writer;
bounded reads compare that cached size instead of serializing the graph again.
Snapshot/index construction precedes the query call and is not covered by its
deadline or intermediate budget. Hosts must separately limit loading and
resident memory, project an authorized graph, and provide hard process limits
where needed. A cooperative deadline is not an operating-system kill.

## SQL scalar counts

Turso and PostgreSQL call `ReadPushdown::scalar_count_read()` on an existing
read plan. Eligible node, fixed-segment or directed multi-pattern sources use
`SELECT COUNT(*)` over their existing SQL joins. Structural node labels are
supported. Additional filters are limited to conjunctions of genuine property
equality against string literals or string-valued parameters. Exact JSON
payload-type checks and byte-wise string equality prevent numeric/boolean
coercion. Inline `label` properties, numeric predicates and other predicate
forms are not admitted by this opt-in. String-valued special types follow the
reference's `Value::to_json` equality, rather than requiring a string enum tag.

Multiple relationship positions require disjoint type sets. Optional matches,
repeated `MATCH`, grouping, ordering and variable-length paths are not admitted.
The backend emits one nonnegative integer; Rust checks
its shape/range and applies the original alias and final pagination through the
shared projection. An empty match source emits zero.

`pushdown::plan_scalar_count_read` exposes the same structural classifier to
callers that do not already own a read plan. Callers must also check
`ScalarCountReadPushdown::supported_by` for the chosen dialect; `to_sql` returns
an error if an exact predicate cannot be rendered. The default implementation
of `SqlDialect::exact_string_property_eq` declines support, so existing dialect
implementors need not change. SQLite, Turso and PostgreSQL implement the hook;
Sail's adapter has not opted into scalar aggregation. Independently, segment and
multi-pattern row-source plans require disjoint relationship type sets: nullable
or duplicate SQL edge IDs cannot prove physical relationship uniqueness. Unsafe
overlapping-type joins fall back instead of counting the same relationship twice.
Unsupported scalar shapes retain each backend's existing fallback, including
the older row-source filter/coercion limitations. Scalar aggregation does not
change remote-backend cancellation behavior or claim those older filter paths fixed.

See the [LSQB execution-plan contract](../benchmarks/lsqb/EXECUTION-PLANS.md)
for timing boundaries, hash-bound admission and immutable historical evidence.
Passing correctness tests or selecting a fast plan does not establish a
performance improvement; that requires a separately qualified measured run.
