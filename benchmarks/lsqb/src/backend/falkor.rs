use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grust_core::{Edge, Graph, Value, relationship_type};
use redis::Value as RedisValue;

use super::QueryExecutionError;
use crate::queries::QueryCase;

const REDIS_IO_GRACE_MS: u64 = 2_000;

const PHYSICAL_NODE_LABELS: &[(&str, &str)] = &[
    ("Company", "company"),
    ("University", "university"),
    ("Continent", "continent"),
    ("Country", "country"),
    ("City", "city"),
    ("Tag", "tag"),
    ("TagClass", "tagclass"),
    ("Forum", "forum"),
    ("Message", "message"),
    ("Person", "person"),
    ("Post", "post"),
    ("Comment", "comment"),
    ("Entity", "entity"),
];

/// Clones the logical LSQB graph into the representation used by
/// `FalkorGraphStore` while retaining Message inheritance as real FalkorDB
/// labels.
///
/// Grust has one primary node label. FalkorDB supports multiple labels, and
/// its adapter reserves the `labels` string-array property for that purpose.
/// `Entity` gives the benchmark loader one common indexed lookup label for
/// edge endpoints. Adding `Post` or `Comment` alongside `Message` lets native
/// openCypher run the source query shapes without duplicating nodes.
pub fn prepare_graph(graph: &Graph) -> Result<Graph, String> {
    let mut prepared = graph.clone();
    for node in &mut prepared.nodes {
        let mut labels = vec!["Entity".to_string(), node.label.as_str().to_string()];
        if node.label.as_str() == "Message" {
            let kind = node
                .props
                .get("kind")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "Falkor native layout requires Message node '{}' to have a string kind",
                        node.id.as_str()
                    )
                })?
                .to_string();
            if !matches!(kind.as_str(), "Post" | "Comment") {
                return Err(format!(
                    "Falkor native layout requires Message node '{}' kind to be Post or Comment, got {kind:?}",
                    node.id.as_str()
                ));
            }
            labels.push(kind);
        }
        node.props
            .insert("labels".to_string(), Value::StringArray(labels));
    }
    Ok(prepared)
}

/// Adapts an executable LSQB count to FalkorDB's physical labels and native
/// cardinality semantics while preserving relationship types and property
/// identifiers.
pub fn adapt_query(case: &QueryCase) -> String {
    adapt_executable(&case.executable)
}

fn adapt_executable(query: &str) -> String {
    let query = restore_native_pattern_predicates(query);
    let query = name_anonymous_nodes(&query);
    let query = normalize_unicode_escapes(&query);
    let mut query = adapt_cypher(&query);

    // FalkorDB 4.20 can push count(*) below later pattern expansions.  A
    // projection boundary retains the complete binding row and produces the
    // openCypher cardinality observed when those rows are returned directly.
    // UNION already supplies its own projection boundaries and must retain
    // its aggregate-per-arm semantics.
    if !query.contains("UNION")
        && let Some(position) = query.rfind("RETURN count(*) AS ")
    {
        query.insert_str(position, "WITH *\n");
    }
    query
}

fn restore_native_pattern_predicates(query: &str) -> String {
    query
        .replace(
            "OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1)\nWITH tag1, tag2, h\nWHERE h IS NULL AND tag1 <> tag2",
            "WHERE NOT (comment)-[:HAS_TAG]->(tag1)\n  AND tag1 <> tag2",
        )
        .replace(
            "OPTIONAL MATCH (person1)-[k:KNOWS]-(person3)\nWITH person1, person3, tag, k\nWHERE k IS NULL AND person1 <> person3",
            "WHERE NOT (person1)-[:KNOWS]-(person3)\n  AND person1 <> person3",
        )
}

fn name_anonymous_nodes(query: &str) -> String {
    let mut output = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    let mut quoted = None;
    let mut escaped = false;
    let mut index = 0_usize;

    while let Some(ch) = chars.next() {
        output.push(ch);
        if let Some(quote) = quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                quoted = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quoted = Some(ch);
            continue;
        }
        if ch == '(' && chars.peek().is_some_and(|next| *next == ':') {
            output.push_str(&format!("_falkor_anon_{index}"));
            index += 1;
        }
    }
    output
}

