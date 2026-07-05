//! # querygraph-memory
//!
//! Marciana at scale: [`typesec-memory`](typesec_memory)'s `MemoryStore`
//! implemented over **any Grust [`GraphStore`] backend** — records and their
//! entity knowledge graph written *incrementally* as nodes and edges, so the
//! same vault semantics run on grust-memory (RAM), Postgres, Falkor,
//! LanceDB, or Sail.
//!
//! ## Graph shape
//!
//! ```text
//! (:MemoryRecord {record: <json>})-[:MENTIONS]->(:MemoryEntity {name, kind})
//! (:MemoryEntity)-[:RELATES {rel, fact_id}]->(:MemoryEntity)
//! ```
//!
//! The full [`StoredRecord`] rides in one JSON property; queryable dimensions
//! stay in the record and are filtered through the shared
//! [`StoreQuery::matches`] semantics (pushdown via GQL is the next
//! iteration — correctness first, the conformance suite pins it).
//!
//! ## The sync/async bridge
//!
//! `MemoryStore` is synchronous by design (FABLE-MEMORY-1 §5.1); Grust's
//! `GraphStore` is async. This crate owns the one sanctioned bridge: a
//! dedicated current-thread runtime, driven directly when no runtime is on
//! the calling thread and from a scoped thread when one is (so calling the
//! vault from inside tokio — e.g. an MCP server — cannot panic).
//!
//! ## Security posture
//!
//! This crate is *storage*: it never reads record content (the field is
//! crate-private in typesec-memory; records round-trip through serde), and
//! authorization stays where it always was — in the capability-gated vault.
//! Passing `typesec_memory::conformance` is the compatibility bar.

use std::collections::BTreeMap;
use std::future::Future;

use chrono::{DateTime, Utc};
use grust_core::prelude::{
    Direction, Edge, EdgeQuery, GraphMutation, GraphMutationStore, Node, NodeId, Start, Step,
    Traversal, Value,
};
use typesec_memory::{MemoryId, MemoryStore, StoreBatchOp, StoreError, StoreQuery, StoredRecord};

const RECORD_LABEL: &str = "MemoryRecord";
const ENTITY_LABEL: &str = "MemoryEntity";
const MENTIONS: &str = "MENTIONS";
const RELATES: &str = "RELATES";

/// `typesec-memory`'s `MemoryStore` over any Grust [`GraphMutationStore`].
pub struct GraphStoreMemoryStore<G: GraphMutationStore> {
    graph: G,
    bridge: Bridge,
}

impl<G: GraphMutationStore> GraphStoreMemoryStore<G> {
    /// Wrap a Grust backend.
    pub fn new(graph: G) -> Self {
        Self {
            graph,
            bridge: Bridge::new(),
        }
    }

    /// Borrow the underlying Grust store.
    pub fn graph(&self) -> &G {
        &self.graph
    }

    fn run<T: Send>(
        &self,
        fut: impl Future<Output = grust_core::prelude::Result<T>> + Send,
    ) -> Result<T, StoreError> {
        self.bridge
            .run(fut)
            .map_err(|err| StoreError::Backend(err.to_string()))
    }

    fn fetch(&self, id: &MemoryId) -> Result<Option<StoredRecord>, StoreError> {
        let node = self.run(self.graph.get_node(&record_node_id(id)))?;
        node.map(|node| decode_record(&node)).transpose()
    }

    /// Reachable entity node ids within `hops` RELATES steps (both directions).
    fn reachable_entities(&self, entity: &str, hops: u8) -> Result<Vec<NodeId>, StoreError> {
        let start = entity_node_id(entity);
        let mut seen: Vec<NodeId> = vec![start.clone()];
        for k in 1..=hops {
            let step = Step {
                direction: Direction::Both,
                edge: Some(RELATES.into()),
                node: None,
            };
            let traversal = Traversal {
                start: Start::Node(start.clone()),
                steps: vec![step; usize::from(k)],
                limit: None,
            };
            for node in self.run(self.graph.traverse(traversal))? {
                if !seen.contains(&node.id) {
                    seen.push(node.id);
                }
            }
        }
        Ok(seen)
    }

