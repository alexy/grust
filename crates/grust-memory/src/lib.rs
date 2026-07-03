use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use grust_core::prelude::*;

#[derive(Clone, Debug, Default)]
pub struct MemoryGraphStore {
    inner: Arc<RwLock<MemoryGraph>>,
}

#[derive(Clone, Debug, Default)]
struct MemoryGraph {
    nodes: BTreeMap<NodeId, Node>,
    edges: BTreeMap<MemoryEdgeKey, Edge>,
    schema: Option<GraphSchema>,
    native_constraints: Vec<GraphConstraint>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MemoryEdgeKey {
    from: NodeId,
    label: Label,
    to: NodeId,
    id: Option<EdgeId>,
}

impl MemoryEdgeKey {
    fn new(from: NodeId, label: Label, to: NodeId, id: Option<EdgeId>) -> Self {
        Self {
            from,
            label,
            to,
            id,
        }
    }

    fn from_edge(edge: &Edge) -> Self {
        Self::new(
            edge.from.clone(),
            edge.label.clone(),
            edge.to.clone(),
            edge.id.clone(),
        )
    }
}

impl MemoryGraphStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn graph(&self) -> Graph {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Graph {
            nodes: inner.nodes.values().cloned().collect(),
            edges: inner.edges.values().cloned().collect(),
        }
    }

    fn node_matches(
        node: &Node,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
    ) -> bool {
        label.is_none_or(|label| &node.label == label)
            && props.iter().all(|(key, value)| {
                if key == "id" {
                    value.as_str().is_some_and(|id| node.id.as_str() == id)
                } else {
                    node.props.get(key) == Some(value)
                }
            })
            && predicates
                .iter()
                .all(|predicate| predicate.matches(node.props.get(&predicate.key)))
    }

    fn matching_node_ids(
        inner: &MemoryGraph,
        label: Option<&Label>,
        props: &Props,
        predicates: &[GraphPropertyPredicate],
    ) -> Vec<NodeId> {
        inner
            .nodes
            .values()
            .filter(|node| Self::node_matches(node, label, props, predicates))
            .map(|node| node.id.clone())
            .collect()
    }

    fn relationship_matches(
        inner: &MemoryGraph,
        edge: &Edge,
        relationship: &GraphRelationshipMatch,
    ) -> bool {
        if edge.label != relationship.label {
            return false;
        }
        if relationship
            .id
            .as_ref()
            .is_some_and(|id| edge.id.as_ref() != Some(id))
        {
            return false;
        }
        if !relationship
            .props
            .iter()
            .all(|(key, value)| edge.props.get(key) == Some(value))
        {
            return false;
        }
        if !relationship
            .predicates
            .iter()
            .all(|predicate| predicate.matches(edge.props.get(&predicate.key)))
        {
            return false;
        }
        let Some(from) = inner.nodes.get(&edge.from) else {
            return false;
        };
        let Some(to) = inner.nodes.get(&edge.to) else {
            return false;
        };
        Self::node_matches(
            from,
            relationship.from.label.as_ref(),
            &relationship.from.props,
            &relationship.from.predicates,
        ) && Self::node_matches(
            to,
            relationship.to.label.as_ref(),
            &relationship.to.props,
            &relationship.to.predicates,
        )
    }

    fn matching_edges(inner: &MemoryGraph, relationship: &GraphRelationshipMatch) -> Vec<Edge> {
        inner
            .edges
            .values()
            .filter(|edge| Self::relationship_matches(inner, edge, relationship))
            .cloned()
            .collect()
    }

    fn graph_snapshot(inner: &MemoryGraph) -> Graph {
        Graph {
            nodes: inner.nodes.values().cloned().collect(),
            edges: inner.edges.values().cloned().collect(),
        }
    }

    fn graph_snapshot_with_node(inner: &MemoryGraph, node: &Node) -> Graph {
        let mut graph = Self::graph_snapshot(inner);
        if let Some(existing) = graph
            .nodes
            .iter_mut()
            .find(|existing| existing.id == node.id)
        {
            *existing = node.clone();
        } else {
            graph.nodes.push(node.clone());
        }
        graph
    }

