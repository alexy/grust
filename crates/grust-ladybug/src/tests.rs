use std::collections::BTreeMap;

use grust_core::prelude::*;

use crate::{LadybugConfig, LadybugGraphStore, LadybugPath};

fn props(entries: &[(&str, Value)]) -> Props {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect::<BTreeMap<_, _>>()
}

fn sample_graph() -> Graph {
    Graph::new(
        vec![
            Node::new(
                "Person",
                "person:ada",
                props(&[("name", Value::from("Ada"))]),
            ),
            Node::new(
                "Talk",
                "talk:grust",
                props(&[("title", Value::from("Grust"))]),
            ),
        ],
        vec![
            Edge::new(
                "Presented By",
                "person:ada",
                "talk:grust",
                props(&[("year", Value::from(2026_i64))]),
            )
            .with_id("edge:presented"),
        ],
    )
}

#[tokio::test]
async fn put_graph_reads_nodes_edges_and_traverses() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    store.bootstrap().await?;
    let graph = sample_graph();
    assert_eq!(
        store.put_graph(&graph).await?,
        LoadReport { nodes: 2, edges: 1 }
    );

    let node = store
        .get_node(&NodeId::from("person:ada"))
        .await?
        .expect("node should be readable");
    assert_eq!(node.label, Label::from("Person"));
    assert_eq!(node.props.get("name"), Some(&Value::from("Ada")));

    let edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::from("person:ada")),
            to: None,
            label: Some(Label::from("Presented By")),
        })
        .await?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to, NodeId::from("talk:grust"));
    assert_eq!(edges[0].props.get("year"), Some(&Value::from(2026_i64)));

    let traversed = store
        .traverse(
            Traversal::from_node("person:ada")
                .out("Presented By")
                .to("Talk"),
        )
        .await?;
    assert_eq!(traversed.len(), 1);
    assert_eq!(traversed[0].id, NodeId::from("talk:grust"));
    Ok(())
}

#[tokio::test]
async fn applies_schema_before_typed_graph_write() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    let schema = GraphSchema::builder()
        .node("Person", vec![])
        .node("Talk", vec![])
        .edge(
            "Presented By",
            vec![Label::from("Person")],
            vec![Label::from("Talk")],
            vec![],
        )
        .build();

    let report = store.put_typed_graph(&schema, &sample_graph()).await?;
    assert_eq!(report, LoadReport { nodes: 2, edges: 1 });
    Ok(())
}

#[tokio::test]
async fn persists_to_directory() -> Result<()> {
    let tempdir =
        tempfile::tempdir().map_err(|err| GrustError::Backend(format!("tempdir error: {err}")))?;
    let path = tempdir.path().join("ladybug");
    {
        let store = LadybugGraphStore::new(LadybugConfig {
            path: LadybugPath::Directory(path.clone()),
            ..LadybugConfig::default()
        })?;
        store.put_graph(&sample_graph()).await?;
    }
    {
        let store = LadybugGraphStore::open(path)?;
        let node = store.get_node(&NodeId::from("talk:grust")).await?;
        assert!(node.is_some());
    }
    Ok(())
}

#[tokio::test]
async fn clear_removes_managed_graph() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    store.put_graph(&sample_graph()).await?;
    store.clear().await?;
    assert!(store.get_node(&NodeId::from("person:ada")).await?.is_none());
    Ok(())
}

#[cfg(feature = "arrow")]
mod arrow_tests {
    use std::sync::Arc;

    use arrow::{
        array::{Array as _, Int64Array, StringArray},
        datatypes::{DataType, Field, Schema},
        ipc::{reader::StreamReader, writer::StreamWriter},
        record_batch::RecordBatch,
    };

    use super::*;

    fn arrow_err(context: &str, err: impl std::fmt::Display) -> GrustError {
        GrustError::Backend(format!("{context}: {err}"))
    }

