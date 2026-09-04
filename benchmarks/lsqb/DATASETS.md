# Dataset ladder

The backend comparison grows through explicit dataset tiers. A larger graph is
not silently substituted for another tier: every result records the source,
scale factor, archive digest, loader transformation, and query/workload
revision that produced it.

## Immediate LSQB tier

The next comparison tier is the official projected-foreign-key LSQB dataset at
scale factor 0.1. It preserves the same labelled-subgraph workload as the
repository's tiny `sfexample` correctness fixture while making load and query
timings less dominated by process startup. Scale factor 0.3 is the explicit
adversarial strain step; it is not an automatic fallback. Downloaded tiers run
the in-process reference, backend row-source, and backend-native aggregate
classes, while whole-store materialization bridges remain unsupported. Any
comparison or ranking stays within its recorded execution class.

Downloaded Rust-producing queries have a second admission gate. The bundled
manifest records the exact maximum logical row cardinality, or a certified
upper bound, separately for the in-process and backend-row-source plans at
each scale. Only bounds at or below 1,000,000 rows execute; larger or
insufficient bounds become explicit `unsupported` outcomes without timing
samples. Backend-native scalar aggregation is exempt because those rows do not
cross into Rust.

Fetch either immutable-by-digest input with:

```sh
benchmarks/lsqb/fetch-dataset.sh --scale 0.1
benchmarks/lsqb/fetch-dataset.sh --scale 0.3
# Reuse an already downloaded object without weakening verification:
benchmarks/lsqb/fetch-dataset.sh --scale 0.3 --archive /path/to/data.tar.zst
```

