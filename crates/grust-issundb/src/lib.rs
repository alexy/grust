//! IssunDB [`GraphStore`] backend for Grust.
//!
//! [IssunDB](https://crates.io/crates/issundb) is an embedded, file-backed
//! property graph database. This crate adapts it to Grust's storage traits so
//! it can be selected as a backend engine.
//!
//! # Identity mapping
//!
//! Grust addresses nodes by a stable, application-supplied string
//! [`NodeId`], when IssunDB assigns opaque `u64` ids on insert. This backend
//! persists the Grust id as the `id` property on each IssunDB node and
//! maintains an in-memory `string id -> u64` index (rebuilt from storage when
//! the database is opened) to translate between the two.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Mutex, RwLock},
};

use async_trait::async_trait;
use grust_core::prelude::*;
use issundb::{Graph as IssunGraph, PropValue, TypeId};

/// Default memory-map size (in gigabytes) used by [`IssunGraphStore::open`].
pub const DEFAULT_MAP_SIZE_GB: usize = 1;

/// Configuration for opening an [`IssunGraphStore`].
#[derive(Clone, Debug)]
pub struct IssunConfig {
    /// Directory in which IssunDB stores its database files.
    pub path: PathBuf,
    /// Upper bound of the IssunDB memory map, in gigabytes.
    pub map_size_gb: usize,
}

impl IssunConfig {
    /// Configuration rooted at `path` with the default map size.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            map_size_gb: DEFAULT_MAP_SIZE_GB,
        }
    }
}

/// A Grust [`GraphStore`] backed by an embedded IssunDB database.
pub struct IssunGraphStore {
    graph: IssunGraph,
    /// Serializes writes so storage mutations and index updates stay in sync.
    write_lock: Mutex<()>,
    /// Maps Grust string node ids to IssunDB `u64` node ids.
    index: RwLock<HashMap<NodeId, u64>>,
    schema: RwLock<Option<GraphSchema>>,
}

impl std::fmt::Debug for IssunGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssunGraphStore")
            .field("indexed_nodes", &self.index.read().map(|i| i.len()).ok())
            .finish_non_exhaustive()
    }
}

impl IssunGraphStore {
    /// Open (or create) an IssunDB database using the supplied configuration.
    pub fn new(config: IssunConfig) -> Result<Self> {
        let graph = IssunGraph::open(&config.path, config.map_size_gb).map_err(issun_err)?;
        let store = Self {
            graph,
            write_lock: Mutex::new(()),
            index: RwLock::new(HashMap::new()),
            schema: RwLock::new(None),
        };
        store.rebuild_index()?;
        Ok(store)
    }

    /// Open (or create) an IssunDB database in `path` with the default map size.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        Self::new(IssunConfig::new(path))
    }

    /// Borrow the underlying IssunDB handle for engine-specific operations
    /// (Cypher queries, vector/text search, etc.).
    pub fn graph(&self) -> &IssunGraph {
        &self.graph
    }

    /// Rebuild IssunDB's CSR adjacency snapshot. Call after large bulk loads to
    /// refresh the snapshot used by Cypher execution.
    pub fn rebuild_csr(&self) -> Result<()> {
        self.graph.rebuild_csr().map_err(issun_err)
    }

    /// Repopulate the `string id -> u64` index from persisted nodes.
    fn rebuild_index(&self) -> Result<()> {
        let mut index = self.index.write().expect("issundb index lock poisoned");
        index.clear();
        for uid in self.graph.all_nodes().map_err(issun_err)? {
            if let Some(node) = read_node(&self.graph, uid)? {
                index.insert(node.id, uid);
            }
        }
        Ok(())
    }

    fn resolve(&self, id: &NodeId) -> Option<u64> {
        self.index
            .read()
            .expect("issundb index lock poisoned")
            .get(id)
            .copied()
    }
}

