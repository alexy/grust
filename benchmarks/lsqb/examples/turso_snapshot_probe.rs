//! Diagnostic: load a projected-FK dataset into a file-backed Turso store,
//! then copy the file, open the copy and build the resident index, timing
//! each step and printing the file size. Not part of any protocol.
//!
//! `cargo run --release --example turso_snapshot_probe -- <data_dir> <scratch_dir>`

use std::path::PathBuf;
use std::time::Instant;

use grust_core::{GraphAdminStore, GraphStore};
use grust_lsqb_runner::dataset::projected_dataset_chunks;
use grust_turso::{TursoConfig, TursoGraphStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let data_dir = PathBuf::from(args.next().expect("data dir"));
    let scratch = PathBuf::from(args.next().expect("scratch dir"));
    std::fs::create_dir_all(&scratch)?;
    let original = scratch.join("snapshot.db");
    let _ = std::fs::remove_file(&original);

    let started = Instant::now();
    let store = TursoGraphStore::connect(TursoConfig {
        path: original.to_string_lossy().into_owned(),
        ..TursoConfig::default()
    })
    .await?;
    store.bootstrap().await?;
    let mut nodes = 0;
    let mut edges = 0;
    for chunk in projected_dataset_chunks(&data_dir, 10_000)? {
        let chunk = chunk?;
        nodes += chunk.nodes.len();
        edges += chunk.edges.len();
        store.put_graph(&chunk).await?;
    }
    let load = started.elapsed();
    drop(store);
    let size = std::fs::metadata(&original)?.len();
    let wal = std::fs::metadata(scratch.join("snapshot.db-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    println!(
        "load: {nodes} nodes, {edges} edges in {:.1} s; file {} bytes, wal {} bytes",
        load.as_secs_f64(),
        size,
        wal
    );

    let copy = scratch.join("worker-copy.db");
    let started = Instant::now();
    std::fs::copy(&original, &copy)?;
    println!("copy: {:.2} s", started.elapsed().as_secs_f64());

    let started = Instant::now();
    let worker = TursoGraphStore::connect(TursoConfig {
        path: copy.to_string_lossy().into_owned(),
        ..TursoConfig::default()
    })
    .await?;
    let opened = started.elapsed();
    let index = worker.indexed_snapshot().await?;
    println!(
        "open: {:.2} s; read-back + index: {:.2} s; {} nodes, {} edges in the index",
        opened.as_secs_f64(),
        started.elapsed().as_secs_f64() - opened.as_secs_f64(),
        index.graph().nodes.len(),
        index.graph().edges.len()
    );
    Ok(())
}
