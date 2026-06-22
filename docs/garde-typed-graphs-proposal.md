# Garde Typed Graph Proposal

Status: historical design note. The `typed-garde` and `typed-zod-rs` ingestion
features are implemented; this document explains the original shape and should
not be read as an open proposal.

This branch proposes an optional `typed-garde` feature for Grust. The feature lets users define typed Rust structs for graph nodes and edges, validate them with `garde`, then lower the validated values into the existing dynamic `Graph` model.

The important boundary is that typed graph definitions are an ingestion layer. Backends continue to receive ordinary `Node`, `Edge`, and `Graph` values, so this does not require a backend schema migration or a new storage abstraction.

## Shape

- `TypedNode` maps a validated Rust struct into a Grust node label, node id, and property map.
- `TypedEdge` maps a validated Rust struct into an edge label, endpoints, optional edge id, and property map.
- `TypedGraphBuilder` validates each typed value before adding it to the inner `GraphBuilder`.
- The default property conversion uses `serde_json::to_value`, so most typed structs only need `Serialize`, `garde::Validate`, and a small `TypedNode` or `TypedEdge` implementation.
- Existing dynamic graph values can coexist with typed values through `from_graph`, `from_builder`, `add_raw_node`, `add_raw_edge`, and `into_builder`.
- The optional `typed-zod-rs` feature adds raw JSON ingestion helpers on top of `typed-garde`: `parse_typed_json`, `parse_typed_json_with`, `add_node_from_json`, and `add_edge_from_json`.

## Extension Model

Typed graphs can grow by adding new Rust types. A project might start with `Person`, `Project`, and `WorksOn`, then later add `Team` and `MemberOf` without changing the original types or the backend contract. Existing untyped graph documents can also be loaded first and then extended with typed additions.

See these runnable examples:

- `crates/grust/examples/typed_graph_garde.rs`: garde-only typed graph construction.
- `crates/grust/examples/typed_graph_garde_mixed.rs`: garde typed values coexisting with raw Grust nodes and edges.
- `crates/grust/examples/typed_graph_zod_garde.rs`: zod-rs JSON shape validation followed by serde decode and garde domain validation.
- `crates/grust/examples/typed_graph_zod_garde_errors.rs`: the boundary between zod-rs shape errors and garde domain errors.

## Why This Fits Grust

Grust already treats labels and properties as the backend-neutral interchange layer. A garde-backed typed layer gives application authors stronger validation at the boundary while preserving the current document formats, graph builder, traversal model, and store traits.