    fn graph_snapshot_with_edge(inner: &MemoryGraph, edge: &Edge) -> Graph {
        let mut graph = Self::graph_snapshot(inner);
        let key = MemoryEdgeKey::from_edge(edge);
        if let Some(existing) = graph
            .edges
            .iter_mut()
            .find(|existing| MemoryEdgeKey::from_edge(existing) == key)
        {
            *existing = edge.clone();
        } else {
            graph.edges.push(edge.clone());
        }
        graph
    }

    fn graph_snapshot_with_graph(inner: &MemoryGraph, input: &Graph) -> Graph {
        let mut nodes = inner.nodes.clone();
        let mut edges = inner.edges.clone();
        for node in &input.nodes {
            nodes.insert(node.id.clone(), node.clone());
        }
        for edge in &input.edges {
            edges.insert(MemoryEdgeKey::from_edge(edge), edge.clone());
        }
        Graph {
            nodes: nodes.into_values().collect(),
            edges: edges.into_values().collect(),
        }
    }

    fn validate_native_constraints(inner: &MemoryGraph, graph: &Graph) -> Result<()> {
        for constraint in &inner.native_constraints {
            match constraint {
                GraphConstraint::NodePropertyRequired { label, key } => {
                    for node in graph.nodes.iter().filter(|node| &node.label == label) {
                        if !node.props.contains_key(key) {
                            return Err(GrustError::Schema(format!(
                                "node '{}' with label '{}' is missing native required constrained property '{}'",
                                node.id.as_str(),
                                label.as_str(),
                                key
                            )));
                        }
                    }
                }
                GraphConstraint::EdgePropertyRequired { label, key } => {
                    for edge in graph.edges.iter().filter(|edge| &edge.label == label) {
                        if !edge.props.contains_key(key) {
                            return Err(GrustError::Schema(format!(
                                "edge '{}' from '{}' to '{}' is missing native required constrained property '{}'",
                                edge.label.as_str(),
                                edge.from.as_str(),
                                edge.to.as_str(),
                                key
                            )));
                        }
                    }
                }
                GraphConstraint::NodePropertyUnique { label, key } => {
                    let mut seen: Vec<(&NodeId, &Value)> = Vec::new();
                    for node in graph.nodes.iter().filter(|node| &node.label == label) {
                        let Some(value) = node.props.get(key) else {
                            continue;
                        };
                        if let Some((existing_id, _)) =
                            seen.iter().find(|(_, existing)| *existing == value)
                        {
                            return Err(GrustError::Schema(format!(
                                "node '{}' with label '{}' duplicates native unique constrained property '{}' from node '{}'",
                                node.id.as_str(),
                                label.as_str(),
                                key,
                                existing_id.as_str()
                            )));
                        }
                        seen.push((&node.id, value));
                    }
                }
                GraphConstraint::EdgePropertyUnique { label, key } => {
                    let mut seen: Vec<(String, &Value)> = Vec::new();
                    for edge in graph.edges.iter().filter(|edge| &edge.label == label) {
                        let Some(value) = edge.props.get(key) else {
                            continue;
                        };
                        if let Some((existing_key, _)) =
                            seen.iter().find(|(_, existing)| *existing == value)
                        {
                            return Err(GrustError::Schema(format!(
                                "edge '{}' duplicates native unique constrained property '{}' from edge '{}'",
                                edge_key(edge),
                                key,
                                existing_key
                            )));
                        }
                        seen.push((edge_key(edge), value));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_write_snapshot(inner: &MemoryGraph, graph: &Graph) -> Result<()> {
        if let Some(schema) = &inner.schema {
            schema.validate_graph(graph)?;
        }
        Self::validate_native_constraints(inner, graph)
    }
}

#[async_trait]
impl GraphStore for MemoryGraphStore {
    async fn apply_schema(&self, schema: &GraphSchema) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        schema.validate_graph(&Self::graph_snapshot(&inner))?;
        inner.schema = Some(schema.clone());
        Ok(())
    }

    fn constraint_capability(&self, constraint: &GraphConstraint) -> GraphConstraintCapability {
        match constraint {
            GraphConstraint::NodePropertyRequired { .. }
            | GraphConstraint::EdgePropertyRequired { .. }
            | GraphConstraint::NodePropertyUnique { .. }
            | GraphConstraint::EdgePropertyUnique { .. } => {
                GraphConstraintCapability::ValidateBeforeWrite
            }
        }
    }

    fn native_constraint_capability(
        &self,
        constraint: &GraphConstraint,
    ) -> GraphNativeConstraintCapability {
        match constraint {
            GraphConstraint::NodePropertyRequired { .. }
            | GraphConstraint::EdgePropertyRequired { .. }
            | GraphConstraint::NodePropertyUnique { .. }
            | GraphConstraint::EdgePropertyUnique { .. } => {
                GraphNativeConstraintCapability::NativeConstraint
            }
        }
    }

    async fn apply_native_constraint(
        &self,
        request: GraphNativeConstraintRequest,
    ) -> Result<GraphNativeConstraintReport> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        if inner.native_constraints.contains(&request.constraint) {
            if request.if_not_exists {
                return Ok(GraphNativeConstraintReport {
                    applied: 0,
                    skipped: 1,
                });
            }
            return Err(GrustError::Schema(format!(
                "native graph constraint already exists: {:?}",
                request.constraint
            )));
        }

        let mut next = inner.native_constraints.clone();
        next.push(request.constraint);
        let graph = Self::graph_snapshot(&inner);
        let staged = MemoryGraph {
            nodes: inner.nodes.clone(),
            edges: inner.edges.clone(),
            schema: inner.schema.clone(),
            native_constraints: next,
        };
        Self::validate_native_constraints(&staged, &graph)?;
        inner.native_constraints = staged.native_constraints;
        Ok(GraphNativeConstraintReport {
            applied: 1,
            skipped: 0,
        })
    }

    async fn put_node(&self, node: &Node) -> Result<PutOutcome> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        Self::validate_write_snapshot(&inner, &Self::graph_snapshot_with_node(&inner, node))?;
        let previous = inner.nodes.insert(node.id.clone(), node.clone());
        Ok(match previous {
            Some(_) => PutOutcome::Updated,
            None => PutOutcome::Inserted,
        })
    }

    async fn put_edge(&self, edge: &Edge) -> Result<PutOutcome> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        Self::validate_write_snapshot(&inner, &Self::graph_snapshot_with_edge(&inner, edge))?;
        let previous = inner
            .edges
            .insert(MemoryEdgeKey::from_edge(edge), edge.clone());
        Ok(match previous {
            Some(_) => PutOutcome::Updated,
            None => PutOutcome::Inserted,
        })
    }

    async fn put_graph(&self, graph: &Graph) -> Result<LoadReport> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        Self::validate_write_snapshot(&inner, &Self::graph_snapshot_with_graph(&inner, graph))?;
        let mut report = LoadReport::default();
        for node in &graph.nodes {
            inner.nodes.insert(node.id.clone(), node.clone());
            report.nodes += 1;
        }
        for edge in &graph.edges {
            inner
                .edges
                .insert(MemoryEdgeKey::from_edge(edge), edge.clone());
            report.edges += 1;
        }
        Ok(report)
    }

