# Garde Typed Graph Proposal

This branch proposes an optional `typed-garde` feature for Grust. The feature lets users define typed Rust structs for graph nodes and edges, validate them with `garde`, then lower the validated values into the existing dynamic `Graph` model.

The important boundary is that typed graph definitions are an ingestion layer. Backends continue to receive ordinary `Node`, `Edge`, and `Graph` values, so this does not require a backend schema migration or a new storage abstraction.

## Shape

- `TypedNode` maps a validated Rust struct into a Grust node label, node id, and property map.
- `TypedEdge` maps a validated Rust struct into an edge label, endpoints, optional edge id, and property map.
- `TypedGraphBuilder` validates each typed value before adding it to the inner `GraphBuilder`.
- The default property conversion uses `serde_json::to_value`, so most typed structs only need `Serialize`, `garde::Validate`, and a small `TypedNode` or `TypedEdge` implementation.
- Existing dynamic graph values can coexist with typed values through `from_graph`, `from_builder`, `add_raw_node`, `add_raw_edge`, and `into_builder`.

## Extension Model

Typed graphs can grow by adding new Rust types. A project might start with `Person`, `Project`, and `WorksOn`, then later add `Team` and `MemberOf` without changing the original types or the backend contract. Existing untyped graph documents can also be loaded first and then extended with typed additions.

See `crates/grust/examples/typed_graph_garde.rs` for a runnable example of that flow.

## Why This Fits Grust

Grust already treats labels and properties as the backend-neutral interchange layer. A garde-backed typed layer gives application authors stronger validation at the boundary while preserving the current document formats, graph builder, traversal model, and store traits.