fn normalize_unicode_escapes(query: &str) -> String {
    let mut output = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    let mut quoted = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if let Some(quote) = quoted {
            if ch == '\\' && !escaped && chars.peek().is_some_and(|next| *next == 'u') {
                let mut probe = chars.clone();
                probe.next();
                let digits = probe.by_ref().take(4).collect::<String>();
                if digits.len() == 4
                    && digits.chars().all(|digit| digit.is_ascii_hexdigit())
                    && let Ok(codepoint) = u32::from_str_radix(&digits, 16)
                    && let Some(decoded) = char::from_u32(codepoint)
                {
                    for _ in 0..5 {
                        chars.next();
                    }
                    output.push(decoded);
                    continue;
                }
            }

            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                quoted = None;
            }
            continue;
        }

        output.push(ch);
        if matches!(ch, '\'' | '"') {
            quoted = Some(ch);
        }
    }
    output
}

pub fn adapt_cypher(query: &str) -> String {
    let mut output = String::with_capacity(query.len());
    let mut chars = query.chars().peekable();
    let mut quoted = None;
    let mut escaped = false;
    let mut square_depth = 0_usize;

    while let Some(ch) = chars.next() {
        if let Some(quote) = quoted {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                quoted = None;
            }
            continue;
        }

        if matches!(ch, '\'' | '"' | '`') {
            quoted = Some(ch);
            output.push(ch);
            continue;
        }

        if ch == '[' {
            square_depth = square_depth.saturating_add(1);
            output.push(ch);
            continue;
        }
        if ch == ']' {
            square_depth = square_depth.saturating_sub(1);
            output.push(ch);
            continue;
        }

        if ch != ':' {
            output.push(ch);
            continue;
        }

        output.push(':');
        if square_depth == 0 && chars.peek().is_some_and(|ch| *ch == '`') {
            chars.next();
            let mut identifier = String::new();
            for ch in chars.by_ref() {
                if ch == '`' {
                    break;
                }
                identifier.push(ch);
            }
            let physical = PHYSICAL_NODE_LABELS.iter().find_map(|(logical, physical)| {
                (*logical == identifier.as_str()).then_some(*physical)
            });
            output.push('`');
            output.push_str(physical.unwrap_or(identifier.as_str()));
            output.push('`');
            continue;
        }
        let mut token = String::new();
        while chars
            .peek()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        {
            token.push(chars.next().expect("peeked Cypher label character"));
        }
        let physical = (square_depth == 0).then(|| {
            PHYSICAL_NODE_LABELS
                .iter()
                .find_map(|(logical, physical)| (*logical == token.as_str()).then_some(*physical))
        });
        let physical = physical.flatten();
        output.push_str(physical.unwrap_or(token.as_str()));
    }

    output
}

/// A persistent Redis connection for timed FalkorDB native reads.
///
/// Connection establishment belongs outside the timed query interval. Each
/// call executes exactly one read-only native query and validates the complete
/// one-row, one-count result contract.
pub struct FalkorNativeClient {
    client: redis::Client,
    connection: redis::Connection,
    graph: String,
}

pub type SharedFalkorNativeClient = Arc<Mutex<FalkorNativeClient>>;

impl FalkorNativeClient {
    pub fn connect(redis_url: &str, graph: impl Into<String>) -> Result<Self, String> {
        let graph = graph.into();
        if graph.is_empty() {
            return Err("FalkorDB graph name must not be empty".to_string());
        }
        let client = redis::Client::open(redis_url)
            .map_err(|_| "cannot configure the FalkorDB Redis client".to_string())?;
        let connection = client
            .get_connection()
            .map_err(|_| "cannot connect to FalkorDB".to_string())?;
        Ok(Self {
            client,
            connection,
            graph,
        })
    }

