# Native Neo4j comparison lane

This lane is under implementation, not published benchmark evidence. It adds
Neo4j as a native engine comparator; it does not invent a Grust Neo4j adapter.
The existing twelve-backend Grust receipts remain unchanged.

## Pinned qualification target

- Neo4j Community 2026.07.1, the current server release identified in
  [Neo4j's release notes](https://neo4j.com/release-notes/) on 2026-09-05 UTC.
- Linux ARM64 platform image:
  `neo4j:2026.07.1-community@sha256:31697c776d8c255152be39430d4b306a414c1409c91dccd093ac5e6baf2cae9d`.
- Rust driver: `neo4rs = 0.9.0-rc.10`, pinned exactly. This is a prerelease
  from [Neo4j Labs](https://github.com/neo4j-labs/neo4rs); runtime/protocol
  compatibility must be demonstrated, not inferred from the version number.

The initial `grust-lsqb-neo4j probe` command is read-only and reports server
version, edition, driver version, and scalar decoding. It uses explicit
transactions and rollback, not automatically retried convenience queries.
Configuration uses `NEO4J_URI`, `NEO4J_USER`, and `NEO4J_PASSWORD`; errors do not
echo endpoint credentials. A probe pass is not a benchmark result.

The initial live probe passed on 2026-09-05 UTC against the pinned ARM64 image:
server2026.07.1 Community, scalar42, explicit rollback acknowledged. Both
scalar-shape unit tests passed. Server inspection confirms
`db.transaction.timeout = 1m`; forced cancellation recovery is not yet qualified.

## Required completion gates

1. Reuse the Rust dataset fingerprint/oracle loaders and bounded graph chunks.
   Preserve Message/Post/Comment inheritance and relationship multiplicity.
2. Run native baseline Cypher and the thirteen adversarial cases. Disclose
   native-engine semantics and any query adaptation separately from Grust's
   portable/reference execution; never pool their timing samples.
3. Use explicit non-retrying transactions, coordinator process deadlines,
   transaction identity, and verified server quiescence after cancellation.
   Merely dropping a Rust future does not establish remote cancellation.
4. Emit flushed incremental observations, load progress, version/image/resource
   identity, and distinct setup/query/recovery timing. Require an independent
   complete receipt before the site claims published Neo4j results.
5. Qualify example and downloaded scales in an isolated owned container. Never
   clear or import into an existing user/service database.

The private harness feature is `neo4j-native`, separate from `full-backends`.
No publishable Grust crate or public API changes are required for this lane.
