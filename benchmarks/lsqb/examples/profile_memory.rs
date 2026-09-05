//! Load-once Memory profiling diagnostic, not benchmark/publication evidence.
//! See profile_memory/README.md for retained-log and macOS sampling commands.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use grust_core::Graph;
use grust_lsqb_runner::{
    backend::{PreparedBackend, QueryExecutionError},
    dataset, provenance,
    queries::{self, DatasetStats, QueryCase},
    report::ExecutionPlan,
};
use serde_json::json;

#[path = "profile_memory/support.rs"]
mod support;
use support::{Options, Progress, Watchdog};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let progress = Progress::new();
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{}", support::HELP);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            let _ = progress.emit("error", json!({"message": error}));
            return ExitCode::FAILURE;
        }
    };
    // Keep the deadline active through final error reporting too: a blocked
    // stdout sink must not outlive the same overall process bound.
    let watchdog = match Watchdog::start(options.max_seconds) {
        Ok(watchdog) => watchdog,
        Err(error) => {
            let _ = progress.emit("error", json!({"message": error}));
            return ExitCode::FAILURE;
        }
    };
    let result = run(&options, &progress, &watchdog).await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = progress.emit("error", json!({"message": error}));
            ExitCode::FAILURE
        }
    }
}

async fn run(options: &Options, progress: &Progress, watchdog: &Watchdog) -> Result<(), String> {
    // This one owned thread also bounds synchronous CSV/index preparation.
    // Its guard stops and joins when main finishes reporting success or error.
    progress.emit(
        "start",
        json!({
            "scale": options.scale,
            "query": options.query,
            "iterations": options.iterations,
            "query_timeout_ms": options.query_timeout_ms,
            "max_seconds": options.max_seconds,
            "progress_every": options.progress_every,
            "lifecycle": "load-once-reuse-immutable-index",
            "deadline": "one-owned-in-process-watchdog-exits-124",
            "timing_scope": "production-parse-plan-execute-on-retained-index",
        }),
    )?;
    let directory = options.dataset_dir.clone().unwrap_or_else(|| {
        options.lsqb_root.join(format!(
            "data/social-network-sf{}-projected-fk",
            options.scale
        ))
    });
    progress.emit("fingerprint_start", json!({}))?;
    let fingerprint = dataset::fingerprint_projected_dataset(&directory)?;
    progress.emit(
        "fingerprint_complete",
        json!({
            "sha256": fingerprint.sha256,
            "csv_files": fingerprint.csv_files,
            "csv_bytes": fingerprint.csv_bytes,
        }),
    )?;

    let (backend, stats) = prepare(&directory, options.chunk_size, progress).await?;
    // Verify exact extracted bytes and decoded counts before executing queries.
    // Fingerprinting is a separate read pass; decoding/indexing occurs once.
    let identity = provenance::lsqb_dataset_identity(&options.scale, stats, &fingerprint)?;
    progress.emit(
        "index_ready",
        json!({
            "nodes": stats.nodes,
            "edges": stats.edges,
            "person_nodes": stats.person_nodes,
            "load_ns": backend.load_ns,
            "fingerprint_and_counts_match_manifest": true,
            "archive_sha256": identity.archive_sha256,
            "extracted_manifest_sha256": identity.extracted_manifest_sha256,
        }),
    )?;

    let cases = cases(options, stats)?;
    for case in &cases {
        let plan = backend.execution_plan(case)?;
        if plan != ExecutionPlan::CountFactorized {
            return Err(format!(
                "{} has unproven/fallback plan {plan:?}; this diagnostic only profiles indexed counts",
                case.id
            ));
        }
        progress.emit(
            "query_ready",
            json!({
                "query": case.id,
                "plan": plan,
                "expected_count": case.expected_count,
                "source_sha256": case.source_sha256,
                "adapter_sha256": queries::sha256(case.executable.as_bytes()),
            }),
        )?;
    }

    // The Memory backend owns the graph/index. No duplicate source graph,
    // reload, warmup protocol, or materialized match-row oracle is introduced.
    let source = Graph::default();
    let mut completed = 0u64;
    for iteration in 1..=options.iterations {
        let report = iteration == 1
            || iteration == options.iterations
            || iteration % options.progress_every == 0;
        for case in &cases {
            watchdog.begin_query(&case.id, options.query_timeout_ms)?;
            if report {
                progress.emit(
                    "query_start",
                    json!({"query": case.id, "iteration": iteration}),
                )?;
            }
            let started = Instant::now();
            let result = backend
                .execute_count(case, &source, options.query_timeout_ms)
                .await;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            watchdog.end_query()?;
            let count = result.map_err(|error| match error {
                QueryExecutionError::Timeout(message) | QueryExecutionError::Error(message) => {
                    format!("{}: {message}", case.id)
                }
            })?;
            check_count(case, count)?;
            completed += 1;
            if report {
                progress.emit(
                    "query_complete",
                    json!({
                        "query": case.id,
                        "iteration": iteration,
                        "elapsed_ms": elapsed_ms,
                        "count": count,
                        "oracle_match": true,
                        "completed_queries": completed,
                    }),
                )?;
            }
        }
    }
    backend.finish().await?;
    progress.emit("complete", json!({"completed_queries": completed}))
}