    /// The graph mutations that persist one record: the record node plus its
    /// entity nodes and MENTIONS edges. Shared by `put` and `apply_batch`.
    fn record_mutations(record: &StoredRecord) -> Result<Vec<GraphMutation>, StoreError> {
        let mut muts = vec![GraphMutation::UpsertNode(encode_record(record)?)];
        for entity in &record.entities {
            let mut props: BTreeMap<String, Value> = BTreeMap::new();
            props.insert("name".into(), Value::String(entity.name.clone()));
            props.insert("kind".into(), Value::String(entity.kind.clone()));
            muts.push(GraphMutation::UpsertNode(Node::new(
                ENTITY_LABEL,
                entity_node_id(&entity.name),
                props,
            )));
            muts.push(GraphMutation::UpsertEdge(Edge::new(
                MENTIONS,
                record_node_id(&record.id),
                entity_node_id(&entity.name),
                BTreeMap::new(),
            )));
        }
        Ok(muts)
    }
}

impl<G: GraphMutationStore> MemoryStore for GraphStoreMemoryStore<G> {
    fn put(&self, record: StoredRecord) -> Result<(), StoreError> {
        let muts = Self::record_mutations(&record)?;
        self.run(self.graph.apply_mutations(&muts))
    }

    fn get(&self, id: &MemoryId) -> Result<Option<StoredRecord>, StoreError> {
        self.fetch(id)
    }

    /// Apply the whole batch as one `apply_mutations` call — atomic on any
    /// backend whose `GraphMutationStore` overrides it transactionally (the
    /// consolidation path relies on this so a supersede never half-commits).
    /// An `Invalidate` reads the record, stamps `invalid_at`, and re-upserts
    /// it; that read is outside the mutation set, but the capability that
    /// authorized the consolidation is the logical lock.
    fn apply_batch(&self, ops: Vec<StoreBatchOp>) -> Result<(), StoreError> {
        let mut muts: Vec<GraphMutation> = Vec::new();
        for op in ops {
            match op {
                StoreBatchOp::Put(record) => muts.extend(Self::record_mutations(&record)?),
                StoreBatchOp::Invalidate { id, at } => {
                    let mut record = self
                        .fetch(&id)?
                        .ok_or_else(|| StoreError::Backend(format!("no record {id}")))?;
                    record.invalid_at = Some(at);
                    muts.push(GraphMutation::UpsertNode(encode_record(&record)?));
                }
            }
        }
        self.run(self.graph.apply_mutations(&muts))
    }

    fn query(&self, query: &StoreQuery) -> Result<Vec<StoredRecord>, StoreError> {
        // Pushdown: the space filter travels to the backend as a property
        // predicate (record nodes carry `space` as a plain prop), so a
        // space-scoped query never scans other tenants' records. Remaining
        // dimensions filter through the shared `StoreQuery::matches`
        // semantics the conformance suite pins.
        let start = match &query.space_id {
            Some(space) => Start::NodesByProperty {
                label: RECORD_LABEL.into(),
                key: "space".into(),
                value: Value::String(space.clone()),
            },
            None => Start::NodesByLabel(RECORD_LABEL.into()),
        };
        let nodes = self.run(self.graph.traverse(Traversal {
            start,
            steps: Vec::new(),
            limit: None,
        }))?;
        let mut out = Vec::new();
        for node in nodes {
            let record = decode_record(&node)?;
            if query.matches(&record) {
                out.push(record);
            }
        }
        // Same deterministic order as the reference stores.
        out.sort_by(|a, b| b.observed_at.cmp(&a.observed_at).then(b.id.cmp(&a.id)));
        if let Some(limit) = query.limit {
            out.truncate(limit);
        }
        Ok(out)
    }

    fn invalidate(&self, id: &MemoryId, at: DateTime<Utc>) -> Result<(), StoreError> {
        let mut record = self
            .fetch(id)?
            .ok_or_else(|| StoreError::Backend(format!("no record {id}")))?;
        record.invalid_at = Some(at);
        self.run(self.graph.put_node(&encode_record(&record)?))?;
        Ok(())
    }