    async fn get_node(&self, id: &NodeId) -> Result<Option<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(inner.nodes.get(id).cloned())
    }

    async fn get_nodes(&self, ids: &[NodeId]) -> Result<Vec<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(ids
            .iter()
            .filter_map(|id| inner.nodes.get(id).cloned())
            .collect())
    }

    async fn get_edges(&self, query: EdgeQuery) -> Result<Vec<Edge>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        Ok(inner
            .edges
            .values()
            .filter(|edge| {
                query.from.as_ref().is_none_or(|from| from == &edge.from)
                    && query.to.as_ref().is_none_or(|to| to == &edge.to)
                    && query
                        .label
                        .as_ref()
                        .is_none_or(|label| label == &edge.label)
            })
            .cloned()
            .collect())
    }

    async fn traverse(&self, traversal: Traversal) -> Result<Vec<Node>> {
        let inner = self.inner.read().expect("memory graph lock poisoned");
        let mut current = match traversal.start {
            Start::Node(id) => inner
                .nodes
                .get(&id)
                .cloned()
                .into_iter()
                .collect::<Vec<_>>(),
            Start::NodesByLabel(label) => inner
                .nodes
                .values()
                .filter(|node| node.label == label)
                .cloned()
                .collect(),
            Start::NodesByProperty { label, key, value } => inner
                .nodes
                .values()
                .filter(|node| node.label == label && node.props.get(&key) == Some(&value))
                .cloned()
                .collect(),
        };

        for step in traversal.steps {
            let mut next = Vec::new();
            for node in &current {
                for edge in inner.edges.values() {
                    let label_matches = step.edge.as_ref().is_none_or(|label| label == &edge.label);
                    let out_matches = matches!(step.direction, Direction::Out | Direction::Both)
                        && edge.from == node.id;
                    let in_matches = matches!(step.direction, Direction::In | Direction::Both)
                        && edge.to == node.id;

                    if !label_matches || (!out_matches && !in_matches) {
                        continue;
                    }

                    let target_id = if out_matches { &edge.to } else { &edge.from };
                    if let Some(target) = inner.nodes.get(target_id)
                        && step
                            .node
                            .as_ref()
                            .is_none_or(|label| label == &target.label)
                    {
                        next.push(target.clone());
                    }
                }
            }
            current = next;
        }

        if let Some(limit) = traversal.limit {
            current.truncate(limit as usize);
        }
        Ok(current)
    }
}

