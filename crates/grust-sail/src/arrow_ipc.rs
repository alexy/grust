use grust_core::{GrustError, Result};

use super::SailGraphStore;

const ARROW_IPC_CHUNKS: &str = "Sail Arrow IPC chunks";
const ARROW_IPC_BYTES: &str = "Sail Arrow IPC bytes";

#[derive(Clone, Copy)]
struct ArrowIpcLimits {
    max_chunks: usize,
    max_bytes: usize,
}

struct ArrowIpcCollector {
    chunks: Vec<Vec<u8>>,
    limits: Option<ArrowIpcLimits>,
    total_bytes: usize,
}

impl ArrowIpcCollector {
    fn new(limits: Option<ArrowIpcLimits>) -> Self {
        Self {
            chunks: Vec::new(),
            limits,
            total_bytes: 0,
        }
    }

    fn push(&mut self, data: Vec<u8>) -> Result<()> {
        if let Some(limits) = self.limits {
            checked_limited_total(ARROW_IPC_CHUNKS, limits.max_chunks, self.chunks.len(), 1)?;

            let observed_bytes = checked_limited_total(
                ARROW_IPC_BYTES,
                limits.max_bytes,
                self.total_bytes,
                data.len(),
            )?;
            self.total_bytes = observed_bytes;
        }

        self.chunks.push(data);
        Ok(())
    }

    fn into_chunks(self) -> Vec<Vec<u8>> {
        self.chunks
    }
}

fn checked_limited_total(
    resource: &'static str,
    limit: usize,
    current: usize,
    added: usize,
) -> Result<usize> {
    let observed = current
        .checked_add(added)
        .ok_or(limit_error(resource, limit, usize::MAX))?;
    if observed <= limit {
        Ok(observed)
    } else {
        Err(limit_error(resource, limit, observed))
    }
}

fn limit_error(resource: &'static str, limit: usize, observed: usize) -> GrustError {
    GrustError::ResourceLimitExceeded {
        resource,
        limit,
        observed,
    }
}

impl SailGraphStore {
    /// Executes Spark SQL through Sail and returns result batches as Arrow IPC streams.
    ///
    /// Each item in the returned vector is the complete IPC stream emitted by
    /// one Spark Connect `ArrowBatch` response.
    pub async fn query_arrow_ipc(&self, sql: &str) -> Result<Vec<Vec<u8>>> {
        self.collect_arrow_ipc(sql, None).await
    }

    /// Executes Spark SQL and bounds the collected Arrow IPC response.
    ///
    /// The chunk and cumulative byte limits are inclusive. An excessive batch
    /// returns [`GrustError::ResourceLimitExceeded`] before its data is retained;
    /// accepted batch bytes move into the collection without an extra copy.
    pub async fn query_arrow_ipc_bounded(
        &self,
        sql: &str,
        max_chunks: usize,
        max_bytes: usize,
    ) -> Result<Vec<Vec<u8>>> {
        self.collect_arrow_ipc(
            sql,
            Some(ArrowIpcLimits {
                max_chunks,
                max_bytes,
            }),
        )
        .await
    }

    async fn collect_arrow_ipc(
        &self,
        sql: &str,
        limits: Option<ArrowIpcLimits>,
    ) -> Result<Vec<Vec<u8>>> {
        let mut collector = ArrowIpcCollector::new(limits);
        self.run_plan(self.query_request(sql, vec![])?, |data| {
            collector.push(data)
        })
        .await?;
        Ok(collector.into_chunks())
    }
}

#[cfg(test)]
mod tests;