async fn prepare(
    directory: &Path,
    chunk_size: usize,
    progress: &Progress,
) -> Result<(PreparedBackend, DatasetStats), String> {
    let mut stats = DatasetStats {
        nodes: 0,
        edges: 0,
        person_nodes: 0,
    };
    let mut chunks = 0u64;
    progress.emit("load_start", json!({"chunk_size": chunk_size}))?;
    let decoded = dataset::projected_dataset_chunks(directory, chunk_size)?.map(|chunk| {
        let graph = chunk?;
        stats.nodes = stats
            .nodes
            .checked_add(graph.nodes.len())
            .ok_or("diagnostic node count overflow")?;
        stats.edges = stats
            .edges
            .checked_add(graph.edges.len())
            .ok_or("diagnostic edge count overflow")?;
        stats.person_nodes = stats
            .person_nodes
            .checked_add(
                graph
                    .nodes
                    .iter()
                    .filter(|node| node.label.as_str() == "Person")
                    .count(),
            )
            .ok_or("diagnostic Person count overflow")?;
        chunks += 1;
        progress.emit(
            "load_chunk_decoded",
            json!({"chunks": chunks, "nodes": stats.nodes, "edges": stats.edges}),
        )?;
        Ok(graph)
    });
    let backend = PreparedBackend::prepare_projected_chunks("memory", decoded).await?;
    Ok((backend, stats))
}

fn cases(options: &Options, stats: DatasetStats) -> Result<Vec<QueryCase>, String> {
    let mut cases = queries::load_baseline_for_scale(&options.lsqb_root, &options.scale)?;
    cases.extend(queries::load_adversarial_for_scale(
        &options.attacks_dir,
        &options.lsqb_root,
        &options.scale,
        stats,
    )?);
    if let Some(selected) = &options.query {
        cases.retain(|case| case.id == *selected);
        if cases.is_empty() {
            return Err(format!("unknown pinned query {selected:?}"));
        }
    }
    Ok(cases)
}

fn check_count(case: &QueryCase, actual: i64) -> Result<(), String> {
    if actual == case.expected_count {
        Ok(())
    } else {
        Err(format!(
            "{} oracle mismatch: expected {}, got {actual}",
            case.id, case.expected_count
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_example_selection_and_count_mismatch_are_explicit() {
        let options = Options::parse(["--query".into(), "q2".into()])
            .unwrap()
            .unwrap();
        let stats = DatasetStats {
            nodes: 28,
            edges: 72,
            person_nodes: 5,
        };
        let cases = cases(&options, stats).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].expected_count, 3);
        assert!(check_count(&cases[0], 3).is_ok());
        assert!(
            check_count(&cases[0], 4)
                .unwrap_err()
                .contains("oracle mismatch")
        );
    }
}
