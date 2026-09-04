# Grust agent context

Grust is a backend-neutral Rust property-graph library: one graph,
query/mutation, schema, and traversal model over Memory, Sail/Spark, PostgreSQL,
pgGraph, PostgreSQL SQL/PGQ, Turso, SurrealDB, FalkorDB, LanceDB, and
CocoIndex. HelixDB and LadybugDB are internal `publish = false` workspace
adapters. `grust-cypher` is the portable GQL/Cypher language layer.

## Start here

- [`AGENTS.md`](AGENTS.md) contains the authoritative repository rules.
- [`FIRSTPAIR.md`](FIRSTPAIR.md) owns the source-side book contract; shared
  build and deployment behavior belongs to `~/src/firstpair`.
- [`PUBLISH.md`](PUBLISH.md), [`RELEASES.md`](RELEASES.md), and
  [`CHANGELOG.md`](CHANGELOG.md) own release operations and history.
- [`docs/GQL_PROFILE_STATEMENT.md`](docs/GQL_PROFILE_STATEMENT.md) and the
  `GqlFeature`/backend manifests own current language claims.
- [`docs/INTEGRATION.md`](docs/INTEGRATION.md) owns live-backend test usage.

Do not infer current Git, registry, build, or deployment state from this file;
inspect the repository, CI, crates.io, and FirstPair handoff directly.

## Language implementation

The original GQL/Cypher completion, Full39075 catalog, Pushdown 2, and
PostgreSQL executor goals are complete and merged. Their goal files are
historical execution records, including old branch names, test floors, and stop
points.

`Full39075` is the name of Grust's widest internal feature profile. It means the
69 supported entries in Grust's 74-entry catalog are implemented and tested;
the other five are intentional strict-write rejections. It does not mean formal
or exhaustive ISO/IEC 39075 certification, and it does not imply identical
execution on every backend. Memory defines the reference behavior; backend
descriptors and live tests distinguish pushdown, native execution, portable
fallback, and unsupported operations.

The language modules live under `crates/grust-cypher/src/`: lexer, parser,
typed AST, semantics, reference reads, read pushdown, catalog/graph-type/session
and transaction surfaces, plus the compatibility write planner and returning
executor. Preserve structured rejection and differential-oracle behavior when
extending them.

## Backend and release discipline

- Do not describe a backend as supporting a capability solely because the
  parser or Memory reference supports it. Check the concrete store and its live
  integration gate.
- `GraphMutationAtomicity::Transactional` describes one
  `apply_mutations` call. Higher-level helpers must submit the whole logical
  statement atomically before claiming statement atomicity. PostgreSQL and
  Turso reject unsupported non-returning plan lowering before writing, then run
  the operations in source order inside one isolated transaction; the generic
  write-with-`RETURN` helper remains sequential and is not a whole-statement
  atomicity boundary.
- Keep facade features aligned with `crates/grust/Cargo.toml`; do not advertise
  the internal HelixDB or LadybugDB crates as crates.io facade features.
- A named release is incomplete until affected crates, changelog, book, release
  post, provenance-stamped TextPack, tag, and external registry verification
  all agree. Follow `PUBLISH.md`; do not hand-copy book or blog artifacts around
  the centralized FirstPair machinery.

## Baseline verification

Select the smallest focused checks while developing, then run the complete
release gates from `PUBLISH.md`. Common local checks are:

```sh
cargo fmt --all -- --check
cargo test -p grust-core -p grust-cypher -p grust-memory -p grust-turso
cargo check -p grust-graph --all-features --all-targets
git diff --check
```

Live services are explicit. Use `scripts/integration-test.sh doctor` before a
profile, record exact image/source revisions, and never count an unavailable or
skipped service as a passing backend test.