    pub fn execute_count(
        &mut self,
        query: &str,
        timeout_ms: u64,
    ) -> Result<i64, QueryExecutionError> {
        let io_timeout = Duration::from_millis(timeout_ms.saturating_add(REDIS_IO_GRACE_MS));
        self.connection
            .set_read_timeout(Some(io_timeout))
            .map_err(|error| {
                QueryExecutionError::Error(format!(
                    "cannot set FalkorDB Redis read deadline: {error}"
                ))
            })?;
        self.connection
            .set_write_timeout(Some(io_timeout))
            .map_err(|error| {
                QueryExecutionError::Error(format!(
                    "cannot set FalkorDB Redis write deadline: {error}"
                ))
            })?;
        let response = redis::cmd("GRAPH.RO_QUERY")
            .arg(&self.graph)
            .arg(query)
            .arg("TIMEOUT")
            .arg(timeout_ms)
            .query::<RedisValue>(&mut self.connection);
        match response {
            Ok(response) => parse_count_response(&response).map_err(QueryExecutionError::Error),
            Err(error) if is_query_timeout(&error) => {
                self.reconnect_after_timeout()?;
                Err(QueryExecutionError::Timeout(format!(
                    "FalkorDB query exceeded the {timeout_ms} ms server cutoff; the request was reaped within the {} ms Redis socket deadline",
                    timeout_ms.saturating_add(REDIS_IO_GRACE_MS)
                )))
            }
            Err(error) => Err(QueryExecutionError::Error(format!(
                "FalkorDB GRAPH.RO_QUERY failed: {error}"
            ))),
        }
    }

    fn reconnect_after_timeout(&mut self) -> Result<(), QueryExecutionError> {
        self.connection = self.client.get_connection().map_err(|_| {
            QueryExecutionError::Error(
                "FalkorDB query timed out and the Redis connection could not be re-established"
                    .to_string(),
            )
        })?;
        Ok(())
    }

    pub fn create_entity_index(&mut self) -> Result<(), String> {
        self.write_query("CREATE INDEX FOR (entity:entity) ON (entity.id)")
    }

    pub fn put_edges(&mut self, edges: &[Edge]) -> Result<(), String> {
        let mut by_relationship = BTreeMap::<String, Vec<&Edge>>::new();
        for edge in edges {
            if !edge.props.is_empty() {
                return Err(format!(
                    "Falkor LSQB native edge loader does not accept properties on {:?}",
                    edge.label.as_str()
                ));
            }
            by_relationship
                .entry(relationship_type(edge.label.as_str()))
                .or_default()
                .push(edge);
        }
        for (relationship, edges) in by_relationship {
            for chunk in edges.chunks(1_000) {
                self.write_query(&edge_batch_query(&relationship, chunk))?;
            }
        }
        Ok(())
    }

    pub fn into_shared(self) -> SharedFalkorNativeClient {
        Arc::new(Mutex::new(self))
    }

    fn write_query(&mut self, query: &str) -> Result<(), String> {
        redis::cmd("GRAPH.QUERY")
            .arg(&self.graph)
            .arg(query)
            .query::<RedisValue>(&mut self.connection)
            .map(|_| ())
            .map_err(|error| format!("FalkorDB GRAPH.QUERY load step failed: {error}"))
    }
}