    fn tombstone(&self, id: &MemoryId) -> Result<bool, StoreError> {
        let existed = self.fetch(id)?.is_some();
        if !existed {
            return Ok(false);
        }
        // delete_node removes the record and its incident MENTIONS edges;
        // RELATES edges asserted by this record are pruned by fact_id.
        self.run(self.graph.delete_node(&record_node_id(id)))?;
        let relates = self.run(self.graph.get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(RELATES.into()),
        }))?;
        for edge in relates {
            if edge.props.get("fact_id") == Some(&Value::String(id.as_str().to_string())) {
                self.run(self.graph.delete_edge(&edge.from, &edge.label, &edge.to))?;
            }
        }
        Ok(true)
    }

    fn link(&self, from: &str, rel: &str, to: &str, record: &MemoryId) -> Result<(), StoreError> {
        for name in [from, to] {
            let mut props: BTreeMap<String, Value> = BTreeMap::new();
            props.insert("name".into(), Value::String(name.to_string()));
            self.run(
                self.graph
                    .put_node(&Node::new(ENTITY_LABEL, entity_node_id(name), props)),
            )?;
        }
        let mut props: BTreeMap<String, Value> = BTreeMap::new();
        props.insert("rel".into(), Value::String(rel.to_string()));
        props.insert("fact_id".into(), Value::String(record.as_str().to_string()));
        self.run(self.graph.put_edge(&Edge::new(
            RELATES,
            entity_node_id(from),
            entity_node_id(to),
            props,
        )))?;
        Ok(())
    }

    fn neighborhood(&self, entity: &str, hops: u8) -> Result<Vec<MemoryId>, StoreError> {
        let mut out: Vec<MemoryId> = Vec::new();
        for entity_node in self.reachable_entities(entity, hops)? {
            // Records that mention this entity: one In step over MENTIONS.
            let mentions = self.run(self.graph.traverse(Traversal {
                start: Start::Node(entity_node),
                steps: vec![Step {
                    direction: Direction::In,
                    edge: Some(MENTIONS.into()),
                    node: Some(RECORD_LABEL.into()),
                }],
                limit: None,
            }))?;
            for node in mentions {
                if let Some(id) = record_id_from_node(&node.id)
                    && !out.contains(&id)
                {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }
}

/// The sync→async bridge: a dedicated current-thread runtime, safe to call
/// from inside or outside another tokio runtime.
struct Bridge {
    rt: tokio::runtime::Runtime,
}

impl Bridge {
    fn new() -> Self {
        Self {
            // No IO/time drivers enabled: the in-memory backend needs none.
            // Network backends can construct with their own runtime instead.
            rt: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("bridge runtime builds"),
        }
    }

    fn run<T: Send>(&self, fut: impl Future<Output = T> + Send) -> T {
        if tokio::runtime::Handle::try_current().is_ok() {
            // Already inside a runtime: block_on here would panic. Drive the
            // bridge runtime from a scoped thread instead.
            std::thread::scope(|scope| {
                scope
                    .spawn(|| self.rt.block_on(fut))
                    .join()
                    .expect("bridge thread completes")
            })
        } else {
            self.rt.block_on(fut)
        }
    }
}

fn record_node_id(id: &MemoryId) -> NodeId {
    NodeId::from(format!("rec:{}", id.as_str()).as_str())
}

fn record_id_from_node(node: &NodeId) -> Option<MemoryId> {
    node.as_str()
        .strip_prefix("rec:")
        .map(MemoryId::from_string)
}

fn entity_node_id(name: &str) -> NodeId {
    NodeId::from(format!("ent:{name}").as_str())
}

fn encode_record(record: &StoredRecord) -> Result<Node, StoreError> {
    let json = serde_json::to_value(record)
        .map_err(|err| StoreError::Backend(format!("record serialization failed: {err}")))?;
    let mut props: BTreeMap<String, Value> = BTreeMap::new();
    props.insert("record".into(), Value::Json(json));
    props.insert("space".into(), Value::String(record.space_id.clone()));
    Ok(Node::new(RECORD_LABEL, record_node_id(&record.id), props))
}

fn decode_record(node: &Node) -> Result<StoredRecord, StoreError> {
    match node.props.get("record") {
        Some(Value::Json(json)) => serde_json::from_value(json.clone())
            .map_err(|err| StoreError::Backend(format!("record deserialization failed: {err}"))),
        _ => Err(StoreError::Backend(format!(
            "node {} has no record payload",
            node.id.as_str()
        ))),
    }
}

#[cfg(test)]
mod tests;
