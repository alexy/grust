# Modeling beads as a grust property graph

[beads](https://github.com/gastownhall/beads) is a Dolt-powered, graph-shaped
issue tracker: issues form a directed graph connected by typed dependency edges
(`blocks`, `parent-child`, `discovered-from`, `related`). That shape maps
directly onto grust's property-graph model, so beads is a natural worked example
of **grust as the graph** for an existing application's data.

The example lives at [`examples/beads`](../examples/beads). It loads a beads
`bd export` JSONL stream into a `grust::Graph` and queries it with Cypher/GQL via
`grust-cypher`.

## The mapping

| beads                                          | grust                                                    |
| ---------------------------------------------- | -------------------------------------------------------- |
| issue                                          | `Issue` node (`id` = issue id)                           |
| issue fields (title, status, priority, type …) | node properties                                          |
| `labels: [..]`                                 | `StringArray` property                                   |
| dependency `{issue_id, depends_on_id, type}`   | edge `(issue_id)-[:<TYPE>]->(depends_on_id)`             |
| dependency `type`                              | edge label, uppercased (`parent-child` → `PARENT_CHILD`) |

Non-issue records in a full export (e.g. comments, which carry `_type` other than
`"issue"`) are skipped on load.

### Why this is a good fit

- **Issues are nodes, dependencies are edges** — no impedance mismatch. beads
  already thinks in graph terms, so the translation is mechanical rather than a
  relational-to-graph re-modeling.
- **Dependency *types* become edge labels**, so traversals are expressed in the
  query language directly: "what blocks this?" is a `[:BLOCKS]` pattern, not a
  filter on a `type` column.
- **Heterogeneous issue fields** (optional owner/assignee, label arrays,
  timestamps) land cleanly as node properties on grust's `Value` type.

## The export format

`bd export` emits JSONL — one record per line. Issue records look like:

```json
{"_type":"issue","id":"bd-2","title":"Implement the loader","status":"in_progress",
 "priority":2,"issue_type":"feature",
 "dependencies":[{"issue_id":"bd-2","depends_on_id":"bd-1","type":"blocks"}]}
```

Each issue carries its outgoing dependencies inline in a `dependencies` array,
which is exactly what the loader walks to produce edges.

## What the example does

`src/lib.rs` provides the reusable mapping:

- `BeadIssue` / `BeadDependency` — serde structs over the export schema (unknown
  fields ignored; missing fields defaulted).
- `edge_label(dep_type)` — normalizes a dependency type to a Cypher-style label.
- `build_graph(&[BeadIssue]) -> Graph` — issues → nodes, dependencies → typed
  edges.
- `parse_jsonl` / `load_jsonl` — stream a JSONL export into issue records / a
  `Graph`, skipping blank lines and non-issue records.

`src/main.rs` is a small CLI that loads a graph and reports on it, including two
grust Cypher queries — a status histogram and a `[:BLOCKS]` traversal:

```sh
# uses the bundled sample-issues.jsonl
cargo run -p grust-beads

# or point it at your own beads export:
bd export > issues.jsonl
cargo run -p grust-beads -- issues.jsonl
```

```
beads graph from .../sample-issues.jsonl:
  issues (nodes):       4
  dependencies (edges): 3
  edges by type:
    BLOCKS: 1
    DISCOVERED_FROM: 1
    PARENT_CHILD: 1
  issues by status (grust Cypher):
    open     2
    closed   1
    in_progress      1
  blocking dependencies (grust Cypher):
    bd-2 -> bd-1
```

The Cypher itself is plain GQL-style read query text run through the grust read
executor:

```cypher
MATCH (n:Issue) RETURN n.status AS status, count(*) AS count ORDER BY count DESC
MATCH (a:Issue)-[:BLOCKS]->(b:Issue) RETURN a.id AS issue, b.id AS depends_on
```

## Dependencies

As an in-repo example, `examples/beads` depends on the workspace `grust-core`
and `grust-cypher` crates by path, so it dogfoods the local tree. To reuse the
same code outside this repository, depend on the published crates instead:

```toml
[dependencies]
grust-core = "0.13.0"
grust-cypher = "0.13.0"
```

## Tests

`tests/load.rs` covers label normalization, typed-edge construction, non-issue
skipping, and an end-to-end Cypher query over a built graph:

```sh
cargo test -p grust-beads
```
