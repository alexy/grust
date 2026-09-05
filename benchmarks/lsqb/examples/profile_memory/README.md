# Memory profiling diagnostic

`profile_memory` is a developer diagnostic, not a matrix benchmark or
publication result. It emits only stdout JSON lines with the distinct schema
`grust-lsqb-memory-profile-diagnostic-v1` and `publication_eligible: false`.
It creates no files, downloads nothing, and never updates frozen evidence.

The existing dataset helper fingerprints the extracted CSV bytes in a separate
read pass, then decodes bounded chunks once into the production Memory backend.
Keep the input directory unchanged during this diagnostic; it is not locked.
The backend builds and retains one immutable typed index for every iteration.
Exact dataset provenance/counts, production structural plan classification, and
the pinned count oracle are checked before accepting query results. Both
baseline and adversarial sources use the existing runner APIs. The default is
the tiny example graph, all 22 count queries, one pass; `--query q2` selects one.

Repeated query time includes the production parse/plan/execute path, runtime
scheduling, and result decoding; it is not an executor-only measurement.
There is no warmup protocol or statistical/publication summary. Every result is
oracle-checked, even when progress is only emitted periodically.

## Retained logs

Build separately, then use `command-progress.py` for this non-publication
diagnostic. Each `--output` directory must not exist; choose fresh names for
retries. These are suggested commands, not recorded measurements:

```sh
python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-profile-memory-build-01 -- \
  cargo build --release --manifest-path benchmarks/lsqb/Cargo.toml \
  --example profile_memory

python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-profile-memory-example-01 -- \
  benchmarks/lsqb/target/release/examples/profile_memory
```

To inspect an already available, pinned SF0.1 or SF0.3 dataset, select its scale;
the standard `upstream/lsqb/data/social-network-sfSCALE-projected-fk` path is used.
`--dataset-dir` selects an existing CSV directory independently from
`--lsqb-root`, which owns the pinned query/oracle files. Nothing fetches missing
data. For the repository's local data directories, one-pass examples are:

```sh
python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-profile-memory-sf01-01 -- \
  benchmarks/lsqb/target/release/examples/profile_memory \
  --scale 0.1 --dataset-dir benchmarks/lsqb/data/social-network-sf0.1-projected-fk \
  --max-seconds 600 --query-timeout-ms 120000

python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-profile-memory-sf03-01 -- \
  benchmarks/lsqb/target/release/examples/profile_memory \
  --scale 0.3 --dataset-dir benchmarks/lsqb/data/social-network-sf0.3-projected-fk \
  --max-seconds 600 --query-timeout-ms 120000
```

For a longer, finite attachment window, for example:

```sh
python3 benchmarks/lsqb/command-progress.py \
  --output /tmp/grust-profile-memory-q2-01 -- \
  benchmarks/lsqb/target/release/examples/profile_memory \
  --scale 0.1 --dataset-dir benchmarks/lsqb/data/social-network-sf0.1-projected-fk \
  --query q2 --iterations 100000 \
  --max-seconds 180 --query-timeout-ms 30000 --progress-every 10
```

After `index_ready`/`query_start`, the diagnostic JSON `pid` identifies the
profiling process. In another terminal, macOS `sample PID 10` can inspect that
process without sampling Cargo or the retained-log wrapper. The diagnostic may
finish earlier if it completes the finite iteration count.

## Deadline ownership

Exactly one owned in-process watchdog thread waits on a channel with
`recv_timeout`; it does not poll, spawn watchdog processes, or restart work.
The control channel has one queued-message slot; expired deadlines take
precedence over queued query/idle messages.
Its overall deadline includes fingerprinting/loading/index construction and
never extends. Query messages add a per-query deadline, always capped by the
overall deadline. The guard remains active through final error reporting;
every normal or error return sends stop and joins the thread.

The production Memory timeout waits for blocking work to quiesce and alone is
not a hard bound. At either deadline this diagnostic immediately exits its own
Memory-only process with status 124 using `_exit`, without writing or flushing
output and without running Rust/C cleanup. A blocked stdout pipe therefore
cannot prevent termination. The external `command-progress.py` terminal record
and exit status identify a hard timeout; there is no guaranteed timeout JSON
event, and an in-progress JSON line may be incomplete. Main-thread progress
events remain incrementally flushed during ordinary execution. There are no
services or child processes to abandon. This is a process deadline, not a new
cooperative read-budget API or the publication runner's worker protocol.
Other errors, including provenance/plan/oracle mismatches, exit 1. A requested
long repeat commonly ends at the intentional overall deadline; that is not a
successful completed pass or publication evidence.