    fn ipc_bytes(batch: &RecordBatch) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut data);
            let mut writer = StreamWriter::try_new(cursor, batch.schema().as_ref())
                .map_err(|err| arrow_err("Arrow IPC writer", err))?;
            writer
                .write(batch)
                .map_err(|err| arrow_err("Arrow IPC write", err))?;
            writer
                .finish()
                .map_err(|err| arrow_err("Arrow IPC finish", err))?;
        }
        Ok(data)
    }

    fn collect_string_column(chunks: &[Vec<u8>], column: usize) -> Result<Vec<String>> {
        let mut values = Vec::new();
        for chunk in chunks {
            let reader = StreamReader::try_new(std::io::Cursor::new(chunk), None)
                .map_err(|err| arrow_err("Arrow IPC reader", err))?;
            for batch in reader {
                let batch = batch.map_err(|err| arrow_err("Arrow batch", err))?;
                let array = batch
                    .column(column)
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| GrustError::Schema("query column is not string".into()))?;
                values.extend((0..array.len()).map(|index| array.value(index).to_string()));
            }
        }
        Ok(values)
    }

    fn collect_i64_column(chunks: &[Vec<u8>], column: usize) -> Result<Vec<i64>> {
        let mut values = Vec::new();
        for chunk in chunks {
            let reader = StreamReader::try_new(std::io::Cursor::new(chunk), None)
                .map_err(|err| arrow_err("Arrow IPC reader", err))?;
            for batch in reader {
                let batch = batch.map_err(|err| arrow_err("Arrow batch", err))?;
                let array = batch
                    .column(column)
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| GrustError::Schema("query column is not i64".into()))?;
                values.extend((0..array.len()).map(|index| array.value(index)));
            }
        }
        Ok(values)
    }

    #[test]
    fn arrow_ipc_node_table_queries_through_ladybug() -> Result<()> {
        let store = LadybugGraphStore::in_memory()?;
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(vec![1, 2])),
                Arc::new(StringArray::from(vec!["Ada", "Grace"])),
            ],
        )
        .map_err(|err| arrow_err("Arrow node batch", err))?;

        store.register_arrow_ipc_node_table("Person", &ipc_bytes(&batch)?)?;
        let chunks =
            store.query_arrow_ipc("MATCH (p:Person) RETURN p.name ORDER BY p.id;", 1024)?;

        assert_eq!(collect_string_column(&chunks, 0)?, vec!["Ada", "Grace"]);
        Ok(())
    }

    #[test]
    fn arrow_ipc_relationship_table_queries_through_ladybug() -> Result<()> {
        let store = LadybugGraphStore::in_memory()?;
        let node_schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let nodes = RecordBatch::try_new(node_schema, vec![Arc::new(Int64Array::from(vec![0, 1]))])
            .map_err(|err| arrow_err("Arrow node batch", err))?;
        store.register_arrow_ipc_node_table("Person", &ipc_bytes(&nodes)?)?;

        let rel_schema = Arc::new(Schema::new(vec![
            Field::new("from", DataType::Int64, false),
            Field::new("to", DataType::Int64, false),
            Field::new("weight", DataType::Int64, false),
        ]));
        let rels = RecordBatch::try_new(
            rel_schema,
            vec![
                Arc::new(Int64Array::from(vec![0, 1])),
                Arc::new(Int64Array::from(vec![1, 0])),
                Arc::new(Int64Array::from(vec![7, 9])),
            ],
        )
        .map_err(|err| arrow_err("Arrow relationship batch", err))?;
        store.register_arrow_ipc_rel_table("Knows", &ipc_bytes(&rels)?, "Person", "Person")?;

        let chunks = store.query_arrow_ipc(
            "MATCH (a:Person)-[r:Knows]->(b:Person) \
             RETURN r.weight ORDER BY a.id, b.id;",
            1024,
        )?;

        assert_eq!(collect_i64_column(&chunks, 0)?, vec![7, 9]);
        Ok(())
    }
}