#[async_trait]
impl GraphStore for IssunGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        *self.schema.write().expect("issundb schema lock poisoned") = Some(schema.clone());
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        if let Some(schema) = self
            .schema
            .read()
            .expect("issundb schema lock poisoned")
            .as_ref()
        {
            schema.validate_node(node)?;
        }

        let _guard = self.write_lock.lock().expect("issundb write lock poisoned");
        let json = node_props_json(&node.id, &node.props);
        if let Some(uid) = self.resolve(&node.id) {
            self.graph.update_node(uid, &json).map_err(issun_err)?;
            Ok(PutOutcome::Updated)
        } else {
            let uid = self
                .graph
                .add_node(node.label.as_str(), &json)
                .map_err(issun_err)?;
            self.index
                .write()
                .expect("issundb index lock poisoned")
                .insert(node.id.clone(), uid);
            Ok(PutOutcome::Inserted)
        }
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        if let Some(schema) = self
            .schema
            .read()
            .expect("issundb schema lock poisoned")
            .as_ref()
        {
            let mut endpoint_labels: HashMap<NodeId, Label> = HashMap::new();
            for id in [&edge.from, &edge.to] {
                if let Some(uid) = self.resolve(id)
                    && let Some(node) = read_node(&self.graph, uid)?
                {
                    endpoint_labels.insert(id.clone(), node.label);
                }
            }
            schema.validate_edge_with(edge, |id| endpoint_labels.get(id))?;
        }

        let _guard = self.write_lock.lock().expect("issundb write lock poisoned");
        let from_uid = self
            .resolve(&edge.from)
            .ok_or_else(|| GrustError::Backend(format!("unknown source node '{}'", edge.from)))?;
        let to_uid = self
            .resolve(&edge.to)
            .ok_or_else(|| GrustError::Backend(format!("unknown target node '{}'", edge.to)))?;
        let json = edge_props_json(&edge.props);

        let existing = self
            .graph
            .out_neighbors(from_uid)
            .map_err(issun_err)?
            .into_iter()
            .find(|n| n.node == to_uid && self.type_matches(n.edge_type, &edge.label));

        match existing {
            Some(neighbor) => {
                self.graph
                    .update_edge(neighbor.edge, &json)
                    .map_err(issun_err)?;
                Ok(PutOutcome::Updated)
            }
            None => {
                self.graph
                    .add_edge(from_uid, to_uid, edge.label.as_str(), &json)
                    .map_err(issun_err)?;
                Ok(PutOutcome::Inserted)
            }
        }
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        if let Some(schema) = self
            .schema
            .read()
            .expect("issundb schema lock poisoned")
            .as_ref()
        {
            schema.validate_graph(graph)?;
        }
        let mut report = LoadReport::default();
        for node in &graph.nodes {
            self.put_node(node).await?;
            report.nodes += 1;
        }
        for edge in &graph.edges {
            self.put_edge(edge).await?;
            report.edges += 1;
        }
        self.graph.rebuild_csr().map_err(issun_err)?;
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        match self.resolve(id) {
            Some(uid) => read_node(&self.graph, uid),
            None => Ok(None),
        }
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        // (src_uid, dst_uid, edge_uid, edge_type)
        let mut raw: Vec<(u64, u64, u64, TypeId)> = Vec::new();
        if let Some(from) = &query.from {
            if let Some(uid) = self.resolve(from) {
                for n in self.graph.out_neighbors(uid).map_err(issun_err)? {
                    raw.push((uid, n.node, n.edge, n.edge_type));
                }
            }
        } else if let Some(to) = &query.to {
            if let Some(uid) = self.resolve(to) {
                for n in self.graph.in_neighbors(uid).map_err(issun_err)? {
                    raw.push((n.node, uid, n.edge, n.edge_type));
                }
            }
        } else if let Some(label) = &query.label {
            for eid in self
                .graph
                .edges_by_type(label.as_str())
                .map_err(issun_err)?
            {
                if let Some(record) = self.graph.get_edge(eid).map_err(issun_err)? {
                    raw.push((record.src, record.dst, eid, record.edge_type));
                }
            }
        } else {
            for uid in self.graph.all_nodes().map_err(issun_err)? {
                for n in self.graph.out_neighbors(uid).map_err(issun_err)? {
                    raw.push((uid, n.node, n.edge, n.edge_type));
                }
            }
        }

        let mut edges = Vec::new();
        for (src_uid, dst_uid, edge_uid, type_id) in raw {
            let label = match self.graph.type_name(type_id).map_err(issun_err)? {
                Some(name) => Label::new(name),
                None => continue,
            };
            if query.label.as_ref().is_some_and(|want| want != &label) {
                continue;
            }
            let from = node_string_id(&self.graph, src_uid)?;
            let to = node_string_id(&self.graph, dst_uid)?;
            if query.from.as_ref().is_some_and(|want| want != &from) {
                continue;
            }
            if query.to.as_ref().is_some_and(|want| want != &to) {
                continue;
            }
            let props = match self.graph.get_edge(edge_uid).map_err(issun_err)? {
                Some(record) => decode_props(&record.props)?,
                None => continue,
            };
            edges.push(Edge {
                id: Some(EdgeId::new(edge_uid.to_string())),
                from,
                to,
                label,
                props,
            });
        }
        Ok(edges)
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let mut current: Vec<u64> = match &traversal.start {
            Start::Node(id) => self.resolve(id).into_iter().collect(),
            Start::NodesByLabel(label) => self
                .graph
                .nodes_by_label(label.as_str())
                .map_err(issun_err)?,
            Start::NodesByProperty { label, key, value } => self
                .graph
                .nodes_by_property(label.as_str(), key.as_str(), prop_value(value)?)
                .map_err(issun_err)?,
        };

        for step in &traversal.steps {
            let mut next = Vec::new();
            for &uid in &current {
                let neighbors = match step.direction {
                    Direction::Out => self.graph.out_neighbors(uid).map_err(issun_err)?,
                    Direction::In => self.graph.in_neighbors(uid).map_err(issun_err)?,
                    Direction::Both => {
                        let mut combined = self.graph.out_neighbors(uid).map_err(issun_err)?;
                        combined.extend(self.graph.in_neighbors(uid).map_err(issun_err)?);
                        combined
                    }
                };
                for neighbor in neighbors {
                    if let Some(edge_label) = &step.edge
                        && !self.type_matches(neighbor.edge_type, edge_label)
                    {
                        continue;
                    }
                    if let Some(node_label) = &step.node {
                        let labels = self.graph.node_labels(neighbor.node).map_err(issun_err)?;
                        if !labels.iter().any(|l| l.as_str() == node_label.as_str()) {
                            continue;
                        }
                    }
                    next.push(neighbor.node);
                }
            }
            current = next;
        }

        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }

        let mut nodes = Vec::new();
        for uid in current {
            if let Some(node) = read_node(&self.graph, uid)? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }
}

