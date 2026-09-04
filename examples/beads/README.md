# grust-beads — model an issue tracker as a grust property graph

[beads](https://github.com/gastownhall/beads) is a graph-shaped issue tracker:
issues form a directed graph connected by typed dependency edges (`blocks`,
`parent-child`, `discovered-from`, `related`). This example uses **grust as the
graph**: it loads a `bd export` JSONL stream into a `grust::Graph` and queries
it with Cypher/GQL via `grust-cypher`.

## Mapping

| beads                         | grust                                                    |
| ----------------------------- | -------------------------------------------------------- |
| issue                         | `Issue` node (`id` = issue id)                           |
| issue fields                  | node properties (title, status, priority, type, …)      |
| dependency `{issue_id, depends_on_id, type}` | edge `(issue_id)-[:<TYPE>]->(depends_on_id)` |
| dependency type               | edge label, uppercased (`parent-child` → `PARENT_CHILD`) |

Non-issue records in a full export (e.g. comments) are skipped on load.

## Run it

```sh
# uses the bundled sample-issues.jsonl
cargo run -p grust-beads

# or point it at your own beads export:
bd export > issues.jsonl
cargo run -p grust-beads -- issues.jsonl
```

Expected output for the bundled fixture:

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

## Test it

```sh
cargo test -p grust-beads
```

## Using grust-beads outside this repo

In-repo, this example depends on the workspace `grust-core` / `grust-cypher`
crates by path. To use the same code in your own project, depend on the
published crates instead:

```toml
[dependencies]
grust-core = "0.13.0"
grust-cypher = "0.13.0"
```
