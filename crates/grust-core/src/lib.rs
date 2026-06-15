use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
};

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

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

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
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

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self::new(value.clone())
            }
        }

        impl From<&$name> for $name {
            fn from(value: &$name) -> Self {
                value.clone()
            }
        }
    };
}

string_newtype!(
    /// Stable application-level node identifier.
    NodeId
);
string_newtype!(
    /// Optional application-level edge identifier.
    EdgeId
);
string_newtype!(
    /// Node or edge label.
    Label
);

/// Validated RFC 3339 date-time string used by [`Value::DateTime`].
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RfcDate(String);

impl RfcDate {
    /// Parses and stores an RFC 3339 date-time such as
    /// `2026-06-12T09:30:00Z` or `2026-06-12T09:30:00.123+02:00`.
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if is_rfc3339_datetime(&value) {
            Ok(Self(value))
        } else {
            Err(GrustError::Schema(format!(
                "'{value}' is not an RFC 3339 date-time"
            )))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for RfcDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RfcDate {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RfcDate {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
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
    /// An RFC 3339 date-time, e.g. `2026-06-12T09:30:00Z`. Construct with
    /// [`Value::datetime`] to get format validation.
    DateTime(RfcDate),
    StringArray(Vec<String>),
    IntArray(Vec<i64>),
    FloatArray(Vec<f64>),
    Json(serde_json::Value),
}

impl Value {
    /// Creates a `Value::DateTime`, validating that the input is an RFC 3339
    /// date-time such as `2026-06-12T09:30:00Z` or
    /// `2026-06-12T09:30:00.123+02:00`.
    pub fn datetime(value: impl Into<String>) -> Result<Self> {
        RfcDate::parse(value).map(Self::DateTime)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_datetime(&self) -> Option<&str> {
        match self {
            Self::DateTime(value) => Some(value.as_str()),
            _ => None,
        }
    }

    pub fn as_string_array(&self) -> Option<&[String]> {
        match self {
            Self::StringArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_int_array(&self) -> Option<&[i64]> {
        match self {
            Self::IntArray(values) => Some(values),
            _ => None,
        }
    }

    pub fn as_float_array(&self) -> Option<&[f64]> {
        match self {
            Self::FloatArray(values) => Some(values),
            _ => None,
        }
    }

    /// Converts to a plain (untagged) JSON value. `DateTime` becomes a JSON
    /// string, so a `to_json`/`from_json` round trip yields `Value::String`.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Null => serde_json::Value::Null,
            Self::Bool(value) => serde_json::Value::Bool(*value),
            Self::Int(value) => serde_json::Value::from(*value),
            Self::Float(value) => serde_json::Value::from(*value),
            Self::String(value) => serde_json::Value::String(value.clone()),
            Self::DateTime(value) => serde_json::Value::String(value.as_str().to_string()),
            Self::StringArray(values) => serde_json::Value::from(values.clone()),
            Self::IntArray(values) => serde_json::Value::from(values.clone()),
            Self::FloatArray(values) => serde_json::Value::from(values.clone()),
            Self::Json(value) => value.clone(),
        }
    }

    /// Converts from JSON, accepting both plain values and the tagged
    /// `{"type": ..., "value": ...}` form that `Value`'s serde representation
    /// produces.
    pub fn from_json(value: serde_json::Value) -> Self {
        if let serde_json::Value::Object(mapping) = &value
            && mapping.contains_key("type")
            && mapping.contains_key("value")
            && let Ok(tagged) = serde_json::from_value(value.clone())
        {
            return tagged;
        }
        Self::from(value)
    }
}

/// Validates the RFC 3339 date-time shape `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`.
fn is_rfc3339_datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let digit = |i: usize| bytes[i].is_ascii_digit();
    let all_digits = |range: std::ops::Range<usize>| range.clone().all(digit);
    let pair = |i: usize| (bytes[i] - b'0') * 10 + (bytes[i + 1] - b'0');

    if !(all_digits(0..4)
        && bytes[4] == b'-'
        && all_digits(5..7)
        && bytes[7] == b'-'
        && all_digits(8..10)
        && bytes[10] == b'T'
        && all_digits(11..13)
        && bytes[13] == b':'
        && all_digits(14..16)
        && bytes[16] == b':'
        && all_digits(17..19))
    {
        return false;
    }
    let year = bytes[0..4]
        .iter()
        .fold(0u16, |acc, digit| acc * 10 + u16::from(digit - b'0'));
    let month = pair(5);
    let day = pair(8);
    let leap_year = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => return false,
    };
    if !(day >= 1 && day <= max_day && pair(11) < 24 && pair(14) < 60 && pair(17) <= 60) {
        return false;
    }

    let mut i = 19;
    if bytes[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return false;
        }
    }
    match bytes.get(i) {
        Some(b'Z') => i + 1 == bytes.len(),
        Some(b'+' | b'-') => {
            i + 6 == bytes.len()
                && all_digits(i + 1..i + 3)
                && bytes[i + 3] == b':'
                && all_digits(i + 4..i + 6)
                && pair(i + 1) < 24
                && pair(i + 4) < 60
        }
        _ => false,
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

impl From<Vec<i64>> for Value {
    fn from(value: Vec<i64>) -> Self {
        Self::IntArray(value)
    }
}

impl From<Vec<f64>> for Value {
    fn from(value: Vec<f64>) -> Self {
        Self::FloatArray(value)
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
                let ints = values
                    .iter()
                    .filter_map(serde_json::Value::as_i64)
                    .collect::<Vec<_>>();
                if !values.is_empty() && ints.len() == values.len() {
                    return Self::IntArray(ints);
                }
                let floats = values
                    .iter()
                    .filter_map(serde_json::Value::as_f64)
                    .collect::<Vec<_>>();
                if !values.is_empty() && floats.len() == values.len() {
                    return Self::FloatArray(floats);
                }
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

#[cfg(feature = "typed-garde")]
pub mod typed {
    use serde::{Serialize, de::DeserializeOwned};

    use crate::{
        Edge, EdgeId, Graph, GraphBuilder, GrustError, Node, NodeId, Props, PutOutcome, Result,
        Value,
    };

    pub use garde;
    #[cfg(feature = "typed-zod-rs")]
    pub use zod_rs;

    pub trait TypedNode: garde::Validate + Serialize {
        const LABEL: &'static str;

        fn node_id(&self) -> NodeId;

        fn node_props(&self) -> Result<Props> {
            props_from_serialize(self)
        }

        fn from_node(node: &Node) -> Result<Self>
        where
            Self: Sized + DeserializeOwned,
            Self::Context: Default,
        {
            let ctx = Self::Context::default();
            Self::from_node_with(node, &ctx)
        }

        fn from_node_with(node: &Node, ctx: &Self::Context) -> Result<Self>
        where
            Self: Sized + DeserializeOwned,
        {
            if node.label.as_str() != Self::LABEL {
                return Err(GrustError::Schema(format!(
                    "node '{}' has label '{}', expected '{}'",
                    node.id.as_str(),
                    node.label.as_str(),
                    Self::LABEL
                )));
            }
            let typed: Self =
                serde_json::from_value(props_to_plain_json(&node.props)).map_err(|err| {
                    GrustError::Serialization(format!("typed node decode error: {err}"))
                })?;
            typed
                .validate_with(ctx)
                .map_err(|err| validation_error(Self::LABEL, err))?;
            let typed_id = typed.node_id();
            if typed_id != node.id {
                return Err(GrustError::Schema(format!(
                    "typed node '{}' decoded id '{}', expected '{}'",
                    Self::LABEL,
                    typed_id.as_str(),
                    node.id.as_str()
                )));
            }
            Ok(typed)
        }
    }

    pub trait TypedEdge: garde::Validate + Serialize {
        const LABEL: &'static str;

        fn source_node_id(&self) -> NodeId;

        fn target_node_id(&self) -> NodeId;

        fn edge_id(&self) -> Option<EdgeId> {
            None
        }

        fn edge_props(&self) -> Result<Props> {
            props_from_serialize(self)
        }

        fn from_edge(edge: &Edge) -> Result<Self>
        where
            Self: Sized + DeserializeOwned,
            Self::Context: Default,
        {
            let ctx = Self::Context::default();
            Self::from_edge_with(edge, &ctx)
        }

        fn from_edge_with(edge: &Edge, ctx: &Self::Context) -> Result<Self>
        where
            Self: Sized + DeserializeOwned,
        {
            if edge.label.as_str() != Self::LABEL {
                return Err(GrustError::Schema(format!(
                    "edge from '{}' to '{}' has label '{}', expected '{}'",
                    edge.from.as_str(),
                    edge.to.as_str(),
                    edge.label.as_str(),
                    Self::LABEL
                )));
            }
            let typed: Self =
                serde_json::from_value(props_to_plain_json(&edge.props)).map_err(|err| {
                    GrustError::Serialization(format!("typed edge decode error: {err}"))
                })?;
            typed
                .validate_with(ctx)
                .map_err(|err| validation_error(Self::LABEL, err))?;
            if typed.source_node_id() != edge.from || typed.target_node_id() != edge.to {
                return Err(GrustError::Schema(format!(
                    "typed edge '{}' decoded endpoints '{}' -> '{}', expected '{}' -> '{}'",
                    Self::LABEL,
                    typed.source_node_id().as_str(),
                    typed.target_node_id().as_str(),
                    edge.from.as_str(),
                    edge.to.as_str()
                )));
            }
            if let Some(decoded_id) = typed.edge_id()
                && edge
                    .id
                    .as_ref()
                    .is_some_and(|edge_id| edge_id != &decoded_id)
            {
                return Err(GrustError::Schema(format!(
                    "typed edge '{}' decoded id '{}', expected '{}'",
                    Self::LABEL,
                    decoded_id.as_str(),
                    edge.id.as_ref().expect("edge id checked").as_str()
                )));
            }
            Ok(typed)
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct TypedGraphBuilder {
        builder: GraphBuilder,
    }

    impl TypedGraphBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn from_builder(builder: GraphBuilder) -> Self {
            Self { builder }
        }

        pub fn from_graph(graph: Graph) -> Self {
            let mut builder = GraphBuilder::new();
            for node in graph.nodes {
                builder.add_node(node);
            }
            for edge in graph.edges {
                builder.add_edge(edge);
            }
            Self { builder }
        }

        pub fn add_raw_node(&mut self, node: Node) -> NodeId {
            self.builder.add_node(node)
        }

        pub fn add_raw_edge(&mut self, edge: Edge) -> PutOutcome {
            self.builder.add_edge(edge)
        }

        pub fn add_node<T>(&mut self, node: &T) -> Result<NodeId>
        where
            T: TypedNode,
            T::Context: Default,
        {
            node.validate()
                .map_err(|err| validation_error(T::LABEL, err))?;
            self.add_validated_node(node)
        }

        pub fn add_node_with<T>(&mut self, node: &T, ctx: &T::Context) -> Result<NodeId>
        where
            T: TypedNode,
        {
            node.validate_with(ctx)
                .map_err(|err| validation_error(T::LABEL, err))?;
            self.add_validated_node(node)
        }

        pub fn add_edge<T>(&mut self, edge: &T) -> Result<PutOutcome>
        where
            T: TypedEdge,
            T::Context: Default,
        {
            edge.validate()
                .map_err(|err| validation_error(T::LABEL, err))?;
            self.add_validated_edge(edge)
        }

        pub fn add_edge_with<T>(&mut self, edge: &T, ctx: &T::Context) -> Result<PutOutcome>
        where
            T: TypedEdge,
        {
            edge.validate_with(ctx)
                .map_err(|err| validation_error(T::LABEL, err))?;
            self.add_validated_edge(edge)
        }

        #[cfg(feature = "typed-zod-rs")]
        pub fn add_node_from_json<T, S>(
            &mut self,
            schema: &S,
            value: &serde_json::Value,
        ) -> Result<NodeId>
        where
            T: TypedNode + DeserializeOwned,
            T::Context: Default,
            S: zod_rs::Schema<serde_json::Value>,
        {
            let node = parse_typed_json::<T, S>(schema, value)?;
            self.add_validated_node(&node)
        }

        #[cfg(feature = "typed-zod-rs")]
        pub fn add_node_from_json_with<T, S>(
            &mut self,
            schema: &S,
            value: &serde_json::Value,
            ctx: &T::Context,
        ) -> Result<NodeId>
        where
            T: TypedNode + DeserializeOwned,
            S: zod_rs::Schema<serde_json::Value>,
        {
            let node = parse_typed_json_with::<T, S>(schema, value, ctx)?;
            self.add_validated_node(&node)
        }

        #[cfg(feature = "typed-zod-rs")]
        pub fn add_edge_from_json<T, S>(
            &mut self,
            schema: &S,
            value: &serde_json::Value,
        ) -> Result<PutOutcome>
        where
            T: TypedEdge + DeserializeOwned,
            T::Context: Default,
            S: zod_rs::Schema<serde_json::Value>,
        {
            let edge = parse_typed_json::<T, S>(schema, value)?;
            self.add_validated_edge(&edge)
        }

        #[cfg(feature = "typed-zod-rs")]
        pub fn add_edge_from_json_with<T, S>(
            &mut self,
            schema: &S,
            value: &serde_json::Value,
            ctx: &T::Context,
        ) -> Result<PutOutcome>
        where
            T: TypedEdge + DeserializeOwned,
            S: zod_rs::Schema<serde_json::Value>,
        {
            let edge = parse_typed_json_with::<T, S>(schema, value, ctx)?;
            self.add_validated_edge(&edge)
        }

        #[must_use = "discarding this means the typed graph was not built"]
        pub fn build(self) -> Graph {
            self.builder.build()
        }

        pub fn into_builder(self) -> GraphBuilder {
            self.builder
        }

        fn add_validated_node<T>(&mut self, node: &T) -> Result<NodeId>
        where
            T: TypedNode,
        {
            let node_id = node.node_id();
            let mut props = node.node_props()?;
            props
                .entry("id".to_string())
                .or_insert_with(|| Value::from(node_id.as_str()));
            let graph_node = Node::new(T::LABEL, node_id, props);
            Ok(self.builder.add_node(graph_node))
        }

        fn add_validated_edge<T>(&mut self, edge: &T) -> Result<PutOutcome>
        where
            T: TypedEdge,
        {
            let mut graph_edge = Edge::new(
                T::LABEL,
                edge.source_node_id(),
                edge.target_node_id(),
                edge.edge_props()?,
            );
            graph_edge.id = edge.edge_id();
            Ok(self.builder.add_edge(graph_edge))
        }
    }

    pub fn props_from_serialize<T>(value: &T) -> Result<Props>
    where
        T: Serialize + ?Sized,
    {
        let serialized = serde_json::to_value(value)
            .map_err(|err| GrustError::Serialization(format!("typed props error: {err}")))?;
        let serde_json::Value::Object(fields) = serialized else {
            return Err(GrustError::Schema(
                "typed graph values must serialize as JSON objects".to_string(),
            ));
        };

        Ok(fields
            .into_iter()
            .map(|(key, value)| (key, Value::from(value)))
            .collect())
    }

    fn props_to_plain_json(props: &Props) -> serde_json::Value {
        serde_json::Value::Object(
            props
                .iter()
                .map(|(key, value)| (key.clone(), value.to_json()))
                .collect(),
        )
    }

    #[cfg(feature = "typed-zod-rs")]
    pub fn parse_typed_json<T, S>(schema: &S, value: &serde_json::Value) -> Result<T>
    where
        T: DeserializeOwned + garde::Validate,
        T::Context: Default,
        S: zod_rs::Schema<serde_json::Value>,
    {
        let ctx = T::Context::default();
        parse_typed_json_with(schema, value, &ctx)
    }

    #[cfg(feature = "typed-zod-rs")]
    pub fn parse_typed_json_with<T, S>(
        schema: &S,
        value: &serde_json::Value,
        ctx: &T::Context,
    ) -> Result<T>
    where
        T: DeserializeOwned + garde::Validate,
        S: zod_rs::Schema<serde_json::Value>,
    {
        schema
            .safe_parse(value)
            .map_err(|err| GrustError::Schema(format!("zod-rs validation failed: {err}")))?;
        let typed: T = serde_json::from_value(value.clone())
            .map_err(|err| GrustError::Serialization(format!("typed JSON decode error: {err}")))?;
        typed
            .validate_with(ctx)
            .map_err(|err| GrustError::Schema(format!("typed validation failed: {err}")))?;
        Ok(typed)
    }

    fn validation_error(label: &str, err: garde::Report) -> GrustError {
        GrustError::Schema(format!("{label} validation failed: {err}"))
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

/// Normalizes an edge label into an uppercase backend relationship type.
///
/// Non-ASCII-alphanumeric characters become underscores. Empty labels fall
/// back to `RELATED_TO`.
pub fn relationship_type(value: &str) -> String {
    let relationship = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if relationship.is_empty() {
        "RELATED_TO".to_string()
    } else {
        relationship
    }
}

/// Normalizes arbitrary schema text into a lower_snake_case backend identifier.
///
/// This helper is for SQL-like backends that require identifiers to start with a
/// non-digit ASCII alphanumeric or underscore character after normalization.
pub fn schema_identifier(value: &str) -> Result<String> {
    let identifier = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    if identifier.is_empty()
        || identifier
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_digit())
    {
        return Err(GrustError::Schema(format!(
            "invalid schema identifier '{value}'"
        )));
    }
    Ok(identifier)
}

/// Returns the stable key used by tabular/export backends for an edge.
///
/// Explicit edge IDs win. Otherwise the structural key joins `from`, `label`,
/// and `to` with U+001F (Unit Separator). Callers that accept arbitrary IDs or
/// labels should reject U+001F before relying on reversibility.
pub fn edge_key(edge: &Edge) -> String {
    edge.id
        .as_ref()
        .map(EdgeId::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            let from = edge.from.as_str();
            let label = edge.label.as_str();
            let to = edge.to.as_str();
            let mut key = String::with_capacity(from.len() + label.len() + to.len() + 2);
            key.push_str(from);
            key.push('\u{1f}');
            key.push_str(label);
            key.push('\u{1f}');
            key.push_str(to);
            key
        })
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

    pub fn from_yaml(yaml: &str) -> Result<Self> {
        yaml::graph_from_yaml(yaml)
    }

    pub fn to_yaml(&self) -> Result<String> {
        yaml::graph_to_yaml(self)
    }

    pub fn from_json(json: &str) -> Result<Self> {
        json::graph_from_json(json)
    }

    pub fn to_json(&self) -> Result<String> {
        json::graph_to_json(self)
    }

    pub fn from_xml(xml: &str) -> Result<Self> {
        xml::graph_from_xml(xml)
    }

    pub fn to_xml(&self) -> Result<String> {
        xml::graph_to_xml(self)
    }

    pub fn builder() -> GraphBuilder {
        GraphBuilder::new()
    }
}

/// Dense, reusable indexes over a [`Graph`].
///
/// This is backend-neutral: it validates edge endpoints, maps node ids to
/// stable vertex indexes, and stores edge adjacency by both node id and vertex
/// index. Higher-level crates can use it for local analytics or query planning
/// without rebuilding the same maps.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphIndex {
    vertex_by_id: HashMap<NodeId, usize>,
    outgoing_by_vertex: Vec<Vec<usize>>,
    incoming_by_vertex: Vec<Vec<usize>>,
    edge_endpoints: Vec<(usize, usize)>,
}

impl GraphIndex {
    pub fn new(graph: &Graph) -> Result<Self> {
        let mut vertex_by_id = HashMap::with_capacity(graph.nodes.len());
        for (index, vertex) in graph.nodes.iter().enumerate() {
            if vertex_by_id.insert(vertex.id.clone(), index).is_some() {
                return Err(GrustError::Schema(format!(
                    "duplicate vertex id '{}'",
                    vertex.id.as_str()
                )));
            }
        }

        let mut outgoing_by_vertex = vec![Vec::<usize>::new(); graph.nodes.len()];
        let mut incoming_by_vertex = vec![Vec::<usize>::new(); graph.nodes.len()];
        let mut edge_endpoints = Vec::<(usize, usize)>::with_capacity(graph.edges.len());

        for (edge_index, edge) in graph.edges.iter().enumerate() {
            let Some(&from_index) = vertex_by_id.get(&edge.from) else {
                return Err(GrustError::Schema(format!(
                    "edge source '{}' is not present in vertices",
                    edge.from.as_str()
                )));
            };
            let Some(&to_index) = vertex_by_id.get(&edge.to) else {
                return Err(GrustError::Schema(format!(
                    "edge destination '{}' is not present in vertices",
                    edge.to.as_str()
                )));
            };

            outgoing_by_vertex[from_index].push(edge_index);
            incoming_by_vertex[to_index].push(edge_index);
            edge_endpoints.push((from_index, to_index));
        }

        Ok(Self {
            vertex_by_id,
            outgoing_by_vertex,
            incoming_by_vertex,
            edge_endpoints,
        })
    }

    pub fn vertex_index(&self, id: &NodeId) -> Option<usize> {
        self.vertex_by_id.get(id).copied()
    }

    pub fn require_vertex_index(&self, id: &NodeId) -> Result<usize> {
        self.vertex_index(id)
            .ok_or_else(|| GrustError::Schema(format!("vertex '{}' is not present", id.as_str())))
    }

    pub fn outgoing_edges(&self, id: &NodeId) -> &[usize] {
        self.vertex_index(id)
            .map(|index| self.outgoing_by_vertex(index))
            .unwrap_or(&[])
    }

    pub fn incoming_edges(&self, id: &NodeId) -> &[usize] {
        self.vertex_index(id)
            .map(|index| self.incoming_by_vertex(index))
            .unwrap_or(&[])
    }

    pub fn outgoing_by_vertex(&self, index: usize) -> &[usize] {
        self.outgoing_by_vertex
            .get(index)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn incoming_by_vertex(&self, index: usize) -> &[usize] {
        self.incoming_by_vertex
            .get(index)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn edge_endpoints(&self, edge_index: usize) -> (usize, usize) {
        self.edge_endpoints[edge_index]
    }

    pub fn edge_endpoints_slice(&self) -> &[(usize, usize)] {
        &self.edge_endpoints
    }

    pub fn out_degree(&self, index: usize) -> usize {
        self.outgoing_by_vertex[index].len()
    }

    pub fn in_degree(&self, index: usize) -> usize {
        self.incoming_by_vertex[index].len()
    }

    pub fn degree(&self, index: usize) -> usize {
        self.in_degree(index) + self.out_degree(index)
    }
}

mod graph_doc {
    use std::collections::{BTreeMap, BTreeSet};

    use serde::{Deserialize, Serialize};

    use crate::{Edge, EdgeId, Graph, GrustError, Label, Node, NodeId, Props, Value};

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct GraphDoc {
        #[serde(default)]
        pub(super) nodes: Vec<NodeDoc>,
        #[serde(default)]
        pub(super) edges: Vec<EdgeDoc>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct NodeDoc {
        pub(super) id: NodeId,
        pub(super) label: Label,
        #[serde(default, deserialize_with = "deserialize_props")]
        pub(super) props: Props,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct NodeDocOut {
        pub(super) id: NodeId,
        pub(super) label: Label,
        #[serde(default)]
        pub(super) props: Props,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct EdgeDoc {
        #[serde(default)]
        pub(super) id: Option<EdgeId>,
        pub(super) label: Label,
        pub(super) from: NodeId,
        pub(super) to: NodeId,
        #[serde(default, deserialize_with = "deserialize_props")]
        pub(super) props: Props,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct EdgeDocOut {
        #[serde(default)]
        pub(super) id: Option<EdgeId>,
        pub(super) label: Label,
        pub(super) from: NodeId,
        pub(super) to: NodeId,
        #[serde(default)]
        pub(super) props: Props,
    }

    pub(super) fn graph_from_doc(doc: GraphDoc) -> super::Result<Graph> {
        let mut ids = BTreeSet::new();
        for node in &doc.nodes {
            if !ids.insert(node.id.clone()) {
                return Err(GrustError::Schema(format!(
                    "duplicate node id '{}'",
                    node.id
                )));
            }
        }

        let mut edges = Vec::with_capacity(doc.edges.len());
        for edge in doc.edges {
            if !ids.contains(&edge.from) {
                return Err(GrustError::Schema(format!(
                    "edge '{}' references unknown from node '{}'",
                    edge.label, edge.from
                )));
            }
            if !ids.contains(&edge.to) {
                return Err(GrustError::Schema(format!(
                    "edge '{}' references unknown to node '{}'",
                    edge.label, edge.to
                )));
            }

            let mut graph_edge = Edge::new(edge.label, edge.from, edge.to, edge.props);
            graph_edge.id = edge.id;
            edges.push(graph_edge);
        }

        let nodes = doc
            .nodes
            .into_iter()
            .map(|node| Node::new(node.label, node.id, node.props))
            .collect();

        Ok(Graph::new(nodes, edges))
    }

    pub(super) fn graph_to_doc(graph: &Graph) -> GraphDocOut {
        GraphDocOut {
            nodes: graph
                .nodes
                .iter()
                .map(|node| NodeDocOut {
                    id: node.id.clone(),
                    label: node.label.clone(),
                    props: without_generated_id(&node.props, &node.id),
                })
                .collect(),
            edges: graph
                .edges
                .iter()
                .map(|edge| EdgeDocOut {
                    id: edge.id.clone(),
                    label: edge.label.clone(),
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    props: edge.props.clone(),
                })
                .collect(),
        }
    }

    fn without_generated_id(props: &Props, id: &NodeId) -> Props {
        let mut props = props.clone();
        if props.get("id") == Some(&Value::from(id.as_str())) {
            props.remove("id");
        }
        props
    }

    fn deserialize_props<'de, D>(deserializer: D) -> std::result::Result<Props, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        raw.into_iter()
            .map(|(key, value)| {
                value_from_json(value)
                    .map(|value| (key, value))
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }

    fn value_from_json(value: serde_json::Value) -> std::result::Result<Value, String> {
        if let serde_json::Value::Object(mapping) = &value
            && mapping.contains_key("type")
            && mapping.contains_key("value")
        {
            return serde_json::from_value(value)
                .map_err(|err| format!("invalid tagged Grust value: {err}"));
        }

        Ok(Value::from_json(value))
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub(super) struct GraphDocOut {
        pub(super) nodes: Vec<NodeDocOut>,
        pub(super) edges: Vec<EdgeDocOut>,
    }
}

mod yaml {
    use crate::{Graph, GrustError};

    pub(super) fn graph_from_yaml(yaml: &str) -> super::Result<Graph> {
        let doc: super::graph_doc::GraphDoc = serde_yaml::from_str(yaml)
            .map_err(|err| GrustError::Serialization(format!("YAML parse error: {err}")))?;
        super::graph_doc::graph_from_doc(doc)
    }

    pub(super) fn graph_to_yaml(graph: &Graph) -> super::Result<String> {
        serde_yaml::to_string(&super::graph_doc::graph_to_doc(graph))
            .map_err(|err| GrustError::Serialization(format!("YAML serialization error: {err}")))
    }
}

mod json {
    use crate::{Graph, GrustError};

    pub(super) fn graph_from_json(json: &str) -> super::Result<Graph> {
        let doc: super::graph_doc::GraphDoc = serde_json::from_str(json)
            .map_err(|err| GrustError::Serialization(format!("JSON parse error: {err}")))?;
        super::graph_doc::graph_from_doc(doc)
    }

    pub(super) fn graph_to_json(graph: &Graph) -> super::Result<String> {
        serde_json::to_string_pretty(&super::graph_doc::graph_to_doc(graph))
            .map_err(|err| GrustError::Serialization(format!("JSON serialization error: {err}")))
    }
}

mod xml {
    use serde::{Deserialize, Serialize};

    use crate::{Edge, EdgeId, Graph, GrustError, Label, Node, NodeId, Props, Value};

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(rename = "graph")]
    struct GraphXml {
        #[serde(default)]
        nodes: NodesXml,
        #[serde(default)]
        edges: EdgesXml,
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct NodesXml {
        #[serde(rename = "node", default)]
        items: Vec<NodeXml>,
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct EdgesXml {
        #[serde(rename = "edge", default)]
        items: Vec<EdgeXml>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct NodeXml {
        id: NodeId,
        label: Label,
        #[serde(default)]
        props: PropsXml,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct EdgeXml {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<EdgeId>,
        label: Label,
        from: NodeId,
        to: NodeId,
        #[serde(default)]
        props: PropsXml,
    }

    #[derive(Debug, Default, Serialize, Deserialize)]
    struct PropsXml {
        #[serde(rename = "prop", default)]
        items: Vec<PropXml>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct PropXml {
        key: String,
        value: Value,
    }

    pub(super) fn graph_from_xml(xml: &str) -> super::Result<Graph> {
        let doc: GraphXml = quick_xml::de::from_str(xml)
            .map_err(|err| GrustError::Serialization(format!("XML parse error: {err}")))?;
        super::graph_doc::graph_from_doc(doc.into())
    }

    pub(super) fn graph_to_xml(graph: &Graph) -> super::Result<String> {
        quick_xml::se::to_string(&GraphXml::from(graph))
            .map_err(|err| GrustError::Serialization(format!("XML serialization error: {err}")))
    }

    impl From<GraphXml> for super::graph_doc::GraphDoc {
        fn from(value: GraphXml) -> Self {
            Self {
                nodes: value.nodes.items.into_iter().map(Into::into).collect(),
                edges: value.edges.items.into_iter().map(Into::into).collect(),
            }
        }
    }

    impl From<NodeXml> for super::graph_doc::NodeDoc {
        fn from(value: NodeXml) -> Self {
            Self {
                id: value.id,
                label: value.label,
                props: value.props.into(),
            }
        }
    }

    impl From<EdgeXml> for super::graph_doc::EdgeDoc {
        fn from(value: EdgeXml) -> Self {
            Self {
                id: value.id,
                label: value.label,
                from: value.from,
                to: value.to,
                props: value.props.into(),
            }
        }
    }

    impl From<PropsXml> for Props {
        fn from(value: PropsXml) -> Self {
            value
                .items
                .into_iter()
                .map(|prop| (prop.key, prop.value))
                .collect()
        }
    }

    impl From<&Graph> for GraphXml {
        fn from(graph: &Graph) -> Self {
            Self {
                nodes: NodesXml {
                    items: graph.nodes.iter().map(NodeXml::from).collect(),
                },
                edges: EdgesXml {
                    items: graph.edges.iter().map(EdgeXml::from).collect(),
                },
            }
        }
    }

    impl From<&Node> for NodeXml {
        fn from(node: &Node) -> Self {
            let props = super::graph_doc::graph_to_doc(&Graph::new(vec![node.clone()], Vec::new()))
                .nodes
                .into_iter()
                .next()
                .expect("node exists")
                .props;
            Self {
                id: node.id.clone(),
                label: node.label.clone(),
                props: props.into(),
            }
        }
    }

    impl From<&Edge> for EdgeXml {
        fn from(edge: &Edge) -> Self {
            Self {
                id: edge.id.clone(),
                label: edge.label.clone(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                props: edge.props.clone().into(),
            }
        }
    }

    impl From<Props> for PropsXml {
        fn from(value: Props) -> Self {
            Self {
                items: value
                    .into_iter()
                    .map(|(key, value)| PropXml { key, value })
                    .collect(),
            }
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum EdgePolicy {
    AllowDuplicates,
    #[default]
    DedupeByFromLabelTo,
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

    /// Adds a node, merging with any existing node that has the same id.
    ///
    /// If a node with the same id and the same label already exists, the new
    /// props are merged in (new values win). If a node with the same id but a
    /// different label exists, the new node replaces it entirely (last write
    /// wins, matching `GraphStore::put_node` overwrite semantics).
    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = node.id.clone();
        self.nodes
            .entry(id.clone())
            .and_modify(|existing| {
                if existing.label == node.label {
                    existing.props.extend(node.props.clone());
                } else {
                    *existing = node.clone();
                }
            })
            .or_insert(node);
        id
    }

    /// Adds an edge, reporting whether it was stored or dropped by the
    /// builder's [`EdgePolicy`].
    pub fn add_edge(&mut self, edge: Edge) -> PutOutcome {
        match self.edge_policy {
            EdgePolicy::AllowDuplicates => {
                self.edges.push(edge);
                PutOutcome::Inserted
            }
            EdgePolicy::DedupeByFromLabelTo => {
                let key = (edge.from.clone(), edge.label.clone(), edge.to.clone());
                if self.edge_keys.insert(key) {
                    self.edges.push(edge);
                    PutOutcome::Inserted
                } else {
                    PutOutcome::Deduped
                }
            }
        }
    }

    #[must_use = "discarding this means the graph was not built"]
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

    #[must_use = "discarding this means the node was not added to the builder"]
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

    #[must_use = "discarding this means the edge was not added to the builder"]
    pub fn finish(self) -> PutOutcome {
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

impl GraphSchema {
    pub fn builder() -> GraphSchemaBuilder {
        GraphSchemaBuilder::default()
    }

    pub fn node_type(&self, label: &Label) -> Option<&NodeType> {
        self.nodes
            .iter()
            .find(|node_type| &node_type.label == label)
    }

    pub fn edge_type(&self, label: &Label) -> Option<&EdgeType> {
        self.edges
            .iter()
            .find(|edge_type| &edge_type.label == label)
    }

    pub fn validate_graph(&self, graph: &Graph) -> Result<()> {
        for node in &graph.nodes {
            self.validate_node(node)?;
        }
        let labels: BTreeMap<&NodeId, &Label> = graph
            .nodes
            .iter()
            .map(|node| (&node.id, &node.label))
            .collect();
        for edge in &graph.edges {
            self.validate_edge_with(edge, |id| labels.get(id).copied())?;
        }
        self.validate_edge_uniqueness(graph)
    }

    /// Enforces each edge type's [`EdgeUniqueness`]: at most one edge of the
    /// type between a given endpoint pair (unordered for undirected types).
    fn validate_edge_uniqueness(&self, graph: &Graph) -> Result<()> {
        let mut seen = BTreeSet::new();
        for edge in &graph.edges {
            let Some(edge_type) = self.edge_type(&edge.label) else {
                continue;
            };
            if edge_type.uniqueness == EdgeUniqueness::None {
                continue;
            }
            let (a, b) = if edge_type.directed || edge.from <= edge.to {
                (&edge.from, &edge.to)
            } else {
                (&edge.to, &edge.from)
            };
            if !seen.insert((edge.label.clone(), a.clone(), b.clone())) {
                return Err(GrustError::Schema(format!(
                    "duplicate edge '{}' between '{}' and '{}' violates {:?} uniqueness",
                    edge.label.as_str(),
                    a.as_str(),
                    b.as_str(),
                    edge_type.uniqueness
                )));
            }
        }
        Ok(())
    }

    pub fn validate_node(&self, node: &Node) -> Result<()> {
        let node_type = self.node_type(&node.label).ok_or_else(|| {
            GrustError::Schema(format!("schema has no node type '{}'", node.label.as_str()))
        })?;
        validate_props(
            &node.props,
            &node_type.fields,
            &format!("node '{}'", node.id.as_str()),
        )
    }

    pub fn validate_edge(&self, edge: &Edge, graph: &Graph) -> Result<()> {
        self.validate_edge_with(edge, |id| {
            graph
                .nodes
                .iter()
                .find(|node| &node.id == id)
                .map(|node| &node.label)
        })
    }

    /// Validates an edge using a label lookup instead of a full `Graph`, so
    /// stores can validate against their own node index without cloning.
    pub fn validate_edge_with<'a>(
        &self,
        edge: &Edge,
        lookup: impl Fn(&NodeId) -> Option<&'a Label>,
    ) -> Result<()> {
        let edge_type = self.edge_type(&edge.label).ok_or_else(|| {
            GrustError::Schema(format!("schema has no edge type '{}'", edge.label.as_str()))
        })?;

        let from_label = lookup(&edge.from).ok_or_else(|| {
            GrustError::Schema(format!(
                "edge '{}' references unknown from node '{}'",
                edge.label.as_str(),
                edge.from.as_str()
            ))
        })?;
        let to_label = lookup(&edge.to).ok_or_else(|| {
            GrustError::Schema(format!(
                "edge '{}' references unknown to node '{}'",
                edge.label.as_str(),
                edge.to.as_str()
            ))
        })?;

        let from_matches =
            |label: &Label| edge_type.from.is_empty() || edge_type.from.contains(label);
        let to_matches = |label: &Label| edge_type.to.is_empty() || edge_type.to.contains(label);
        // Undirected edge types accept their endpoint labels in either
        // orientation.
        let endpoints_ok = (from_matches(from_label) && to_matches(to_label))
            || (!edge_type.directed && from_matches(to_label) && to_matches(from_label));
        if !endpoints_ok {
            if !from_matches(from_label) {
                return Err(GrustError::Schema(format!(
                    "edge '{}' cannot start from node label '{}'",
                    edge.label.as_str(),
                    from_label.as_str()
                )));
            }
            return Err(GrustError::Schema(format!(
                "edge '{}' cannot end at node label '{}'",
                edge.label.as_str(),
                to_label.as_str()
            )));
        }

        validate_props(
            &edge.props,
            &edge_type.fields,
            &format!("edge '{}'", edge.label.as_str()),
        )
    }

    /// Validates an edge's label and props against the schema without
    /// checking endpoint nodes, for stores that persist a single edge and
    /// cannot cheaply resolve its endpoints.
    pub fn validate_edge_props(&self, edge: &Edge) -> Result<()> {
        let edge_type = self.edge_type(&edge.label).ok_or_else(|| {
            GrustError::Schema(format!("schema has no edge type '{}'", edge.label.as_str()))
        })?;
        validate_props(
            &edge.props,
            &edge_type.fields,
            &format!("edge '{}'", edge.label.as_str()),
        )
    }
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
    IntArray,
    FloatArray,
    Json,
}

/// How many edges of one type may exist between a pair of nodes.
///
/// `validate_graph` enforces `FromTo` and `FromLabelTo` identically — at most
/// one edge of the type between a given endpoint pair (unordered when the
/// type is undirected). The distinction is a storage-key hint for backends
/// that keep all edge labels in one table: `FromLabelTo` keys include the
/// label, `FromTo` keys do not.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum EdgeUniqueness {
    None,
    FromTo,
    FromLabelTo,
}

#[derive(Clone, Debug, Default)]
pub struct GraphSchemaBuilder {
    nodes: Vec<NodeType>,
    edges: Vec<EdgeType>,
}

impl GraphSchemaBuilder {
    pub fn node(mut self, label: impl Into<Label>, fields: impl Into<Vec<Field>>) -> Self {
        self.nodes.push(NodeType {
            label: label.into(),
            fields: fields.into(),
        });
        self
    }

    pub fn edge(
        mut self,
        label: impl Into<Label>,
        from: impl Into<Vec<Label>>,
        to: impl Into<Vec<Label>>,
        fields: impl Into<Vec<Field>>,
    ) -> Self {
        self.edges.push(EdgeType {
            label: label.into(),
            from: from.into(),
            to: to.into(),
            fields: fields.into(),
            directed: true,
            uniqueness: EdgeUniqueness::FromLabelTo,
        });
        self
    }

    pub fn edge_type(mut self, edge_type: EdgeType) -> Self {
        self.edges.push(edge_type);
        self
    }

    pub fn build(self) -> GraphSchema {
        GraphSchema {
            nodes: self.nodes,
            edges: self.edges,
        }
    }
}

impl Field {
    pub fn required(name: impl Into<String>, ty: FieldType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: true,
        }
    }

    pub fn optional(name: impl Into<String>, ty: FieldType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: false,
        }
    }
}

fn validate_props(props: &Props, fields: &[Field], context: &str) -> Result<()> {
    for field in fields {
        match props.get(&field.name) {
            Some(value) => validate_field_value(value, &field.ty, context, &field.name)?,
            None if field.required => {
                return Err(GrustError::Schema(format!(
                    "{context} missing required field '{}'",
                    field.name
                )));
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_field_value(
    value: &Value,
    ty: &FieldType,
    context: &str,
    field_name: &str,
) -> Result<()> {
    let matches = match (value, ty) {
        (Value::String(_), FieldType::String)
        | (Value::Int(_), FieldType::Int)
        | (Value::Float(_), FieldType::Float)
        | (Value::Bool(_), FieldType::Bool)
        | (Value::DateTime(_), FieldType::DateTime)
        | (Value::StringArray(_), FieldType::StringArray)
        | (Value::IntArray(_), FieldType::IntArray)
        | (Value::FloatArray(_), FieldType::FloatArray)
        | (_, FieldType::Json) => true,
        // Plain strings remain valid date-times for backward compatibility,
        // but must still parse as RFC 3339.
        (Value::String(value), FieldType::DateTime) => is_rfc3339_datetime(value),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(GrustError::Schema(format!(
            "{context} field '{field_name}' expected {ty:?}, got {value:?}"
        )))
    }
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

    /// Constrains the target node label of the most recent step.
    ///
    /// # Panics
    ///
    /// Panics if called before any step has been added with `out`, `in_`, or
    /// `both`, since there is no step for the label to apply to.
    pub fn to(mut self, node: impl Into<Label>) -> Self {
        let step = self
            .steps
            .last_mut()
            .expect("Traversal::to() must follow out(), in_(), or both()");
        step.node = Some(node.into());
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

/// Outcome of writing a single node or edge.
///
/// `Inserted` and `Updated` are precise only for stores that can cheaply
/// distinguish create from replace. Remote upsert-oriented backends commonly
/// return `Upserted` because doing otherwise would require an extra read or a
/// backend-specific write primitive. Portable callers should treat all three
/// written outcomes as success and should not rely on `Inserted` or `Updated`
/// unless they are intentionally targeting a backend that documents those
/// precise outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PutOutcome {
    /// The element did not exist before and was created.
    Inserted,
    /// An element with the same identity existed and was overwritten or
    /// merged.
    Updated,
    /// The element was written by an upsert and the backend cannot tell
    /// whether it was an insert or an update.
    Upserted,
    /// The element was dropped by a dedupe policy and nothing was written.
    Deduped,
}

impl PutOutcome {
    /// True unless the element was deduped away.
    pub fn written(self) -> bool {
        !matches!(self, Self::Deduped)
    }
}

/// Counts of nodes and edges written by a bulk load. Each count reflects an
/// upsert applied to the backend, not distinct newly created elements.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoadReport {
    pub nodes: usize,
    pub edges: usize,
}

#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Applies backend schema metadata.
    ///
    /// `GraphSchema` is a portable declaration of expected node labels, edge
    /// labels, fields, endpoint labels, direction, and uniqueness. The default
    /// implementation is a no-op for schemaless stores. Backend implementations
    /// may use the schema for validation, typed native tables, views, indexes,
    /// generated query shapes, or database-native schema definitions.
    ///
    /// Applying a schema does not imply the same enforcement guarantee on every
    /// backend. Callers that need portable preflight validation should call
    /// [`GraphSchema::validate_graph`] before writing, or use
    /// [`GraphStore::put_typed_graph`], which does that validation before
    /// applying the backend schema and writing the graph. Individual backends
    /// document whether they also validate each subsequent write at runtime.
    async fn apply_schema(&self, _schema: &GraphSchema) -> Result<()> {
        Ok(())
    }

    /// Writes one node.
    ///
    /// The returned [`PutOutcome`] reports the most precise result the backend
    /// can provide. Remote upsert backends generally return
    /// [`PutOutcome::Upserted`] for both inserts and updates.
    async fn put_node(&self, node: &Node) -> Result<PutOutcome>;

    /// Writes one edge.
    ///
    /// The returned [`PutOutcome`] reports the most precise result the backend
    /// can provide. Remote upsert backends generally return
    /// [`PutOutcome::Upserted`] for both inserts and updates.
    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome>;

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut report = LoadReport::default();
        for node in &graph.nodes {
            if self.put_node(node).await?.written() {
                report.nodes += 1;
            }
        }
        for edge in &graph.edges {
            if self.put_edge(edge).await?.written() {
                report.edges += 1;
            }
        }
        Ok(report)
    }

    async fn put_typed_graph(&self, schema: &GraphSchema, graph: &Graph) -> Result<LoadReport> {
        schema.validate_graph(graph)?;
        self.apply_schema(schema).await?;
        self.put_graph(graph).await
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>>;

    /// Reads multiple nodes by ID.
    ///
    /// The default implementation preserves the input order and calls
    /// [`GraphStore::get_node`] once per ID. Backends with a native batch-read
    /// path should override this to avoid per-node round trips during traversal
    /// and other fan-out reads.
    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        for id in ids {
            if let Some(node) = self.get_node(id).await? {
                nodes.push(node);
            }
        }
        Ok(nodes)
    }

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

/// A single incremental change to a graph, for delta-oriented pipelines.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GraphMutation {
    UpsertNode(Node),
    PatchNode {
        id: NodeId,
        props: Props,
    },
    DeleteNode(NodeId),
    UpsertEdge(Edge),
    DeleteEdge {
        from: NodeId,
        label: Label,
        to: NodeId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GraphMutationPlanKind {
    Create,
    Merge,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum GraphMutationPlanOp {
    UpsertNode {
        kind: GraphMutationPlanKind,
        node: Node,
    },
    PatchNode {
        id: NodeId,
        props: Props,
    },
    UpsertEdge {
        kind: GraphMutationPlanKind,
        edge: Edge,
    },
    DeleteNode(NodeId),
    DeleteEdge {
        from: NodeId,
        label: Label,
        to: NodeId,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphMutationPlan {
    pub operations: Vec<GraphMutationPlanOp>,
}

impl GraphMutationPlan {
    pub fn new(operations: Vec<GraphMutationPlanOp>) -> Self {
        Self { operations }
    }

    pub fn push(&mut self, operation: GraphMutationPlanOp) {
        self.operations.push(operation);
    }

    pub fn report(&self) -> GraphMutationReport {
        let mut report = GraphMutationReport::default();
        for operation in &self.operations {
            report.record(operation);
        }
        report
    }

    pub fn into_mutations(self) -> Vec<GraphMutation> {
        self.operations
            .into_iter()
            .map(GraphMutation::from)
            .collect()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct GraphMutationReport {
    pub creates: usize,
    pub merges: usize,
    pub deletes: usize,
    pub patches: usize,
    pub node_upserts: usize,
    pub edge_upserts: usize,
    pub node_deletes: usize,
    pub edge_deletes: usize,
    pub node_patches: usize,
}

impl GraphMutationReport {
    pub fn record(&mut self, operation: &GraphMutationPlanOp) {
        match operation {
            GraphMutationPlanOp::UpsertNode { kind, .. }
            | GraphMutationPlanOp::UpsertEdge { kind, .. } => {
                match kind {
                    GraphMutationPlanKind::Create => self.creates += 1,
                    GraphMutationPlanKind::Merge => self.merges += 1,
                }
                match operation {
                    GraphMutationPlanOp::UpsertNode { .. } => self.node_upserts += 1,
                    GraphMutationPlanOp::UpsertEdge { .. } => self.edge_upserts += 1,
                    _ => {}
                }
            }
            GraphMutationPlanOp::PatchNode { .. } => {
                self.patches += 1;
                self.node_patches += 1;
            }
            GraphMutationPlanOp::DeleteNode(_) => {
                self.deletes += 1;
                self.node_deletes += 1;
            }
            GraphMutationPlanOp::DeleteEdge { .. } => {
                self.deletes += 1;
                self.edge_deletes += 1;
            }
        }
    }
}

impl From<GraphMutationPlanOp> for GraphMutation {
    fn from(operation: GraphMutationPlanOp) -> Self {
        match operation {
            GraphMutationPlanOp::UpsertNode { node, .. } => Self::UpsertNode(node),
            GraphMutationPlanOp::PatchNode { id, props } => Self::PatchNode { id, props },
            GraphMutationPlanOp::UpsertEdge { edge, .. } => Self::UpsertEdge(edge),
            GraphMutationPlanOp::DeleteNode(id) => Self::DeleteNode(id),
            GraphMutationPlanOp::DeleteEdge { from, label, to } => {
                Self::DeleteEdge { from, label, to }
            }
        }
    }
}

/// Incremental mutation support for stores that can delete elements.
///
/// Deletes are idempotent: removing an element that does not exist is not an
/// error.
#[async_trait]
pub trait GraphMutationStore: GraphStore {
    /// Deletes a node and all edges incident to it.
    async fn delete_node(&self, id: &NodeId) -> Result<()>;

    /// Deletes the edge(s) matching `(from, label, to)`.
    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()>;

    /// Applies mutations in order, stopping at the first error.
    ///
    /// The default implementation calls the single-mutation methods one at a
    /// time and is not atomic: if a later mutation fails, earlier successful
    /// mutations are not rolled back. Backends with transaction support should
    /// override this method and apply the whole slice in one transaction.
    async fn apply_mutations(&self, mutations: &[GraphMutation]) -> Result<()> {
        for mutation in mutations {
            match mutation {
                GraphMutation::UpsertNode(node) => {
                    self.put_node(node).await?;
                }
                GraphMutation::PatchNode { id, props } => {
                    if let Some(mut node) = self.get_node(id).await? {
                        for (key, value) in props {
                            node.props.insert(key.clone(), value.clone());
                        }
                        self.put_node(&node).await?;
                    }
                }
                GraphMutation::DeleteNode(id) => self.delete_node(id).await?,
                GraphMutation::UpsertEdge(edge) => {
                    self.put_edge(edge).await?;
                }
                GraphMutation::DeleteEdge { from, label, to } => {
                    self.delete_edge(from, label, to).await?
                }
            }
        }
        Ok(())
    }
}

pub mod prelude {
    pub use crate::{
        Direction, Edge, EdgeId, EdgePolicy, EdgeQuery, EdgeType, EdgeUniqueness, Field, FieldType,
        Graph, GraphAdminStore, GraphBuilder, GraphIndex, GraphMutation, GraphMutationPlan,
        GraphMutationPlanKind, GraphMutationPlanOp, GraphMutationReport, GraphMutationStore,
        GraphSchema, GraphSchemaBuilder, GraphStore, GrustError, Label, LoadReport, Node, NodeId,
        NodeType, Props, PutOutcome, Result, RfcDate, Start, Step, Traversal, Value, edge_key,
        relationship_type, schema_identifier,
    };

    #[cfg(feature = "typed-garde")]
    pub use crate::typed::{TypedEdge, TypedGraphBuilder, TypedNode, garde, props_from_serialize};

    #[cfg(feature = "typed-zod-rs")]
    pub use crate::typed::{parse_typed_json, parse_typed_json_with, zod_rs};
}

#[cfg(test)]
mod tests;