#[async_trait]
impl GraphAdminStore for IssunGraphStore {
    async fn clear(&self) -> Result<()> {
        let _guard = self.write_lock.lock().expect("issundb write lock poisoned");
        for uid in self.graph.all_nodes().map_err(issun_err)? {
            self.graph.delete_node(uid).map_err(issun_err)?;
        }
        self.index
            .write()
            .expect("issundb index lock poisoned")
            .clear();
        Ok(())
    }
}

#[async_trait]
impl GraphMutationStore for IssunGraphStore {
    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        let _guard = self.write_lock.lock().expect("issundb write lock poisoned");
        let uid = self
            .index
            .write()
            .expect("issundb index lock poisoned")
            .remove(id);
        if let Some(uid) = uid {
            self.graph.delete_node(uid).map_err(issun_err)?;
        }
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        let _guard = self.write_lock.lock().expect("issundb write lock poisoned");
        let (Some(from_uid), Some(to_uid)) = (self.resolve(from), self.resolve(to)) else {
            return Ok(());
        };
        if let Some(neighbor) = self
            .graph
            .out_neighbors(from_uid)
            .map_err(issun_err)?
            .into_iter()
            .find(|n| n.node == to_uid && self.type_matches(n.edge_type, label))
        {
            self.graph.delete_edge(neighbor.edge).map_err(issun_err)?;
        }
        Ok(())
    }
}

impl IssunGraphStore {
    fn type_matches(&self, type_id: TypeId, label: &Label) -> bool {
        self.graph
            .type_name(type_id)
            .ok()
            .flatten()
            .is_some_and(|name| name == label.as_str())
    }
}

fn issun_err(err: issundb::Error) -> GrustError {
    GrustError::Backend(format!("IssunDB error: {err}"))
}

/// Read an IssunDB node into a Grust [`Node`], recovering the string id from
/// the persisted `id` property (falling back to the IssunDB `u64`).
fn read_node(graph: &IssunGraph, uid: u64) -> Result<Option<Node>> {
    let record = match graph.get_node(uid).map_err(issun_err)? {
        Some(record) => record,
        None => return Ok(None),
    };
    let label = graph
        .node_labels(uid)
        .map_err(issun_err)?
        .into_iter()
        .next()
        .unwrap_or_default();
    let mut props = decode_props(&record.props)?;
    let id = props
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| uid.to_string());
    props
        .entry("id".to_string())
        .or_insert_with(|| Value::from(id.as_str()));
    Ok(Some(Node {
        id: NodeId::new(id),
        label: Label::new(label),
        props,
    }))
}

fn node_string_id(graph: &IssunGraph, uid: u64) -> Result<NodeId> {
    Ok(read_node(graph, uid)?
        .map(|node| node.id)
        .unwrap_or_else(|| NodeId::new(uid.to_string())))
}

fn node_props_json(id: &NodeId, props: &Props) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in props {
        map.insert(key.clone(), value.to_json());
    }
    map.entry("id".to_string())
        .or_insert_with(|| serde_json::Value::from(id.as_str()));
    serde_json::Value::Object(map)
}

fn edge_props_json(props: &Props) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in props {
        map.insert(key.clone(), value.to_json());
    }
    serde_json::Value::Object(map)
}

fn decode_props(bytes: &[u8]) -> Result<Props> {
    let value: serde_json::Value = rmp_serde::from_slice(bytes).map_err(|err| {
        GrustError::Serialization(format!("issundb property decode error: {err}"))
    })?;
    Ok(json_to_props(value))
}

fn json_to_props(value: serde_json::Value) -> Props {
    match value {
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(key, value)| (key, Value::from_json(value)))
            .collect(),
        _ => Props::new(),
    }
}

fn prop_value(value: &Value) -> Result<PropValue> {
    match value {
        Value::Bool(v) => Ok(PropValue::Bool(*v)),
        Value::Int(v) => Ok(PropValue::Int(*v)),
        Value::Float(v) => Ok(PropValue::Float(*v)),
        Value::String(v) => Ok(PropValue::Str(v.clone())),
        other => Err(GrustError::Unsupported(format!(
            "issundb cannot match on property value {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests;
