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
