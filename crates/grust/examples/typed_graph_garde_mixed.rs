use grust::prelude::*;
use grust::typed::garde;
use serde::Serialize;

#[derive(Debug, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Person {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    name: String,
}

impl TypedNode for Person {
    const LABEL: &'static str = "Person";

    fn node_id(&self) -> NodeId {
        format!("person:{}", self.id).into()
    }
}

fn main() -> Result<()> {
    let existing = Graph::new(
        vec![Node::new("Document", "doc:garde-proposal", Props::new())],
        Vec::new(),
    );

    let mut builder = TypedGraphBuilder::from_graph(existing);
    builder.add_node(&Person {
        id: "nia".to_string(),
        name: "Nia".to_string(),
    })?;
    builder.add_raw_edge(Edge::new(
        "AUTHORED",
        "person:nia",
        "doc:garde-proposal",
        Props::new(),
    ));

    println!("{}", builder.build().to_yaml()?);

    Ok(())
}