fn edge_batch_query(relationship: &str, edges: &[&Edge]) -> String {
    let rows = edges
        .iter()
        .map(|edge| {
            format!(
                "{{from:{},to:{}}}",
                cypher_string(edge.from.as_str()),
                cypher_string(edge.to.as_str())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "UNWIND [{rows}] AS row \
         MATCH (a:entity {{`id`:row.from}}), (b:entity {{`id`:row.to}}) \
         CREATE (a)-[:{relationship}]->(b)"
    )
}

fn cypher_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('\'');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('\'');
    escaped
}

/// Runs the blocking Redis request away from Tokio's scheduler while reusing
/// the connection established during backend preparation.
pub async fn execute_count(
    client: &SharedFalkorNativeClient,
    case: &QueryCase,
    timeout_ms: u64,
) -> Result<i64, QueryExecutionError> {
    let client = Arc::clone(client);
    let query = adapt_query(case);
    tokio::task::spawn_blocking(move || {
        let mut client = client.lock().map_err(|_| {
            QueryExecutionError::Error("FalkorDB native client lock is poisoned".to_string())
        })?;
        client.execute_count(&query, timeout_ms)
    })
    .await
    .map_err(|error| QueryExecutionError::Error(format!("FalkorDB query worker failed: {error}")))?
}

fn is_query_timeout(error: &redis::RedisError) -> bool {
    if error.is_timeout() {
        return true;
    }
    let message = error.to_string().to_ascii_lowercase();
    message.contains("query timed out") || message.contains("query timeout")
}

fn parse_count_response(response: &RedisValue) -> Result<i64, String> {
    let response = without_attributes(response);
    if let RedisValue::Map(entries) = response {
        return count_from_map(entries);
    }

    let sections = response.as_sequence().ok_or_else(|| {
        format!(
            "FalkorDB count response must be an array, got {}",
            redis_value_kind(response)
        )
    })?;
    if sections.len() < 2 {
        return Err(format!(
            "FalkorDB count response has {} section(s); expected headers and rows",
            sections.len()
        ));
    }

    let headers = without_attributes(&sections[0])
        .as_sequence()
        .ok_or_else(|| "FalkorDB count response headers are not an array".to_string())?;
    let count_column = headers
        .iter()
        .position(|header| redis_text(header).is_some_and(|header| header == "count"))
        .or_else(|| (headers.len() == 1).then_some(0))
        .ok_or_else(|| {
            "FalkorDB count response has multiple columns and no 'count' column".to_string()
        })?;

    let rows = without_attributes(&sections[1])
        .as_sequence()
        .ok_or_else(|| "FalkorDB count response rows are not an array".to_string())?;
    if rows.len() != 1 {
        return Err(format!(
            "FalkorDB count query returned {} rows; expected exactly one",
            rows.len()
        ));
    }

    let row = without_attributes(&rows[0]);
    if let RedisValue::Map(entries) = row {
        let header = headers.get(count_column).and_then(redis_text);
        return scalar_from_row_map(entries, header);
    }
    let cells = row
        .as_sequence()
        .ok_or_else(|| "FalkorDB count result row is not an array".to_string())?;
    let cell = cells.get(count_column).ok_or_else(|| {
        format!(
            "FalkorDB count result row has {} cells but count is column {}",
            cells.len(),
            count_column
        )
    })?;
    redis_i64(cell)
}

fn scalar_from_row_map(
    entries: &[(RedisValue, RedisValue)],
    header: Option<&str>,
) -> Result<i64, String> {
    if let Some(header) = header {
        for (key, value) in entries {
            if redis_text(key).is_some_and(|key| key == header) {
                return redis_i64(value);
            }
        }
    }
    if entries.len() == 1 {
        return redis_i64(&entries[0].1);
    }
    Err("FalkorDB map result row has no scalar count column".to_string())
}

fn count_from_map(entries: &[(RedisValue, RedisValue)]) -> Result<i64, String> {
    for (key, value) in entries {
        if redis_text(key).is_some_and(|key| key == "count") {
            return redis_i64(value);
        }
    }
    for (key, value) in entries {
        if redis_text(key).is_some_and(|key| matches!(key, "data" | "result" | "results" | "rows"))
            && let Ok(count) = parse_count_response(value)
        {
            return Ok(count);
        }
    }
    Err("FalkorDB map response has no 'count' value".to_string())
}

fn without_attributes(value: &RedisValue) -> &RedisValue {
    match value {
        RedisValue::Attribute { data, .. } => without_attributes(data),
        value => value,
    }
}

fn redis_text(value: &RedisValue) -> Option<&str> {
    match without_attributes(value) {
        RedisValue::BulkString(bytes) => std::str::from_utf8(bytes).ok(),
        RedisValue::SimpleString(value) => Some(value),
        RedisValue::VerbatimString { text, .. } => Some(text),
        _ => None,
    }
}

fn redis_i64(value: &RedisValue) -> Result<i64, String> {
    let value = without_attributes(value);
    match value {
        RedisValue::Int(value) => Ok(*value),
        RedisValue::BulkString(bytes) => std::str::from_utf8(bytes)
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| "FalkorDB count cell is not an integer".to_string()),
        RedisValue::SimpleString(value) | RedisValue::VerbatimString { text: value, .. } => value
            .parse::<i64>()
            .map_err(|_| "FalkorDB count cell is not an integer".to_string()),
        RedisValue::Double(value)
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= i64::MIN as f64
                && *value <= i64::MAX as f64 =>
        {
            Ok(*value as i64)
        }
        value => Err(format!(
            "FalkorDB count cell must be an integer, got {}",
            redis_value_kind(value)
        )),
    }
}

