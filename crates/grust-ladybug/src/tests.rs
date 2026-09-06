use std::collections::BTreeMap;

use grust_core::prelude::*;

use crate::{LadybugConfig, LadybugGraphMode, LadybugGraphStore, LadybugPath};

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
    let store = LadybugGraphStore::new(LadybugConfig::untyped())?;
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

#[test]
fn ladybug_graph_mode_helpers_map_to_dynamic_schema() {
    assert_eq!(
        LadybugConfig::untyped().graph_mode(),
        LadybugGraphMode::Untyped
    );
    assert!(LadybugConfig::untyped().dynamic_schema);
    assert_eq!(LadybugConfig::typed().graph_mode(), LadybugGraphMode::Typed);
    assert!(!LadybugConfig::typed().dynamic_schema);
}

#[tokio::test]
async fn applies_schema_before_typed_graph_write() -> Result<()> {
    let store = LadybugGraphStore::new(LadybugConfig::typed())?;
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
async fn apply_schema_rejects_generated_table_name_collisions() -> Result<()> {
    let store = LadybugGraphStore::new(LadybugConfig::typed())?;
    let node_collision = GraphSchema::builder()
        .node("a-b", Vec::new())
        .node("a_b", Vec::new())
        .build();
    let error = store
        .apply_schema(&node_collision)
        .await
        .expect_err("colliding node table names must fail");
    assert!(error.to_string().contains("grust_node_a_b"));

    let relationship_collision = GraphSchema::builder()
        .edge(
            "a-b",
            vec![Label::from("c")],
            vec![Label::from("d")],
            Vec::new(),
        )
        .edge(
            "a",
            vec![Label::from("b-c")],
            vec![Label::from("d")],
            Vec::new(),
        )
        .build();
    let error = store
        .apply_schema(&relationship_collision)
        .await
        .expect_err("colliding composed relationship table names must fail");
    assert!(error.to_string().contains("grust_rel_a_b_c_d"));

    let metadata_collision = GraphSchema::builder().node("index", Vec::new()).build();
    let error = store
        .apply_schema(&metadata_collision)
        .await
        .expect_err("typed table must not shadow metadata");
    assert!(error.to_string().contains("grust_node_index"));

    let duplicate_field = GraphSchema::builder()
        .node(
            "Person",
            vec![
                Field::optional("name", FieldType::String),
                Field::optional("name", FieldType::String),
            ],
        )
        .build();
    assert!(store.apply_schema(&duplicate_field).await.is_err());
    Ok(())
}

#[tokio::test]
async fn writes_reject_ambiguous_edge_keys_before_mutation() -> Result<()> {
    let first = Edge::new("b\u{1f}c", "a", "d", Props::new());
    let second = Edge::new("c", "a\u{1f}b", "d", Props::new());
    assert_eq!(edge_key(&first), edge_key(&second));

    let store = LadybugGraphStore::in_memory()?;
    let graph = Graph::new(
        vec![
            Node::new("Node", "a", Props::new()),
            Node::new("Node", "a\u{1f}b", Props::new()),
            Node::new("Node", "d", Props::new()),
        ],
        vec![first, second],
    );
    let error = store
        .put_graph(&graph)
        .await
        .expect_err("ambiguous structural edge keys must fail");
    assert!(error.to_string().contains("U+001F"));
    assert!(store.get_node(&NodeId::new("a")).await?.is_none());

    let explicit = Edge::new("KNOWS", "a", "d", Props::new()).with_id("edge\u{1f}one");
    assert!(store.put_edge(&explicit).await.is_err());
    assert!(store.get_edges(EdgeQuery::default()).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn writes_reject_node_ids_that_alias_metadata_before_mutation() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    let reserved = Node::new("Person", "table\u{1f}Person", Props::new());
    let error = store
        .put_node(&reserved)
        .await
        .expect_err("metadata-shaped node ids must fail before writing");
    assert!(error.to_string().contains("U+001F"));

    let valid = Node::new("Person", "person-1", Props::new());
    let graph = Graph::new(vec![valid.clone(), reserved], Vec::new());
    let error = store
        .put_graph(&graph)
        .await
        .expect_err("the complete graph must reject metadata-shaped node ids");
    assert!(error.to_string().contains("U+001F"));
    assert!(store.get_node(&valid.id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn idless_edges_round_trip_and_can_be_updated() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    store
        .put_graph(&Graph::new(
            vec![
                Node::new("Person", "person:ada", Props::new()),
                Node::new("Talk", "talk:grust", Props::new()),
            ],
            vec![Edge::new(
                "Presented By",
                "person:ada",
                "talk:grust",
                props(&[("year", Value::from(2025_i64))]),
            )],
        ))
        .await?;

    let mut edges = store
        .get_edges(EdgeQuery {
            from: Some(NodeId::from("person:ada")),
            to: Some(NodeId::from("talk:grust")),
            label: Some(Label::from("Presented By")),
        })
        .await?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id, None);

    let mut edge = edges.pop().expect("edge should be readable");
    edge.props.insert("year".to_string(), Value::from(2026_i64));
    assert_eq!(store.put_edge(&edge).await?, PutOutcome::Upserted);

    let edges = store.get_edges(EdgeQuery::default()).await?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id, None);
    assert_eq!(edges[0].props.get("year"), Some(&Value::from(2026_i64)));

    let explicit =
        Edge::new("Attended", "person:ada", "talk:grust", Props::new()).with_id("edge:explicit");
    store.put_edge(&explicit).await?;
    let explicit = store
        .get_edges(EdgeQuery {
            from: None,
            to: None,
            label: Some(Label::from("Attended")),
        })
        .await?
        .pop()
        .expect("explicit edge should be readable");
    assert_eq!(explicit.id, Some(EdgeId::from("edge:explicit")));
    Ok(())
}

#[tokio::test]
async fn typed_mode_requires_schema_before_writes() -> Result<()> {
    let store = LadybugGraphStore::new(LadybugConfig::typed())?;
    let err = store
        .put_graph(&sample_graph())
        .await
        .expect_err("typed Ladybug mode should require schema first");
    assert!(err.to_string().contains("requires apply_schema"));
    Ok(())
}

#[tokio::test]
async fn applied_schema_validates_later_untyped_writes() -> Result<()> {
    let store = LadybugGraphStore::in_memory()?;
    let schema = GraphSchema::builder()
        .node("Person", vec![Field::required("name", FieldType::String)])
        .build();
    store.apply_schema(&schema).await?;

    let err = store
        .put_node(&Node::new("Person", "person:ada", Props::new()))
        .await
        .expect_err("applied schema should validate later writes");
    assert!(err.to_string().contains("missing required field 'name'"));
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

#[tokio::test]
async fn clear_preserves_applied_schema_tables_in_typed_mode() -> Result<()> {
    let store = LadybugGraphStore::new(LadybugConfig::typed())?;
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
    store.put_typed_graph(&schema, &sample_graph()).await?;
    store.clear().await?;

    let report = store.put_graph(&sample_graph()).await?;

    assert_eq!(report, LoadReport { nodes: 2, edges: 1 });
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

#[tokio::test]
async fn bulk_load_keeps_upsert_semantics_and_serves_traversals() {
    let store = LadybugGraphStore::in_memory().expect("open");
    // Two rows for the same id and the same (from, to) pair in one load: the
    // last one wins, as a sequence of MERGEs would leave it.
    let first = Graph::new(
        vec![
            Node::new("V", "a", props(&[("v", Value::from(1))])),
            Node::new("V", "b", Props::default()),
            Node::new("V", "c", Props::default()),
            Node::new("V", "a", props(&[("v", Value::from(2))])),
        ],
        vec![
            Edge::new("E", "a", "b", props(&[("w", Value::from(1))])),
            Edge::new("E", "a", "c", Props::default()),
            Edge::new("E", "a", "b", props(&[("w", Value::from(2))])),
        ],
    );
    let report = store.put_graph(&first).await.expect("bulk load");
    assert_eq!((report.nodes, report.edges), (4, 3));
    let a = store
        .get_node(&"a".into())
        .await
        .expect("read")
        .expect("a exists");
    assert_eq!(a.props.get("v"), Some(&Value::from(2)));
    let out = store
        .get_edges(EdgeQuery {
            from: Some("a".into()),
            to: None,
            label: Some("E".into()),
        })
        .await
        .expect("edges");
    assert_eq!(out.len(), 2, "one edge per pair");
    let ab = out
        .iter()
        .find(|edge| edge.to.as_str() == "b")
        .expect("a->b");
    assert_eq!(ab.props.get("w"), Some(&Value::from(2)));

    // A second load with an existing id and pair updates them and adds the rest.
    let second = Graph::new(
        vec![
            Node::new("V", "a", props(&[("v", Value::from(3))])),
            Node::new("V", "b", Props::default()),
            Node::new("V", "d", Props::default()),
        ],
        vec![
            Edge::new("E", "a", "b", props(&[("w", Value::from(3))])),
            Edge::new("E", "d", "a", Props::default()),
        ],
    );
    store.put_graph(&second).await.expect("second load");
    let a = store
        .get_node(&"a".into())
        .await
        .expect("read")
        .expect("a exists");
    assert_eq!(a.props.get("v"), Some(&Value::from(3)));
    let out = store
        .get_edges(EdgeQuery {
            from: Some("a".into()),
            to: None,
            label: Some("E".into()),
        })
        .await
        .expect("edges");
    assert_eq!(out.len(), 2);
    assert_eq!(
        out.iter()
            .find(|e| e.to.as_str() == "b")
            .unwrap()
            .props
            .get("w"),
        Some(&Value::from(3))
    );

    let mut reached: Vec<_> = store
        .traverse_ids(Traversal::from_node("a").out("E"))
        .await
        .expect("traverse_ids")
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect();
    reached.sort();
    assert_eq!(reached, ["b", "c"]);
    let incoming = store
        .traverse(Traversal::from_node("a").in_("E"))
        .await
        .expect("traverse");
    assert_eq!(
        incoming.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
        ["d"]
    );
    let both = store
        .traverse_ids(Traversal::from_node("a").both("E"))
        .await
        .expect("both");
    assert_eq!(both.len(), 3);
}
