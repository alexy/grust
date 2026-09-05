//! Incrementally recorded native-engine qualification; not a publication lane yet.
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use grust_lsqb_runner::{dataset, provenance, queries, safe_output};
use neo4rs::query;
use serde_json::json;

async fn bounded_load<T>(
    budget: Duration,
    work: impl std::future::Future<Output = Result<T, &'static str>>,
) -> Result<T, &'static str> {
    tokio::time::timeout(budget, work)
        .await
        .map_err(|_| "native import deadline exceeded; stop dedicated service")?
}

fn record(file: &mut fs::File, value: &serde_json::Value) -> Result<(), &'static str> {
    let mut line = serde_json::to_vec(value).map_err(|_| "serialize native progress failed")?;
    line.push(b'\n');
    file.write_all(&line)
        .map_err(|_| "write native progress failed")?;
    file.sync_all().map_err(|_| "sync native progress failed")?;
    std::io::stdout()
        .write_all(&line)
        .map_err(|_| "stdout native progress failed")?;
    std::io::stdout()
        .flush()
        .map_err(|_| "flush native progress failed")
}

pub(super) async fn run(args: &[String]) -> Result<(), &'static str> {
    if ![4, 6].contains(&args.len()) || !["example", "0.1", "0.3"].contains(&args[2].as_str()) {
        return Err("usage: qualify LSQB_ROOT ATTACKS_DIR SCALE NEW_OUTPUT_DIR [WARMUPS RUNS]");
    }
    let sampling = super::sampling::Sampling::parse(&args[4..])?;
    let root = Path::new(&args[0]);
    let scale = &args[2];
    let directory = root.join(format!("data/social-network-sf{scale}-projected-fk"));
    let inspected =
        dataset::inspect_projected_dataset(&directory).map_err(|_| "inspect dataset failed")?;
    let stats = queries::DatasetStats {
        nodes: inspected.nodes,
        edges: inspected.edges,
        person_nodes: inspected.person_nodes,
    };
    let fingerprint = dataset::fingerprint_projected_dataset(&directory)
        .map_err(|_| "fingerprint dataset failed")?;
    let identity = provenance::lsqb_dataset_identity(scale, stats, &fingerprint)
        .map_err(|_| "dataset provenance mismatch")?;
    let baseline = queries::load_baseline_for_scale(root, scale)
        .map_err(|_| "baseline oracle verification failed")?;
    let attacks = queries::load_adversarial_for_scale(Path::new(&args[1]), root, scale, stats)
        .map_err(|_| "adversarial oracle verification failed")?;
    let mut cases = Vec::new();
    for (suite, entries) in [("baseline", baseline), ("adversarial", attacks)] {
        for mut case in entries {
            if suite == "baseline" {
                case.executable =
                    fs::read_to_string(root.join(format!("cypher/{}.cypher", case.id)))
                        .map_err(|_| "read native baseline failed")?;
                if queries::sha256(case.executable.as_bytes()) != case.source_sha256 {
                    return Err("native baseline source changed after verification");
                }
            }
            cases.push((suite, case));
        }
    }
    let output = Path::new(&args[3]);
    fs::create_dir(output).map_err(|_| "output directory must be new")?;
    let mut journal = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("observations.jsonl"))
        .map_err(|_| "create native journal failed")?;
    let graph = super::connect()?;
    let started = Instant::now();
    bounded_load(Duration::from_secs(600), async {
        let (mut nodes, mut edges) = (0, 0);
        super::loading::load(&graph, &directory, |new_nodes, new_edges| {
            nodes += new_nodes;
            edges += new_edges;
            record(
                &mut journal,
                &json!({"event":"load-progress", "nodes":nodes, "edges":edges,
            "elapsed_ms":started.elapsed().as_millis(), "complete":false}),
            )
        })
        .await?;
        if nodes != stats.nodes || edges != stats.edges {
            return Err("native import totals differ");
        }
        for (statement, expected) in [
            ("MATCH (n) RETURN count(n)", stats.nodes),
            ("MATCH ()-[r]->() RETURN count(r)", stats.edges),
        ] {
            if super::loading::scalar(&graph, query(statement), false).await? != expected as i64 {
                return Err("native database totals differ after loading");
            }
        }
        Ok(())
    })
    .await?;
    let load_ns = started.elapsed().as_nanos();
    if load_ns > 600_000_000_000 {
        return Err("native import exceeded its budget; stop dedicated service");
    }
    drop(graph);
    let mut observations = Vec::new();
    for (suite, case) in cases {
        for (phase, sample_index) in sampling.plan() {
            let mut start_event =
                json!({"event":"query-start", "suite":suite,"id":case.id,"complete":false});
            if !sampling.legacy {
                start_event["phase"] = json!(phase);
                start_event["sample_index"] = json!(sample_index);
            }
            record(&mut journal, &start_event)?;
            let (result, recovery) = super::process::run(&case.executable, 60_000).await?;
            let outcome = match result.outcome {
                grust_lsqb_runner::observation_process::WorkerOutcome::Pass
                    if result.actual_count == Some(case.expected_count) =>
                {
                    "pass"
                }
                grust_lsqb_runner::observation_process::WorkerOutcome::Pass => "mismatch",
                grust_lsqb_runner::observation_process::WorkerOutcome::Timeout => "timeout",
                grust_lsqb_runner::observation_process::WorkerOutcome::Error => "error",
            };
            let mut observation = json!({"event":"observation-recorded", "complete":false,"suite":suite,
            "id":case.id,"expected_count":case.expected_count,"actual_count":result.actual_count,
            "outcome":outcome,"elapsed_ns":result.elapsed_ns,"source_sha256":case.source_sha256,
            "query_sha256":queries::sha256(case.executable.as_bytes()),
            "timing_boundary":"coordinator-go-through-scalar-consumption-and-rollback-result",
            "setup_ns":result.setup_ns,"process_recovery_ns":result.recovery_ns,
            "termination":result.termination,"server_recovery":recovery,"query_timeout_ms":60_000});
            if !sampling.legacy {
                observation["phase"] = json!(phase);
                observation["sample_index"] = json!(sample_index);
            }
            record(&mut journal, &observation)?;
            observations.push(observation);
        }
    }
    let mut report = json!({"schema":"grust-neo4j-native-diagnostic-v1", "complete":false,
        "warning":"These are not LDBC Benchmark Results.","scale":scale,"dataset":identity,
        "driver":"neo4rs","driver_version":"0.9.0-rc.10","load_ns":load_ns,
        "observations":observations,"publication_receipt":null});
    if !sampling.legacy {
        report["schema"] = json!("grust-neo4j-native-diagnostic-v2");
        report["load_timeout_ms"] = json!(600_000);
        report["sampling"] = json!({"warmups_per_query":sampling.warmups,"measurements_per_query":sampling.runs,
            "order":"query-major-warmups-then-measurements","worker_lifecycle":"fresh-process-per-sample"});
    }
    safe_output::write_new(
        &output.join("diagnostic.json"),
        &serde_json::to_vec_pretty(&report).map_err(|_| "serialize native report failed")?,
    )
    .map_err(|_| "write native report failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_load_does_not_inherit_the_sampling_budget() {
        assert!(
            bounded_load::<()>(Duration::from_millis(1), std::future::pending())
                .await
                .is_err()
        );
        assert_eq!(
            bounded_load(Duration::from_secs(1), async { Ok(42) }).await,
            Ok(42)
        );
        assert_eq!(
            bounded_load::<()>(Duration::from_secs(1), async { Err("load failed") }).await,
            Err("load failed")
        );
    }
}
