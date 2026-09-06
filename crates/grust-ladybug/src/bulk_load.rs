//! Bulk loading through registered Arrow tables.
//!
//! One `MERGE` per node or edge costs the engine a full statement each,
//! measured at about 50 ms of CPU for a relationship `MERGE … SET` on a 4-vCPU
//! host, so a 200,000-edge slice took five hours. The `lbug` crate can register
//! Arrow record batches as tables; `COPY … FROM (MATCH …)` over such a table
//! loads the same 100,000 nodes in a quarter of a second and 112,000 edges in
//! eight.
//!
//! `put_graph` keeps its upsert semantics: rows whose node id, or whose
//! `(from, to)` pair for a relationship table, already exists go through the
//! per-row `MERGE` path exactly as before; every other row is copied. Within
//! one load the last row for an id or pair wins, which is what a sequence of
//! `MERGE`s would leave behind.

use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, atomic::AtomicU64},
};

use arrow::{
    array::{ArrayRef, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use grust_core::prelude::*;

use super::{LadybugGraphStore, ladybug_error, props_to_string};

/// A registered Arrow table's name is unique per process so two loads on the
/// same connection never collide.
static SCRATCH_TABLES: AtomicU64 = AtomicU64::new(0);

impl LadybugGraphStore {
    /// Write `nodes` into `table`, copying rows whose id is new and merging
    /// the rest. Returns how many rows were written.
    pub(super) fn bulk_write_nodes_locked(
        &self,
        conn: &lbug::Connection<'_>,
        table: &str,
        nodes: &[&Node],
    ) -> Result<usize> {
        let existing = self.existing_node_ids(conn, table)?;
        let mut fresh: BTreeMap<&str, &Node> = BTreeMap::new();
        for node in nodes {
            if existing.contains(node.id.as_str()) {
                self.write_node_locked(conn, node, table)?;
            } else {
                fresh.insert(node.id.as_str(), node);
            }
        }
        if fresh.is_empty() {
            return Ok(nodes.len());
        }
        let ids: Vec<&str> = fresh.keys().copied().collect();
        let props = fresh
            .values()
            .map(|node| props_to_string(&node.props))
            .collect::<Result<Vec<_>>>()?;
        let batch = string_batch(&[
            ("id", ids.clone()),
            ("props", props.iter().map(String::as_str).collect()),
        ])?;
        self.copy_from_arrow_nodes(conn, table, batch)?;
        self.bulk_write_node_index(conn, table, &fresh)?;
        Ok(nodes.len())
    }

    /// Write `edges` into `rel_table`, copying pairs that are new and merging
    /// the rest. Returns how many rows were written.
    pub(super) fn bulk_write_edges_locked(
        &self,
        conn: &lbug::Connection<'_>,
        rel_table: &str,
        from_table: &str,
        to_table: &str,
        edges: &[&Edge],
    ) -> Result<usize> {
        let existing = self.existing_edge_pairs(conn, rel_table, from_table, to_table)?;
        let mut fresh: BTreeMap<(&str, &str), &Edge> = BTreeMap::new();
        for edge in edges {
            let pair = (edge.from.as_str(), edge.to.as_str());
            if existing.contains(&(pair.0.to_string(), pair.1.to_string())) {
                self.write_edge_locked(conn, edge, rel_table, from_table, to_table)?;
            } else {
                fresh.insert(pair, edge);
            }
        }
        if fresh.is_empty() {
            return Ok(edges.len());
        }
        let ids = fresh
            .values()
            .map(|edge| checked_edge_key(edge))
            .collect::<Result<Vec<_>>>()?;
        let props = fresh
            .values()
            .map(|edge| props_to_string(&edge.props))
            .collect::<Result<Vec<_>>>()?;
        let batch = string_batch(&[
            ("from", fresh.keys().map(|(from, _)| *from).collect()),
            ("to", fresh.keys().map(|(_, to)| *to).collect()),
            ("id", ids.iter().map(String::as_str).collect()),
            ("props", props.iter().map(String::as_str).collect()),
        ])?;
        let scratch = scratch_table_name(&self.config.table_prefix);
        conn.create_arrow_rel_table(&scratch, &[batch], from_table, to_table)
            .map_err(ladybug_error)?;
        let copy = Self::exec(
            conn,
            &format!(
                "COPY {rel_table} FROM (MATCH (a:{from_table})-[r:{scratch}]->(b:{to_table}) RETURN a.id, b.id, r.id, r.props);"
            ),
        );
        conn.drop_arrow_table(&scratch).map_err(ladybug_error)?;
        copy?;
        Ok(edges.len())
    }

    fn copy_from_arrow_nodes(
        &self,
        conn: &lbug::Connection<'_>,
        table: &str,
        batch: RecordBatch,
    ) -> Result<()> {
        let scratch = scratch_table_name(&self.config.table_prefix);
        conn.create_arrow_table(&scratch, &[batch])
            .map_err(ladybug_error)?;
        let copy = Self::exec(
            conn,
            &format!("COPY {table} FROM (MATCH (n:{scratch}) RETURN n.id, n.props);"),
        );
        conn.drop_arrow_table(&scratch).map_err(ladybug_error)?;
        copy
    }

    /// The metadata index gets one row per new node, copied the same way.
    fn bulk_write_node_index(
        &self,
        conn: &lbug::Connection<'_>,
        table: &str,
        fresh: &BTreeMap<&str, &Node>,
    ) -> Result<()> {
        let (node_index, _) = self.metadata_tables()?;
        let labels: Vec<&str> = fresh.values().map(|node| node.label.as_str()).collect();
        let batch = string_batch(&[
            ("id", fresh.keys().copied().collect()),
            ("kind", vec!["node"; fresh.len()]),
            ("label", labels),
            ("table_name", vec![table; fresh.len()]),
        ])?;
        let scratch = scratch_table_name(&self.config.table_prefix);
        conn.create_arrow_table(&scratch, &[batch])
            .map_err(ladybug_error)?;
        let copy = Self::exec(
            conn,
            &format!(
                "COPY {node_index} FROM (MATCH (n:{scratch}) RETURN n.id, n.kind, n.label, n.table_name);"
            ),
        );
        conn.drop_arrow_table(&scratch).map_err(ladybug_error)?;
        copy
    }

    fn existing_node_ids(
        &self,
        conn: &lbug::Connection<'_>,
        table: &str,
    ) -> Result<HashSet<String>> {
        let rows = conn
            .query(&format!("MATCH (n:{table}) RETURN n.id;"))
            .map_err(ladybug_error)?;
        rows.map(|row| super::row_string(&row, 0, "node id"))
            .collect()
    }

    fn existing_edge_pairs(
        &self,
        conn: &lbug::Connection<'_>,
        rel_table: &str,
        from_table: &str,
        to_table: &str,
    ) -> Result<HashSet<(String, String)>> {
        let rows = conn
            .query(&format!(
                "MATCH (a:{from_table})-[r:{rel_table}]->(b:{to_table}) RETURN a.id, b.id;"
            ))
            .map_err(ladybug_error)?;
        rows.map(|row| {
            Ok((
                super::row_string(&row, 0, "edge")?,
                super::row_string(&row, 1, "edge")?,
            ))
        })
        .collect()
    }
}

fn scratch_table_name(prefix: &str) -> String {
    let n = SCRATCH_TABLES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{prefix}_arrow_in_{}_{n}", std::process::id())
}

/// A record batch of non-null UTF-8 columns.
fn string_batch(columns: &[(&str, Vec<&str>)]) -> Result<RecordBatch> {
    let schema = Arc::new(Schema::new(
        columns
            .iter()
            .map(|(name, _)| Field::new(*name, DataType::Utf8, false))
            .collect::<Vec<_>>(),
    ));
    let arrays: Vec<ArrayRef> = columns
        .iter()
        .map(|(_, values)| Arc::new(StringArray::from(values.clone())) as ArrayRef)
        .collect();
    RecordBatch::try_new(schema, arrays)
        .map_err(|err| GrustError::Serialization(format!("Ladybug Arrow batch error: {err}")))
}
