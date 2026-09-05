# Grust 0.13.2 "Krill": Correct Counts, Explicit Capabilities, Bounded Sessions

Grust is a backend-neutral property-graph library for Rust. One API describes
labeled nodes, relationships, properties, schemas, traversal, and mutation
across Memory, Turso, PostgreSQL, FalkorDB, SurrealDB, Sail, LanceDB, and other
storage and export adapters. That shared API is useful only when backend
differences remain explicit and results remain faithful. Krill strengthens
both boundaries through defects found by running the graph comparison.

See the [repository and API guide](https://github.com/querygraph/grust), the
[Grust book](https://firstpair.org/read/grust/), and the
[benchmark hub](https://adversari.al/graph/) for the larger picture. The
[backend qualification record](https://github.com/querygraph/grust/blob/main/benchmarks/lsqb/BACKENDS.md)
and [dataset guide](https://github.com/querygraph/grust/blob/main/benchmarks/lsqb/DATASETS.md)
separate supported execution paths from future benchmark work.

## A count is not necessarily a server-side aggregate

Some portable graph queries push joins into a backend but perform their final
projection in Rust. When no named binding is needed, the SQL plan selects an
integer `1` for each match. Sail 0.7.1 returns those markers as Arrow Int32,
while Grust's decoder previously required strings. LSQB q1 exposed the mismatch.

Krill accepts integer markers alongside text columns, preserving match
multiplicity, nulls, and result-batch boundaries. It does not relabel this
execution as a native aggregate: fetching rows and projecting in Rust remains
a different performance class from returning a scalar computed by the engine.

## Execute only the capabilities the backend actually has

The zero-hop adversarial case exposed another boundary: Sail 0.7.1 cannot
execute Grust's recursive walk CTE. The Spark dialect now gates that plan
before submission. Variable-length paths use the shared reference fallback,
and benchmark records identify graph materialization plus Rust execution.
Larger-scale admission can reject that path rather than silently timing an
unbounded graph download. This is not a claim of native recursive performance.

## Remote state needs explicit cleanup

`SailGraphStore::close().await` consumes a client and requests release of its
owned Spark Connect session. Temporary views and other session state are
invalidated; durable warehouse files are not deleted. Other clients sharing
the same session ID are affected, so callers must finish their operations first.
Ordinary Rust drop cannot perform this asynchronous cleanup.

Benchmark workers emit their result before releasing the session. Cleanup
belongs to bounded recovery, outside query timing; failure or a hung close
invalidates the worker completion. A release acknowledgement is not treated
as proof that a forcibly interrupted remote query has stopped.

The comparison harness also records each completed observation incrementally
and uses process-isolated hard deadlines with steady progress events. Partial
logs remain useful diagnostics, but only a complete verified publication
receipt makes a run admissible evidence. These LSQB-derived runs are not
official LDBC Benchmark Results, and no winner is inferred across incompatible
execution classes or resource configurations.

## Scoped release

Krill advances `grust-cypher`, `grust-sail`, and `grust-graph` to 0.13.2.
The facade requires the corrected optional dependencies; `grust-surreal`
remains 0.13.1 and other publishable crates remain 0.13.0.

```toml
[dependencies]
grust = { package = "grust-graph", version = "0.13.2", features = ["sail"] }
```

Live Docker qualification passed all nine baseline queries and thirteen
adversarial cases, including a rerun with explicit session cleanup. A separate
live test proves closing one session leaves another session's graph intact.
Those diagnostic outputs do not replace
the separately receipt-verified, resource-disclosed full comparison or imply
that larger-scale runs have finished.
