//! Aggregate-only load telemetry. Worker stderr remains private and discarded;
//! coordinator loads expose these records in the host's incremental cell log.

use std::io::{self, Write};
use std::time::Instant;

pub(crate) struct LoadProgress {
    started: Instant,
    chunks: u64,
    nodes: u64,
    edges: u64,
}

impl LoadProgress {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            chunks: 0,
            nodes: 0,
            edges: 0,
        }
    }

    pub(crate) fn completed(&mut self, nodes: usize, edges: usize) {
        self.chunks += 1;
        self.nodes += nodes as u64;
        self.edges += edges as u64;
        // No graph values, paths, endpoints, or backend errors are serialized.
        // This is best-effort telemetry, not a query observation or receipt.
        let _ = self.write(&mut io::stderr().lock());
    }

    fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        let line = format!(
            "grust-lsqb-progress {{\"event\":\"load_chunk_complete\",\"chunks\":{},\"nodes\":{},\"edges\":{},\"elapsed_ms\":{}}}\n",
            self.chunks,
            self.nodes,
            self.edges,
            self.started.elapsed().as_millis()
        );
        writer.write_all(line.as_bytes())?;
        writer.flush()
    }
}

/// One record after a durable store's resident typed index is built: the
/// snapshot's vertex and edge counts, its exact serialized byte size, and the
/// read-back plus construction time. Aggregate figures only, like the chunk
/// records above; the build precedes READY and is never inside a query.
pub(crate) fn resident_index_built(index: &grust_core::TypedGraphIndex, started: Instant) {
    let _ = write_resident_index_built(
        &mut io::stderr().lock(),
        index.graph().nodes.len(),
        index.graph().edges.len(),
        index.serialized_graph_bytes(),
        started.elapsed().as_millis(),
    );
}

fn write_resident_index_built(
    writer: &mut impl Write,
    nodes: usize,
    edges: usize,
    serialized_graph_bytes: usize,
    elapsed_ms: u128,
) -> io::Result<()> {
    let line = format!(
        "grust-lsqb-progress {{\"event\":\"resident_index_built\",\"nodes\":{nodes},\"edges\":{edges},\"serialized_graph_bytes\":{serialized_graph_bytes},\"elapsed_ms\":{elapsed_ms}}}\n"
    );
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resident_index_telemetry_contains_only_sizes_and_elapsed_time() {
        let mut output = Vec::new();
        write_resident_index_built(&mut output, 432_235, 1_360_000, 98_000_000, 41_500).unwrap();
        assert!(output.len() < 512);
        let line = String::from_utf8(output).unwrap();
        let event: serde_json::Value =
            serde_json::from_str(line.strip_prefix("grust-lsqb-progress ").unwrap()).unwrap();
        assert_eq!(event.as_object().unwrap().len(), 5);
        assert_eq!(event["event"], "resident_index_built");
        assert_eq!(event["nodes"], 432_235);
        assert_eq!(event["edges"], 1_360_000);
        assert_eq!(event["serialized_graph_bytes"], 98_000_000);
        assert_eq!(event["elapsed_ms"], 41_500);
    }

    #[test]
    fn load_telemetry_contains_only_cumulative_counts_and_elapsed_time() {
        let progress = LoadProgress {
            started: Instant::now(),
            chunks: 3,
            nodes: 20_000,
            edges: 10_000,
        };
        let mut output = Vec::new();
        progress.write(&mut output).unwrap();
        assert!(output.len() < 512);
        let line = String::from_utf8(output).unwrap();
        assert!(line.ends_with('\n'));
        let event: serde_json::Value =
            serde_json::from_str(line.strip_prefix("grust-lsqb-progress ").unwrap()).unwrap();
        assert_eq!(event.as_object().unwrap().len(), 5);
        assert_eq!(event["event"], "load_chunk_complete");
        assert_eq!(event["chunks"], 3);
        assert_eq!(event["nodes"], 20_000);
        assert_eq!(event["edges"], 10_000);
        assert!(event["elapsed_ms"].is_u64());
    }
}