#[async_trait]
impl GraphMutationStore for MemoryGraphStore {
    async fn delete_node(&self, id: &NodeId) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner.nodes.remove(id);
        inner
            .edges
            .retain(|key, _| key.from != *id && key.to != *id);
        Ok(())
    }

    async fn delete_edge(&self, from: &NodeId, label: &Label, to: &NodeId) -> Result<()> {
        let mut inner = self.inner.write().expect("memory graph lock poisoned");
        inner
            .edges
            .retain(|key, _| key.from != *from || key.label != *label || key.to != *to);
        Ok(())
    }
}

#[async_trait]
impl CypherMutationExecutor for MemoryGraphStore {
    async fn execute_cypher_mutation_plan(
        &self,
        plan: &GraphMutationPlan,
    ) -> Result<GraphMutationReport> {
        let mut report = plan.report();
        for operation in &plan.operations {
            match operation {
                GraphMutationPlanOp::PatchMatchingNodes {
                    label,
                    props,
                    predicates,
                    patch,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props, predicates);
                    report.matched_rows += ids.len();
                    report.node_patches += ids.len();
                    report.changed_nodes += ids.len();

                    let mut patched = Vec::with_capacity(ids.len());
                    for id in &ids {
                        if let Some(node) = inner.nodes.get(id) {
                            let mut node = node.clone();
                            for (key, value) in patch {
                                node.props.insert(key.clone(), value.clone());
                            }
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            patched.push(node);
                        }
                    }
                    for node in patched {
                        inner.nodes.insert(node.id.clone(), node);
                    }
                }
                GraphMutationPlanOp::UpdateMatchingNodeProperty {
                    label,
                    props,
                    predicates,
                    target_key,
                    source_key,
                    op,
                    operand,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props, predicates);
                    report.matched_rows += ids.len();
                    report.node_patches += ids.len();
                    report.changed_nodes += ids.len();

                    let mut updated = Vec::with_capacity(ids.len());
                    for id in &ids {
                        if let Some(node) = inner.nodes.get(id) {
                            let mut node = node.clone();
                            let current = node.props.get(source_key).ok_or_else(|| {
                                GrustError::CypherExecution(format!(
                                    "numeric expression source property '{source_key}' is missing"
                                ))
                            })?;
                            let value = evaluate_numeric_update(current, *op, operand)?;
                            node.props.insert(target_key.clone(), value);
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            updated.push(node);
                        }
                    }
                    for node in updated {
                        inner.nodes.insert(node.id.clone(), node);
                    }
                }
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label,
                    props,
                    predicates,
                    keys,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props, predicates);
                    report.matched_rows += ids.len();
                    report.node_property_removes += ids.len();
                    report.changed_nodes += ids.len();