fn redis_value_kind(value: &RedisValue) -> &'static str {
    match value {
        RedisValue::Nil => "nil",
        RedisValue::Int(_) => "integer",
        RedisValue::BulkString(_) => "bulk string",
        RedisValue::Array(_) => "array",
        RedisValue::SimpleString(_) => "simple string",
        RedisValue::Okay => "ok",
        RedisValue::Map(_) => "map",
        RedisValue::Attribute { .. } => "attribute",
        RedisValue::Set(_) => "set",
        RedisValue::Double(_) => "double",
        RedisValue::Boolean(_) => "boolean",
        RedisValue::VerbatimString { .. } => "verbatim string",
        RedisValue::BigNumber(_) => "big number",
        RedisValue::Push { .. } => "push",
        RedisValue::ServerError(_) => "server error",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;

    use grust_core::{Node, Props};

    use super::*;

    fn message(id: &str, kind: Option<Value>) -> Node {
        let mut props = Props::new();
        if let Some(kind) = kind {
            props.insert("kind".to_string(), kind);
        }
        Node::new("Message", id, props)
    }

    fn bulk(value: &str) -> RedisValue {
        RedisValue::BulkString(value.as_bytes().to_vec())
    }

    #[test]
    fn prepares_message_inheritance_without_mutating_the_source() {
        let graph = Graph::new(
            vec![
                message("Post:1", Some(Value::from("Post"))),
                message("Comment:2", Some(Value::from("Comment"))),
                Node::new("Person", "Person:3", BTreeMap::new()),
            ],
            Vec::new(),
        );

        let prepared = prepare_graph(&graph).unwrap();

        assert_eq!(
            prepared.nodes[0].props.get("labels"),
            Some(&Value::StringArray(vec![
                "Entity".into(),
                "Message".into(),
                "Post".into()
            ]))
        );
        assert_eq!(
            prepared.nodes[1].props.get("labels"),
            Some(&Value::StringArray(vec![
                "Entity".into(),
                "Message".into(),
                "Comment".into()
            ]))
        );
        assert_eq!(
            prepared.nodes[2].props.get("labels"),
            Some(&Value::StringArray(vec!["Entity".into(), "Person".into()]))
        );
        assert!(!graph.nodes[0].props.contains_key("labels"));
    }

    #[test]
    fn rejects_messages_without_a_known_source_kind() {
        let missing = Graph::new(vec![message("Message:1", None)], Vec::new());
        assert!(prepare_graph(&missing).unwrap_err().contains("string kind"));

        let other = Graph::new(
            vec![message("Message:2", Some(Value::from("Article")))],
            Vec::new(),
        );
        assert!(
            prepare_graph(&other)
                .unwrap_err()
                .contains("Post or Comment")
        );
    }

    #[test]
    fn edge_batches_use_the_common_indexed_endpoint_label() {
        let edge = Edge::new("KNOWS", "Person:one'quoted", "Person:two", BTreeMap::new());
        let query = edge_batch_query("KNOWS", &[&edge]);
        assert!(query.contains("MATCH (a:entity {`id`:row.from}), (b:entity {`id`:row.to})"));
        assert!(query.contains("CREATE (a)-[:KNOWS]->(b)"));
        assert!(query.contains("Person:one\\'quoted"));
    }

    #[test]
    fn adapts_only_unquoted_lsqb_node_label_tokens() {
        let source = "MATCH (p:Person)-[r:Person]->(m:Message {kind: 'Post'}), \
                      (t:TagClass) WHERE m.note = ':Person' RETURN count(*) AS count";
        assert_eq!(
            adapt_cypher(source),
            "MATCH (p:person)-[r:Person]->(m:message {kind: 'Post'}), \
             (t:tagclass) WHERE m.note = ':Person' RETURN count(*) AS count"
        );
    }

    #[test]
    fn names_anonymous_nodes_and_adds_a_cardinality_barrier() {
        let source = "MATCH (:Country)<-[:IS_PART_OF]-(:City)\nRETURN count(*) AS count\n";
        assert_eq!(
            adapt_executable(source),
            "MATCH (_falkor_anon_0:country)<-[:IS_PART_OF]-(_falkor_anon_1:city)\n\
             WITH *\nRETURN count(*) AS count\n"
        );
    }

    #[test]
    fn restores_falkor_native_pattern_predicate_before_counting() {
        let source = "MATCH (person1:Person)-[:KNOWS]-(person3:Person)\n\
                      OPTIONAL MATCH (person1)-[k:KNOWS]-(person3)\n\
                      WITH person1, person3, tag, k\n\
                      WHERE k IS NULL AND person1 <> person3\n\
                      RETURN count(*) AS count";
        let adapted = adapt_executable(source);
        assert!(adapted.contains("WHERE NOT (person1)-[:KNOWS]-(person3)"));
        assert!(!adapted.contains("OPTIONAL MATCH"));
        assert!(adapted.ends_with("WITH *\nRETURN count(*) AS count"));
    }

    #[test]
    fn keeps_union_aggregate_boundaries_unchanged() {
        let source = "MATCH (p:Person) RETURN count(*) AS count\n\
                      UNION\n\
                      MATCH (p:Person) RETURN count(*) AS count";
        let adapted = adapt_executable(source);
        assert!(!adapted.contains("WITH *"));
        assert_eq!(adapted.matches("RETURN count(*) AS count").count(), 2);
    }

    #[test]
    fn adapts_quoted_node_labels_but_not_quoted_property_keys() {
        let source = "MATCH (person:`Person`) WHERE person.`missing-🧪` IS NULL \
                      RETURN count(*) AS count";
        assert_eq!(
            adapt_executable(source),
            "MATCH (person:`person`) WHERE person.`missing-🧪` IS NULL \
             WITH *\nRETURN count(*) AS count"
        );
    }

    #[test]
    fn normalizes_unicode_escapes_and_accepts_a_custom_scalar_alias() {
        let source = "MATCH (person:Person) WHERE 'é' = '\\u00e9' \
                      RETURN count(*) AS `résultat_🦀`";
        let adapted = adapt_executable(source);
        assert!(adapted.contains("WHERE 'é' = 'é'"));
        assert!(adapted.contains("WITH *\nRETURN count(*) AS `résultat_🦀`"));

        let response = RedisValue::Array(vec![
            RedisValue::Array(vec![bulk("résultat_🦀")]),
            RedisValue::Array(vec![RedisValue::Array(vec![RedisValue::Int(5)])]),
            RedisValue::Array(Vec::new()),
        ]);
        assert_eq!(parse_count_response(&response).unwrap(), 5);
    }

    #[test]
    fn reads_resp2_tabular_count() {
        let response = RedisValue::Array(vec![
            RedisValue::Array(vec![bulk("count")]),
            RedisValue::Array(vec![RedisValue::Array(vec![RedisValue::Int(42)])]),
            RedisValue::Array(vec![bulk(
                "Query internal execution time: 0.1 milliseconds",
            )]),
        ]);
        assert_eq!(parse_count_response(&response).unwrap(), 42);
    }

    #[test]
    fn reads_resp3_attributes_and_map_rows() {
        let response = RedisValue::Attribute {
            data: Box::new(RedisValue::Array(vec![
                RedisValue::Set(vec![RedisValue::SimpleString("count".to_string())]),
                RedisValue::Array(vec![RedisValue::Map(vec![(
                    RedisValue::SimpleString("count".to_string()),
                    RedisValue::BulkString(b"17".to_vec()),
                )])]),
            ])),
            attributes: Vec::new(),
        };
        assert_eq!(parse_count_response(&response).unwrap(), 17);
    }

    #[test]
    fn rejects_ambiguous_or_non_integer_results() {
        let no_rows = RedisValue::Array(vec![
            RedisValue::Array(vec![bulk("count")]),
            RedisValue::Array(Vec::new()),
        ]);
        assert!(
            parse_count_response(&no_rows)
                .unwrap_err()
                .contains("0 rows")
        );

        let text = RedisValue::Array(vec![
            RedisValue::Array(vec![bulk("count")]),
            RedisValue::Array(vec![RedisValue::Array(vec![bulk("not-a-count")])]),
        ]);
        assert!(
            parse_count_response(&text)
                .unwrap_err()
                .contains("not an integer")
        );
    }

    #[test]
    fn classifies_transport_and_server_query_timeouts() {
        let transport = redis::RedisError::from(io::Error::new(
            io::ErrorKind::TimedOut,
            "socket deadline elapsed",
        ));
        assert!(is_query_timeout(&transport));

        let server =
            redis::RedisError::from((redis::ErrorKind::Extension, "FalkorDB query timed out"));
        assert!(is_query_timeout(&server));

        let other = redis::RedisError::from((redis::ErrorKind::Extension, "syntax error"));
        assert!(!is_query_timeout(&other));
    }
}
