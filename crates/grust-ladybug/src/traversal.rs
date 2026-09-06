//! Traversal steps as one query per relationship table.
//!
//! The previous walk fetched a vertex's edges and then looked every target
//! node up by primary key, one prepared statement each: 12,215 statements and
//! 19.5 s for wiki-Talk's hub. A step is now one `MATCH (a)-[r]->(b) WHERE
//! a.id = $id RETURN b.id, b.props` per relationship table that can carry it,
//! which the engine answers in milliseconds, and `traverse_ids` asks for the
//! ids alone.

use std::collections::BTreeMap;

use grust_core::prelude::*;

use super::{LadybugGraphStore, RelTable, ladybug_error, props_from_string, row_string};

/// What a step wants back for each neighbour.
#[derive(Clone, Copy)]
enum Want {
    Nodes,
    Ids,
}

impl LadybugGraphStore {
    /// Every vertex `traversal` reaches, as `GraphStore::traverse` returns it.
    pub(super) fn traverse_locked(
        &self,
        conn: &lbug::Connection<'_>,
        traversal: Traversal,
    ) -> Result<Vec<Node>> {
        self.bootstrap_locked(conn)?;
        let mut current = self.start_nodes(conn, traversal.start)?;
        let tables = self.rel_tables(conn)?;
        for step in &traversal.steps {
            let mut next = BTreeMap::new();
            for node in &current {
                for neighbour in self.step_neighbours(conn, &tables, &node.id, step, Want::Nodes)? {
                    let id = neighbour.id.clone();
                    next.insert(id, neighbour);
                }
            }
            current = next.into_values().collect();
        }
        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }
        Ok(current)
    }

    /// The ids of the vertices `traversal` reaches, in the same order as
    /// `traverse_locked`, without decoding their properties.
    pub(super) fn traverse_ids_locked(
        &self,
        conn: &lbug::Connection<'_>,
        traversal: Traversal,
    ) -> Result<Vec<NodeId>> {
        self.bootstrap_locked(conn)?;
        let mut current: Vec<NodeId> = self
            .start_nodes(conn, traversal.start)?
            .into_iter()
            .map(|node| node.id)
            .collect();
        let tables = self.rel_tables(conn)?;
        for step in &traversal.steps {
            let mut next = BTreeMap::new();
            for id in &current {
                for neighbour in self.step_neighbours(conn, &tables, id, step, Want::Ids)? {
                    next.insert(neighbour.id.clone(), ());
                }
            }
            current = next.into_keys().collect();
        }
        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }
        Ok(current)
    }

    /// The neighbours of `id` for one step: one query per relationship table
    /// whose label and endpoint labels can satisfy the step.
    fn step_neighbours(
        &self,
        conn: &lbug::Connection<'_>,
        tables: &[RelTable],
        id: &NodeId,
        step: &Step,
        want: Want,
    ) -> Result<Vec<Node>> {
        let mut found = Vec::new();
        for table in tables {
            if step
                .edge
                .as_ref()
                .is_some_and(|label| label != &table.label)
            {
                continue;
            }
            let from_table = self.node_table_name(&table.from_label)?;
            let to_table = self.node_table_name(&table.to_label)?;
            let accepts = |label: &Label| step.node.as_ref().is_none_or(|wanted| wanted == label);
            if matches!(step.direction, Direction::Out | Direction::Both)
                && accepts(&table.to_label)
            {
                let cypher = format!(
                    "MATCH (a:{from_table})-[r:{}]->(b:{to_table}) WHERE a.id = $id RETURN {};",
                    table.table,
                    columns("b", want)
                );
                found.extend(self.neighbour_rows(conn, &cypher, id, &table.to_label, want)?);
            }
            if matches!(step.direction, Direction::In | Direction::Both)
                && accepts(&table.from_label)
            {
                let cypher = format!(
                    "MATCH (a:{from_table})-[r:{}]->(b:{to_table}) WHERE b.id = $id RETURN {};",
                    table.table,
                    columns("a", want)
                );
                found.extend(self.neighbour_rows(conn, &cypher, id, &table.from_label, want)?);
            }
        }
        Ok(found)
    }

    fn neighbour_rows(
        &self,
        conn: &lbug::Connection<'_>,
        cypher: &str,
        id: &NodeId,
        label: &Label,
        want: Want,
    ) -> Result<Vec<Node>> {
        let mut statement = conn.prepare(cypher).map_err(ladybug_error)?;
        let rows = conn
            .execute(
                &mut statement,
                vec![("id", lbug::Value::String(id.as_str().to_string()))],
            )
            .map_err(ladybug_error)?;
        rows.map(|row| {
            let props = match want {
                Want::Nodes => props_from_string(&row_string(&row, 1, "neighbour")?)?,
                Want::Ids => Props::default(),
            };
            Ok(Node {
                id: NodeId::from(row_string(&row, 0, "neighbour")?),
                label: label.clone(),
                props,
            })
        })
        .collect()
    }
}

fn columns(alias: &str, want: Want) -> String {
    match want {
        Want::Nodes => format!("{alias}.id, {alias}.props"),
        Want::Ids => format!("{alias}.id"),
    }
}