The script uses objects listed by the [Graph Data Council LSQB dataset
index](https://ldbcouncil.org/data-sets-surf-repository/lsqb.html), verifies the exact byte count and
SHA-256 digest, rejects archive entries outside the expected directory, rejects
links and other special entries, and refuses to merge into or overwrite an
existing destination. On systems where `tar` cannot reliably invoke its zstd
filter (including older macOS bsdtar), the fetcher uses `zstd`/`unzstd` to
create a staged uncompressed tar before applying the same path/type checks. It
then computes the runner's filename/length/content manifest, checks the known
fingerprint, and writes a `.grust-lsqb-verified` receipt. Run scripts recompute
the manifest rather than trusting the receipt alone.

| Scale | Projected-FK archive | Archive bytes | Archive SHA-256 | Extracted CSV manifest SHA-256 |
|---|---|---:|---|---|
| 0.1 | [`social-network-sf0.1-projected-fk.tar.zst`](https://datasets.ldbcouncil.org/lsqb/social-network-sf0.1-projected-fk.tar.zst) | 6,362,514 | `20b08cfbc0b765bb066135a4c8d99367fb4f0d5c500a63b725e258dcb91b7005` | `c0d76ea897df030f901c7436d2d7ee0cd31591db54c3c6c311d79a68fa138085` |
| 0.3 | [`social-network-sf0.3-projected-fk.tar.zst`](https://datasets.ldbcouncil.org/lsqb/social-network-sf0.3-projected-fk.tar.zst) | 19,134,337 | `4aad6e31047a356d40e8c315916c3fe35a77911024136d69868b39b16f8ccf33` | `aeb94da1177ca732b127574116d7624b131113ffc7f6f8e612b0bb2dab31d5f3` |

These URLs are release-independent object names, so the recorded digest—not
the URL alone—is the dataset identity. The values above were verified against
the objects served on 2026-09-04. Keep the upstream LSQB source pinned to the
commit recorded by the harness; the [LSQB repository](https://github.com/ldbc/lsqb)
documents the projected-FK layout and remains the source for queries, loader
conventions, license, and notices.

The fetched directories live under `benchmarks/lsqb/data/`, which is ignored by
Git. They are inputs, not result artifacts. A result manifest must repeat the
selected archive SHA-256 rather than relying on a developer's local directory
name.

## Broader workload ladder

The following suites answer different questions and must remain separate
tracks. They are not interchangeable scale upgrades for LSQB.

GDC's [current published benchmark catalog](https://ldbcouncil.org/benchmarks/)
does not list a drop-in ISO GQL engine-performance suite. Any translated
workload therefore pins [ISO/IEC 39075:2024](https://www.iso.org/standard/76120.html)
and [Cor 1:2026](https://www.iso.org/standard/93701.html), then hashes the
translation separately. [OpenGQL grammar 1.9.0](https://github.com/opengql/grammar/releases/tag/1.9.0)
is useful syntax input, but it is deliberately permissive and is not a semantic
conformance kit.
The [Semantic Publishing Benchmark](https://ldbcouncil.org/benchmarks/semantic-publishing/)
targets RDF/SPARQL and is therefore not a substitute for this
property-graph/GQL comparison.

| Suite | Role in adversari.al/graph | Adoption and provenance requirements |
|---|---|---|
| [SNB Business Intelligence](https://ldbcouncil.org/benchmarks/snb/bi/) | Twenty analytical templates expand to 28 executable variants. A daily batch executes 30 bindings per variant—840 reads—plus one day of eight insert and eight delete operation types. Use it for sustained joins, aggregation, paths, updates, power, and throughput. | Pin the stable SNB 2.2.4 specification, generator, and BI v1.0.3.1 implementation. Hash any adversari.al GQL translation separately and record generator configuration, scale, factors, update stream, scoring boundary, and validation. Formal correctness starts at SF10 and audited performance at SF30+. |
| [SNB Interactive v1](https://ldbcouncil.org/benchmarks/snb/interactive/) | Mature OLTP baseline with 14 complex reads, seven short reads, and eight inserts. Its compliance gate requires 95% of operations to **start** within one second of schedule; P50/P90/P95/P99 response times are reported, not used as a P95 response-time SLA. | Pin specification, driver, generator, implementation, parameters, update stream, concurrency, recovery, and ACID evidence. SF1 is development, SF10 formal validation, and SF30+ the auditable performance range. Do not blend Interactive throughput with BI scores. |
| [SNB Interactive v2](https://github.com/ldbc/ldbc_snb_interactive_v2_impls) | Experimental successor with 14 complex reads, seven short reads, eight inserts, eight deletes, and a weighted cheapest-path Q14. | Keep this a separately labelled work-in-progress track: its driver/implementations have no release tags and published audited results remain v1-only. Record the exact component catalogs because there is no single guaranteed complete high-scale bundle. |
| [FinBench](https://ldbcouncil.org/benchmarks/finbench/) | Forty transaction templates: 12 complex reads, six simple reads, 19 writes, and three read-write operations, plus ACID checks. These anti-fraud/risk scenarios add mutation and concurrency pressure absent from LSQB. | Pin the stable v0.1 specification/data generator plus the exact driver, implementations, and ACID suite; the v0.1 driver still calls itself alpha and GDC lists no audited result. Official parameter archives exist only for SF1 and SF10. SF1 is formal validation; a conforming benchmark run uses SF10. |
| [Graphalytics](https://ldbcouncil.org/benchmarks/graphalytics/) | Whole-graph analytical kernels: BFS, PageRank, weakly connected components, community detection by label propagation, local clustering coefficient, and single-source shortest paths. This is the future algorithm/analytics track, not a property-graph query-language track. | Pin the [Graphalytics driver](https://github.com/ldbc/ldbc_graphalytics), specification version, dataset package and digest, reference outputs, algorithm parameters, platform driver, and validation output. Dataset packages are large and have had corrected reference outputs, so use the current [official dataset index](https://ldbcouncil.org/benchmarks/graphalytics/datasets/) rather than copied links. |
| [Text2GraphQuery](https://ldbcouncil.org/gql-community/text2gq/) | Natural-language graph-query accuracy, not an engine-throughput scale. The [2026-08-05 preprint](https://arxiv.org/pdf/2602.11745.pdf) reports 22,273 base examples, 267,276 pairs, 34 databases, and 13 domains across three languages and four annotations. | Upstream artifacts currently conflict: the unversioned [DataGen](https://github.com/ldbc/Text2GraphQuery-DataGen) download still advertises 178,184 pairs and its SQL/PGQ supplement covers 19,633 of 22,407 candidates. Record the exact downloaded digest, repository commits, graphs, language, model/provider, prompt/sampling settings, and evaluator; never call the moving URL an immutable full corpus. |

The selected next-workload sequence is concrete and intentionally modest:

| Track | First gate | Second gate | Why this order |
|---|---|---|---|
| SNB BI | SF1/SF3 development | SF10 validation, SF30 sustained performance, then larger tiers | Scale factors describe logical reference serialization, not archive bytes. Current public full-CSV downloads reach SF3000 merged-FK and SF10000 projected-FK; defined tiers and published audited evidence reach SF30000. Preserve the full 28-variant/840-read daily batch and update stream. |
| SNB Interactive v1 | SF1 development | SF10 formal validation, then SF30+ performance | Preserve the official mix and 95%-on-time scheduling rule. Normal v1 data/parameters reach SF1000; a special complete Parquet snapshot, update stream, and parameter set exists at SF3000. |
| SNB Interactive v2 | SF1/SF10 experimental | SF100/SF300 cheapest-path and deep-delete strain | Keep it non-auditable/WIP. Defined tiers reach SF30000 and the update index reaches SF10000, but no single official complete bundle is promised across all components at those ceilings. |
| FinBench | SF0.1 loader/correctness | SF1 validation, then SF10 complete performance and ACID | Stable v0.1 data ends at SF10 and only SF1/SF10 have published parameter archives. SF3 data can be a derived strain run, but not an official-parameter benchmark run. Graph statistics grow from 64,485 vertices / 610,658 edges at SF0.1 to 6,069,955 / 51,889,416 at SF10. |
| Graphalytics | `wiki-Talk`, then weighted `datagen-7_5-fb` for all six kernels | `graph500-22`, larger Graph500 tiers, `com-friendster`, and `datagen-sf10k-fb` | Keep algorithms separate from GQL. `wiki-Talk`, Graph500, and `com-friendster` omit SSSP; a weighted Datagen graph is required for the full six-kernel suite. `com-friendster` has 65,608,366 vertices / 1,806,067,135 edges. Large targets include `datagen-sf10k-fb` at 100,218,750 / 9,404,822,538 and `graph500-30` at 447,797,986 / 17,022,117,362. |
| Text2GraphQuery | Digest-pinned validation sample | Digest-pinned complete downloaded artifact | Treat the paper's 267,276-pair corpus and the currently served 178,184-pair artifact as distinct identities until upstream reconciles them. This remains language/model accuracy, not database performance. |

The first-rung objects below were downloaded from the links in GDC's current
dataset catalog and hashed on 2026-09-04. They are staged inputs for future
track-specific loaders, not evidence that Grust has run those complete
benchmarks yet.

| Candidate object | Exact bytes | SHA-256 | Contents relevant to admission |
|---|---:|---|---|
| [SNB BI SF1 composite merged-FK](https://datasets.ldbcouncil.org/bi-pre-audit/bi-sf1-composite-merged-fk.tar.zst) | 216,780,094 | `a72938e244e6aa9d99632fcd5065e50c669ecf4d00f60bd162b266df4a7aba13` | 804 regular archive members covering the initial snapshot and insert/delete microbatches |
| [FinBench SF0.1](https://datasets.ldbcouncil.org/finbench/sf0.1.tar.gz) | 66,710,298 | `f0359b5c4515cd5d86349b4a11a7470f6f153e42c5ac21c59e70f5c0d0b37a60` | 119 regular files and 200,538,363 uncompressed member bytes, including incremental transaction inputs; loader/correctness admission only because the official parameter catalog starts at SF1 |
| [Graphalytics wiki-Talk](https://datasets.ldbcouncil.org/graphalytics/wiki-Talk.tar.zst) | 36,637,436 | `a575c04743a3511b917a7d1cbd9357b79c04b5262b3d6241ac38ca4d82e3ca60` | 2,394,385 vertices, 5,021,410 directed edges, parameters and reference outputs for five algorithms |
| [Graphalytics graph500-22](https://datasets.ldbcouncil.org/graphalytics/graph500-22.tar.zst) | 212,233,822 | `d88e1cb7ca83a2348a9a90d6796991c0a780ed1e3882bd85e36c50f3346ce074` | 2,396,657 vertices, 64,155,735 undirected edges, parameters and reference outputs for five algorithms |

Adoption also pins executable workload sources, not only data. The candidate
snapshot currently resolves to:

| Artifact | Pin |
|---|---|
| SNB specification | `ldbc_snb_docs` v2.2.4, `5f7956e07a214373c363b371a3b88bc83ddcd118` |
| SNB Spark data generator | v0.5.1, `2459f4e45834c78902a50511fc64a05c48dd4029` |
| SNB BI driver/reference implementations | v1.0.3.1, `c2a48a6e71485222f03adac5ca7d46a6584f0ac4` |
| SNB Interactive v1 driver / Hadoop generator / v1 implementations | v1.0.0 `ead4f13b86055df405bb9b1ab0bfcf1c3ae962f5` / v1.0.0 `37d35f40f5023fcf1afd3b6d0984f71c202f4bca` / 1.0.0 `f9c394a92cd55e535893f6c9907b141d6533c817` |
| FinBench v0.1.0 specification / generator / driver / implementations / ACID | `d3ec7036bf6919df8cd3eeaa3a986048e779ea02` / `eddcc0551861eaefeb9b37497b10de1bb0f52672` / `27e5640f47e91c783112ca654f670d15863780a6` / `c488318b66dbdbd7e366e6bb62e2857c2354e271` / `3a9c4d8b0dc2bbde26ac31d2f4c5709b4f0e9fcf` |
| Graphalytics driver | v1.10.0, `90b01f311c5c5518494dd7ec4d93f7b857776d34` |
| Text2GraphQuery data generator / driver | `7cd94dfa39102bf97753177e52ad6181a1f1e373` / `303f0737cb1ac04d83bc19dc4cada9c69390d3b7` |

Moving any of these pins creates a new experiment identity and requires fresh
semantic validation and result evidence.

Licenses differ. The Text2GraphQuery DataGen repository is Apache-2.0, while
its Driver currently declares no license; never infer permission from the
organization or a sibling repository. Tool licenses also do not automatically
grant the same license for every real-world Graphalytics source dataset. Retain the
dataset's original source citation and terms, and do not mirror it until those
terms have been checked. Derived documentation and claims also retain GDC
attribution, consistent with its CC BY 4.0 fair-use guidance.

Start each new track with semantic cross-validation and a small reproducible
fixture. Add performance claims only after the loaders, result oracle, warmup,
repetition policy, resource allocation, and timing boundary are identical or
explicitly normalized across the participating backends.

## Fair-use and result labels

The Grust and adversari.al runs are independent, unaudited engineering results.
They are not “LDBC Benchmark Results.” The [GDC fair-use
policy](https://ldbcouncil.org/benchmarks/fair-use-policies/) reserves that label
for successfully audited result sets and asks derived work to explain how it
differs from the standard. Every derived or partial track here must therefore:

- state that its numbers are not LDBC Benchmark Results;
- distinguish unchanged upstream inputs from Grust/adversari.al adaptations;
- publish exact source commits, dataset digests, query transforms, backend and
  container versions, hardware/resource limits, warmups, repetitions, and
  timing boundaries; and
- report unsupported cases as capability gaps, not zero-time successes or
  silently omitted workload results.

LSQB itself is described by its maintainers as a lightweight subgraph-query
microbenchmark and not an official LDBC benchmark. SNB BI, FinBench, and
Graphalytics each have their own completeness, validation, disclosure, and
scoring rules; running a subset for development does not confer the official
benchmark result label. The Text2GraphQuery charter targets query-generation
accuracy and explicitly does not rank the performance of engines used to
execute generated queries.