                    let mut updated = Vec::with_capacity(ids.len());
                    for id in &ids {
                        if let Some(node) = inner.nodes.get(id) {
                            let mut node = node.clone();
                            for key in keys {
                                node.props.remove(key);
                            }
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            updated.push(node);
                        }
                    }
                    for node in updated {
                        inner.nodes.insert(node.id.clone(), node);
                    }
                }
                GraphMutationPlanOp::DeleteMatchingNodes {
                    label,
                    props,
                    predicates,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let ids = Self::matching_node_ids(&inner, label.as_ref(), props, predicates);
                    let incident_edges = inner
                        .edges
                        .keys()
                        .filter(|key| ids.iter().any(|id| id == &key.from || id == &key.to))
                        .count();

                    report.matched_rows += ids.len();
                    report.node_deletes += ids.len();
                    report.changed_nodes += ids.len();
                    report.edge_deletes += incident_edges;
                    report.changed_edges += incident_edges;

                    for id in &ids {
                        inner.nodes.remove(id);
                    }
                    inner
                        .edges
                        .retain(|key, _| !ids.iter().any(|id| id == &key.from || id == &key.to));
                }
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_patches += edges.len();
                    report.changed_edges += edges.len();

                    let mut patched = Vec::with_capacity(edges.len());
                    for mut edge in edges {
                        for (key, value) in patch {
                            edge.props.insert(key.clone(), value.clone());
                        }
                        if let Some(schema) = &inner.schema {
                            schema.validate_edge_with(&edge, |id| {
                                inner.nodes.get(id).map(|node| &node.label)
                            })?;
                        }
                        patched.push(edge);
                    }
                    for edge in patched {
                        inner.edges.insert(MemoryEdgeKey::from_edge(&edge), edge);
                    }
                }
                GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                    relationship,
                    target_key,
                    source_key,
                    op,
                    operand,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_patches += edges.len();
                    report.changed_edges += edges.len();

                    let mut updated = Vec::with_capacity(edges.len());
                    for mut edge in edges {
                        let current = edge.props.get(source_key).ok_or_else(|| {
                            GrustError::CypherExecution(format!(
                                "numeric expression source property '{source_key}' is missing"
                            ))
                        })?;
                        let value = evaluate_numeric_update(current, *op, operand)?;
                        edge.props.insert(target_key.clone(), value);
                        if let Some(schema) = &inner.schema {
                            schema.validate_edge_with(&edge, |id| {
                                inner.nodes.get(id).map(|node| &node.label)
                            })?;
                        }
                        updated.push(edge);
                    }
                    for edge in updated {
                        inner.edges.insert(MemoryEdgeKey::from_edge(&edge), edge);
                    }
                }
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship, keys, ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_property_removes += edges.len();
                    report.changed_edges += edges.len();

                    let mut updated = Vec::with_capacity(edges.len());
                    for mut edge in edges {
                        for key in keys {
                            edge.props.remove(key);
                        }
                        if let Some(schema) = &inner.schema {
                            schema.validate_edge_with(&edge, |id| {
                                inner.nodes.get(id).map(|node| &node.label)
                            })?;
                        }
                        updated.push(edge);
                    }
                    for edge in updated {
                        inner.edges.insert(MemoryEdgeKey::from_edge(&edge), edge);
                    }
                }
                GraphMutationPlanOp::DeleteMatchingEdges { relationship, .. } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    report.matched_rows += edges.len();
                    report.edge_deletes += edges.len();
                    report.changed_edges += edges.len();
                    for edge in edges {
                        inner.edges.remove(&MemoryEdgeKey::from_edge(&edge));
                    }
                }
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges,
                    endpoint_nodes,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let edges = Self::matching_edges(&inner, relationship);
                    let mut ids = edges
                        .iter()
                        .flat_map(|edge| {
                            endpoint_nodes.iter().map(|endpoint| match endpoint {
                                GraphRelationshipEndpoint::From => edge.from.clone(),
                                GraphRelationshipEndpoint::To => edge.to.clone(),
                            })
                        })
                        .collect::<Vec<_>>();
                    ids.sort();
                    ids.dedup();

                    let mut edge_keys = if *delete_edges {
                        edges
                            .iter()
                            .map(MemoryEdgeKey::from_edge)
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    edge_keys.extend(
                        inner
                            .edges
                            .keys()
                            .filter(|key| ids.iter().any(|id| id == &key.from || id == &key.to))
                            .cloned(),
                    );
                    edge_keys.sort();
                    edge_keys.dedup();

                    report.matched_rows += edges.len();
                    report.node_deletes += ids.len();
                    report.changed_nodes += ids.len();
                    report.edge_deletes += edge_keys.len();
                    report.changed_edges += edge_keys.len();

                    for id in &ids {
                        inner.nodes.remove(id);
                    }
                    for key in edge_keys {
                        inner.edges.remove(&key);
                    }
                }
                GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                    kind,
                    from,
                    to,
                    label,
                    props,
                    edge_id_policy,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let from_ids = Self::matching_node_ids(
                        &inner,
                        from.label.as_ref(),
                        &from.props,
                        &from.predicates,
                    );
                    let to_ids = Self::matching_node_ids(
                        &inner,
                        to.label.as_ref(),
                        &to.props,
                        &to.predicates,
                    );
                    let matched_rows = from_ids.len().saturating_mul(to_ids.len());
                    report.matched_rows += matched_rows;
                    report.edge_upserts += matched_rows;
                    report.changed_edges += matched_rows;
                    let explicit_edge_id = explicit_edge_id_from_props(props)?;
                    if explicit_edge_id.is_some() && matched_rows > 1 {
                        return Err(GrustError::CypherUnsupportedCardinality(
                            "row-producing MATCH ... CREATE/MERGE with an explicit relationship id must produce exactly one edge".to_string(),
                        ));
                    }

                    let mut edges = Vec::with_capacity(matched_rows);
                    for from_id in &from_ids {
                        for to_id in &to_ids {
                            let mut edge = Edge::new(
                                label.clone(),
                                from_id.clone(),
                                to_id.clone(),
                                props.clone(),
                            );
                            if let Some(id) = explicit_edge_id.clone() {
                                edge = edge.with_id(id);
                            } else if row_edge_id_policy_generates(*kind, *edge_id_policy) {
                                edge = edge
                                    .with_id(generated_row_edge_id(from_id, label, to_id, props));
                            }
                            if let Some(schema) = &inner.schema {
                                schema.validate_edge_with(&edge, |id| {
                                    inner.nodes.get(id).map(|node| &node.label)
                                })?;
                            }
                            edges.push(edge);
                        }
                    }
                    for edge in edges {
                        let previous = inner.edges.insert(MemoryEdgeKey::from_edge(&edge), edge);
                        if previous.is_some() {
                            report.edge_updates += 1;
                        } else {
                            report.edge_inserts += 1;
                        }
                    }
                }
                GraphMutationPlanOp::UpsertNode { node, .. } => {
                    classify_node_upsert(self.put_node(node).await?, &mut report);
                }
                GraphMutationPlanOp::UpsertEdge { edge, .. } => {
                    classify_edge_upsert(self.put_edge(edge).await?, &mut report);
                }
                GraphMutationPlanOp::SetMatchingNodeFromNode {
                    target_label,
                    target_props,
                    target_predicates,
                    target_key,
                    source_label,
                    source_props,
                    source_predicates,
                    source_key,
                    op,
                    operand,
                    correlation,
                    ..
                } => {
                    let mut inner = self.inner.write().expect("memory graph lock poisoned");
                    let mut target_ids = Self::matching_node_ids(
                        &inner,
                        target_label.as_ref(),
                        target_props,
                        target_predicates,
                    );
                    let mut source_ids = Self::matching_node_ids(
                        &inner,
                        source_label.as_ref(),
                        source_props,
                        source_predicates,
                    );
                    target_ids.sort();
                    source_ids.sort();
                    let target_set: std::collections::BTreeSet<NodeId> =
                        target_ids.iter().cloned().collect();
                    let source_set: std::collections::BTreeSet<NodeId> =
                        source_ids.iter().cloned().collect();
                    // Build (target, source) pairs deterministically per correlation.
                    let pairs: Vec<(NodeId, NodeId)> = match correlation {
                        GraphWriteCorrelation::Cartesian => {
                            let mut pairs = Vec::new();
                            for t in &target_ids {
                                for s in &source_ids {
                                    pairs.push((t.clone(), s.clone()));
                                }
                            }
                            pairs
                        }
                        GraphWriteCorrelation::OutgoingRelationship { label } => {
                            let mut pairs: Vec<(NodeId, NodeId)> = inner
                                .edges
                                .values()
                                .filter(|e| {
                                    &e.label == label
                                        && target_set.contains(&e.from)
                                        && source_set.contains(&e.to)
                                })
                                .map(|e| (e.from.clone(), e.to.clone()))
                                .collect();
                            pairs.sort();
                            pairs
                        }
                        GraphWriteCorrelation::IncomingRelationship { label } => {
                            let mut pairs: Vec<(NodeId, NodeId)> = inner
                                .edges
                                .values()
                                .filter(|e| {
                                    &e.label == label
                                        && source_set.contains(&e.from)
                                        && target_set.contains(&e.to)
                                })
                                .map(|e| (e.to.clone(), e.from.clone()))
                                .collect();
                            pairs.sort();
                            pairs
                        }
                    };
                    // Apply target[target_key] = source[source_key] [op operand];
                    // for cartesian fan-out the last source (by id order) wins.
                    let mut changed: std::collections::BTreeSet<NodeId> = Default::default();
                    for (t, s) in pairs {
                        let value = {
                            let Some(source_node) = inner.nodes.get(&s) else {
                                continue;
                            };
                            let current = source_node.props.get(source_key).ok_or_else(|| {
                                GrustError::CypherExecution(format!(
                                    "cross-variable update source property '{source_key}' is missing"
                                ))
                            })?;
                            match op {
                                Some(op) => evaluate_numeric_update(current, *op, operand)?,
                                None => current.clone(),
                            }
                        };
                        if let Some(node) = inner.nodes.get(&t) {
                            let mut node = node.clone();
                            node.props.insert(target_key.clone(), value);
                            if let Some(schema) = &inner.schema {
                                schema.validate_node(&node)?;
                            }
                            inner.nodes.insert(node.id.clone(), node);
                            changed.insert(t);
                        }
                    }
                    report.matched_rows += changed.len();
                    report.node_patches += changed.len();
                    report.changed_nodes += changed.len();
                }
                _ => {
                    let mutation = GraphMutation::from(operation.clone());
                    self.apply_mutations(std::slice::from_ref(&mutation))
                        .await?;
                }
            }
        }
        Ok(report)
    }
}

fn explicit_edge_id_from_props(props: &Props) -> Result<Option<String>> {
    match props.get("id") {
        Some(Value::String(id)) => Ok(Some(id.clone())),
        Some(_) => Err(GrustError::CypherSyntax(
            "relationship id property must be a string literal".to_string(),
        )),
        None => Ok(None),
    }
}

fn row_edge_id_policy_generates(kind: GraphMutationPlanKind, policy: GraphRowEdgeIdPolicy) -> bool {
    matches!(
        (kind, policy),
        (
            GraphMutationPlanKind::Create,
            GraphRowEdgeIdPolicy::GenerateForCreate
                | GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        ) | (
            GraphMutationPlanKind::Merge,
            GraphRowEdgeIdPolicy::GenerateForCreateAndMerge
        )
    )
}

#[cfg(test)]
mod tests;
