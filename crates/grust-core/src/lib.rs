use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, GrustError>;
pub type Props = BTreeMap<String, Value>;

#[derive(Debug, thiserror::Error)]
pub enum GrustError {
    #[error("backend error: {0}")]
    Backend(String),
    #[error("schema error: {0}")]
    Schema(String),
    #[error("unsupported graph feature: {0}")]
    Unsupported(String),
    #[error("serialization error: {0}")]
    Serialization(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct NodeId(String);

impl NodeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for NodeId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for NodeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for NodeId {
    fn from(value: &String) -> Self {
        Self::new(value.clone())
    }
}

impl From<&NodeId> for NodeId {
    fn from(value: &NodeId) -> Self {
        value.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct EdgeId(String);

impl EdgeId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for EdgeId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for EdgeId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for EdgeId {
    fn from(value: &String) -> Self {
        Self::new(value.clone())
    }
}

impl From<&EdgeId> for EdgeId {
    fn from(value: &EdgeId) -> Self {
        value.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct Label(String);

impl Label {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for Label {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Label {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<&String> for Label {
    fn from(value: &String) -> Self {
        Self::new(value.clone())
    }
}

impl From<&Label> for Label {
    fn from(value: &Label) -> Self {
        value.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringArray(Vec<String>),
    Json(serde_json::Value),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_string_array(&self) -> Option<&[String]> {
        match self {
            Self::StringArray(values) => Some(values),
            _ => None,
        }
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::String(value.clone())
    }
}

impl From<Vec<String>> for Value {
    fn from(value: Vec<String>) -> Self {
        Self::StringArray(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<usize> for Value {
    fn from(value: usize) -> Self {
        Self::Int(value as i64)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<serde_json::Value> for Value {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(value) => Self::Bool(value),
            serde_json::Value::Number(value) => {
                if let Some(value) = value.as_i64() {
                    Self::Int(value)
                } else if let Some(value) = value.as_f64() {
                    Self::Float(value)
                } else {
                    Self::Json(serde_json::Value::Number(value))
                }
            }
            serde_json::Value::String(value) => Self::String(value),
            serde_json::Value::Array(values) => {
                let strings = values
                    .iter()
                    .filter_map(|value| value.as_str().map(ToString::to_string))
                    .collect::<Vec<_>>();
                if strings.len() == values.len() {
                    Self::StringArray(strings)
                } else {
                    Self::Json(serde_json::Value::Array(values))
                }
            }
            serde_json::Value::Object(value) => Self::Json(serde_json::Value::Object(value)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub label: Label,
    pub props: Props,
}

impl Node {
    pub fn new(label: impl Into<Label>, id: impl Into<NodeId>, props: impl Into<Props>) -> Self {
        let id = id.into();
        let mut props = props.into();
        props
            .entry("id".to_string())
            .or_insert_with(|| Value::from(id.as_str()));
        Self {
            id,
            label: label.into(),
            props,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: Option<EdgeId>,
    pub from: NodeId,
    pub to: NodeId,
    pub label: Label,
    pub props: Props,
}

impl Edge {
    pub fn new(
        label: impl Into<Label>,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
        props: impl Into<Props>,
    ) -> Self {
        Self {
            id: None,
            from: from.into(),
            to: to.into(),
            label: label.into(),
            props: props.into(),
        }
    }

    pub fn with_id(mut self, id: impl Into<EdgeId>) -> Self {
        self.id = Some(id.into());
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    pub fn new(nodes: Vec<Node>, edges: Vec<Edge>) -> Self {
        Self { nodes, edges }
    }

    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EdgePolicy {
    AllowDuplicates,
    DedupeByFromLabelTo,
}

impl Default for EdgePolicy {
    fn default() -> Self {
        Self::DedupeByFromLabelTo
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphBuilder {
    nodes: BTreeMap<NodeId, Node>,
    edges: Vec<Edge>,
    edge_keys: BTreeSet<(NodeId, Label, NodeId)>,
    edge_policy: EdgePolicy,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn edge_policy(mut self, edge_policy: EdgePolicy) -> Self {
        self.edge_policy = edge_policy;
        self
    }

    pub fn node<'a>(
        &'a mut self,
        label: impl Into<Label>,
        id: impl Into<NodeId>,
    ) -> NodeBuilder<'a> {
        NodeBuilder {
            builder: self,
            label: label.into(),
            id: id.into(),
            props: Props::new(),
        }
    }

    pub fn edge<'a>(
        &'a mut self,
        label: impl Into<Label>,
        from: impl Into<NodeId>,
        to: impl Into<NodeId>,
    ) -> EdgeBuilder<'a> {
        EdgeBuilder {
            builder: self,
            id: None,
            label: label.into(),
            from: from.into(),
            to: to.into(),
            props: Props::new(),
        }
    }

    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = node.id.clone();
        self.nodes
            .entry(id.clone())
            .and_modify(|existing| {
                if existing.label == node.label {
                    existing.props.extend(node.props.clone());
                }
            })
            .or_insert(node);
        id
    }

    pub fn add_edge(&mut self, edge: Edge) -> Option<EdgeId> {
        let id = edge.id.clone();
        match self.edge_policy {
            EdgePolicy::AllowDuplicates => self.edges.push(edge),
            EdgePolicy::DedupeByFromLabelTo => {
                let key = (edge.from.clone(), edge.label.clone(), edge.to.clone());
                if self.edge_keys.insert(key) {
                    self.edges.push(edge);
                }
            }
        }
        id
    }

    pub fn build(self) -> Graph {
        Graph {
            nodes: self.nodes.into_values().collect(),
            edges: self.edges,
        }
    }
}

pub struct NodeBuilder<'a> {
    builder: &'a mut GraphBuilder,
    label: Label,
    id: NodeId,
    props: Props,
}

impl<'a> NodeBuilder<'a> {
    pub fn prop(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.props.insert(key.into(), value.into());
        self
    }

    pub fn props(mut self, props: Props) -> Self {
        self.props.extend(props);
        self
    }

    pub fn finish(self) -> NodeId {
        let node = Node::new(self.label, self.id, self.props);
        self.builder.add_node(node)
    }
}

pub struct EdgeBuilder<'a> {
    builder: &'a mut GraphBuilder,
    id: Option<EdgeId>,
    label: Label,
    from: NodeId,
    to: NodeId,
    props: Props,
}

impl<'a> EdgeBuilder<'a> {
    pub fn id(mut self, id: impl Into<EdgeId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn prop(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.props.insert(key.into(), value.into());
        self
    }

    pub fn props(mut self, props: Props) -> Self {
        self.props.extend(props);
        self
    }

    pub fn finish(self) -> Option<EdgeId> {
        let mut edge = Edge::new(self.label, self.from, self.to, self.props);
        edge.id = self.id;
        self.builder.add_edge(edge)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphSchema {
    pub nodes: Vec<NodeType>,
    pub edges: Vec<EdgeType>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeType {
    pub label: Label,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EdgeType {
    pub label: Label,
    pub from: Vec<Label>,
    pub to: Vec<Label>,
    pub fields: Vec<Field>,
    pub directed: bool,
    pub uniqueness: EdgeUniqueness,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub required: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FieldType {
    String,
    Int,
    Float,
    Bool,
    DateTime,
    StringArray,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EdgeUniqueness {
    None,
    FromTo,
    FromLabelTo,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Traversal {
    pub start: Start,
    pub steps: Vec<Step>,
    pub limit: Option<u32>,
}

impl Traversal {
    pub fn from_node(id: impl Into<NodeId>) -> Self {
        Self {
            start: Start::Node(id.into()),
            steps: Vec::new(),
            limit: None,
        }
    }

    pub fn out(mut self, edge: impl Into<Label>) -> Self {
        self.steps.push(Step {
            direction: Direction::Out,
            edge: Some(edge.into()),
            node: None,
        });
        self
    }

    pub fn in_(mut self, edge: impl Into<Label>) -> Self {
        self.steps.push(Step {
            direction: Direction::In,
            edge: Some(edge.into()),
            node: None,
        });
        self
    }

    pub fn both(mut self, edge: impl Into<Label>) -> Self {
        self.steps.push(Step {
            direction: Direction::Both,
            edge: Some(edge.into()),
            node: None,
        });
        self
    }

    pub fn to(mut self, node: impl Into<Label>) -> Self {
        if let Some(step) = self.steps.last_mut() {
            step.node = Some(node.into());
        }
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Start {
    Node(NodeId),
    NodesByLabel(Label),
    NodesByProperty {
        label: Label,
        key: String,
        value: Value,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub direction: Direction,
    pub edge: Option<Label>,
    pub node: Option<Label>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Direction {
    Out,
    In,
    Both,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdgeQuery {
    pub from: Option<NodeId>,
    pub to: Option<NodeId>,
    pub label: Option<Label>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadReport {
    pub nodes: usize,
    pub edges: usize,
}

#[async_trait]
pub trait GraphStore: Send + Sync {
    async fn apply_schema(&self, _schema: &GraphSchema) -> Result<()> {
        Ok(())
    }

    async fn put_node(&self, node: &Node) -> Result<NodeId>;
    async fn put_edge(&self, edge: &Edge) -> Result<Option<EdgeId>>;

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut report = LoadReport::default();
        for node in &graph.nodes {
            self.put_node(node).await?;
            report.nodes += 1;
        }
        for edge in &graph.edges {
            self.put_edge(edge).await?;
            report.edges += 1;
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>>;
    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>>;
    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>>;
}

#[async_trait]
pub trait GraphAdminStore: GraphStore {
    async fn bootstrap(&self) -> Result<()> {
        Ok(())
    }

    async fn clear(&self) -> Result<()>;
}

pub mod prelude {
    pub use crate::{
        Direction, Edge, EdgeId, EdgePolicy, EdgeQuery, EdgeType, Field, FieldType, Graph,
        GraphAdminStore, GraphBuilder, GraphSchema, GraphStore, GrustError, Label, LoadReport,
        Node, NodeId, NodeType, Props, Result, Start, Step, Traversal, Value,
    };
}

#[cfg(test)]
mod tests;
