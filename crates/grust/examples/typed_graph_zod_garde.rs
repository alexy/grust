use grust::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zod_rs::prelude::{Schema, number, object, string};

#[derive(Debug, Deserialize, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Person {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    name: String,
    #[garde(length(min = 1), inner(length(min = 1)))]
    skills: Vec<String>,
}

impl TypedNode for Person {
    const LABEL: &'static str = "Person";

    fn node_id(&self) -> NodeId {
        format!("person:{}", self.id).into()
    }
}

#[derive(Debug, Deserialize, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Project {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    title: String,
}

impl TypedNode for Project {
    const LABEL: &'static str = "Project";

    fn node_id(&self) -> NodeId {
        format!("project:{}", self.id).into()
    }
}

#[derive(Debug, Deserialize, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct WorksOn {
    #[garde(length(min = 1))]
    person_id: String,
    #[garde(length(min = 1))]
    project_id: String,
    #[garde(range(min = 1, max = 100))]
    allocation_percent: u8,
}

impl TypedEdge for WorksOn {
    const LABEL: &'static str = "WORKS_ON";

    fn from_node_id(&self) -> NodeId {
        format!("person:{}", self.person_id).into()
    }

    fn to_node_id(&self) -> NodeId {
        format!("project:{}", self.project_id).into()
    }
}

fn main() -> Result<()> {
    let person_schema = object()
        .field("id", string().min(1))
        .field("name", string().min(1))
        .field("skills", string().min(1).array())
        .strict();
    let project_schema = object()
        .field("id", string().min(1))
        .field("title", string().min(1))
        .strict();
    let works_on_schema = object()
        .field("person_id", string().min(1))
        .field("project_id", string().min(1))
        .field("allocation_percent", number().int().min(1.0).max(100.0))
        .strict();

    let mut builder = TypedGraphBuilder::new();
    builder.add_node_from_json::<Person, _>(
        &person_schema,
        &json!({
            "id": "nia",
            "name": "Nia",
            "skills": ["rust", "graphs"]
        }),
    )?;
    builder.add_node_from_json::<Project, _>(
        &project_schema,
        &json!({
            "id": "grust",
            "title": "Grust"
        }),
    )?;
    builder.add_edge_from_json::<WorksOn, _>(
        &works_on_schema,
        &json!({
            "person_id": "nia",
            "project_id": "grust",
            "allocation_percent": 80
        }),
    )?;

    println!("{}", builder.build().to_yaml()?);

    Ok(())
}
