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
    #[garde(length(min = 1), inner(length(min = 1)))]
    skills: Vec<String>,
}

impl TypedNode for Person {
    const LABEL: &'static str = "Person";

    fn node_id(&self) -> NodeId {
        format!("person:{}", self.id).into()
    }
}

#[derive(Debug, Serialize, garde::Validate)]
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

#[derive(Debug, Serialize, garde::Validate)]
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

    fn source_node_id(&self) -> NodeId {
        format!("person:{}", self.person_id).into()
    }

    fn target_node_id(&self) -> NodeId {
        format!("project:{}", self.project_id).into()
    }
}

#[derive(Debug, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Team {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    name: String,
}

impl TypedNode for Team {
    const LABEL: &'static str = "Team";

    fn node_id(&self) -> NodeId {
        format!("team:{}", self.id).into()
    }
}

#[derive(Debug, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct MemberOf {
    #[garde(length(min = 1))]
    person_id: String,
    #[garde(length(min = 1))]
    team_id: String,
}

impl TypedEdge for MemberOf {
    const LABEL: &'static str = "MEMBER_OF";

    fn source_node_id(&self) -> NodeId {
        format!("person:{}", self.person_id).into()
    }

    fn target_node_id(&self) -> NodeId {
        format!("team:{}", self.team_id).into()
    }
}

fn main() -> Result<()> {
    let mut builder = TypedGraphBuilder::new();

    builder.add_node(&Person {
        id: "nia".to_string(),
        name: "Nia".to_string(),
        skills: vec!["rust".to_string(), "graphs".to_string()],
    })?;
    builder.add_node(&Project {
        id: "grust".to_string(),
        title: "Grust".to_string(),
    })?;
    builder.add_edge(&WorksOn {
        person_id: "nia".to_string(),
        project_id: "grust".to_string(),
        allocation_percent: 80,
    })?;

    builder.add_node(&Team {
        id: "platform".to_string(),
        name: "Platform".to_string(),
    })?;
    builder.add_edge(&MemberOf {
        person_id: "nia".to_string(),
        team_id: "platform".to_string(),
    })?;

    let graph = builder.build();
    println!("{}", graph.to_yaml()?);

    Ok(())
}
