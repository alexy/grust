//! Mutation plan entrypoints and the CypherMutationPlanner (extracted from lib.rs).

use crate::*;

pub fn cypher_mutation_plan_with_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<(GraphMutationPlan, Vec<CypherGeneratedNodeId>)> {
    let cypher = strip_cypher_comments(cypher)?;
    let statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("writable Cypher statement is empty"));
    }

    let mut planner = CypherMutationPlanner {
        node_id_policy: options.node_id_policy,
        relationship_id_policy: options.relationship_id_policy,
        null_assignment: options.null_assignment,
        parameters: options.parameters,
        ..CypherMutationPlanner::default()
    };
    let mut plan = GraphMutationPlan::default();
    for statement in statements {
        for operation in planner.plan_statement(statement)?.operations {
            plan.push(operation);
        }
    }
    Ok((plan, planner.generated_node_ids))
}

pub fn sail_cypher_mutation_plan_with_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<(GraphMutationPlan, Vec<CypherGeneratedNodeId>)> {
    cypher_mutation_plan_with_options(cypher, options)
}

pub struct CypherPlannedMutationWithReturn {
    pub plan: GraphMutationPlan,
    pub generated_node_ids: Vec<CypherGeneratedNodeId>,
    pub node_bindings: HashMap<String, NodeId>,
    pub edge_bindings: HashMap<String, CypherBoundEdgeIdentity>,
    pub row_node_bindings: HashMap<String, GraphNodeMatch>,
    pub row_edge_match_bindings: HashMap<String, GraphRelationshipMatch>,
    pub row_edge_bindings: HashMap<String, CypherRowProducedEdgeBinding>,
    pub row_path_bindings: HashMap<String, CypherRowProducedPathBinding>,
    pub return_clause: CypherReturnClause,
}

pub fn cypher_mutation_plan_with_return_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherPlannedMutationWithReturn> {
    let cypher = strip_cypher_comments(cypher)?;
    let mut statements = split_cypher_statements(&cypher)?;
    if statements.is_empty() {
        return Err(cypher_syntax("writable Cypher statement is empty"));
    }

    let final_statement = statements
        .pop()
        .expect("checked non-empty statement collection");
    for statement in &statements {
        if find_unquoted_keyword(statement, "RETURN").is_some() {
            return Err(cypher_syntax(
                "writable Cypher only supports RETURN on the final statement",
            ));
        }
    }
    let (final_mutation, return_clause) = split_final_return(final_statement)?;
    if final_mutation.trim().is_empty() {
        return Err(cypher_syntax(
            "writable Cypher RETURN requires a preceding mutation statement",
        ));
    }

    let mut planner = CypherMutationPlanner {
        node_id_policy: options.node_id_policy,
        relationship_id_policy: options.relationship_id_policy,
        null_assignment: options.null_assignment,
        parameters: options.parameters,
        bind_delete_return_rows: true,
        ..CypherMutationPlanner::default()
    };
    let mut plan = GraphMutationPlan::default();
    for statement in statements {
        for operation in planner.plan_statement(statement)?.operations {
            plan.push(operation);
        }
    }
    for operation in planner.plan_statement(final_mutation)?.operations {
        plan.push(operation);
    }
    let return_clause = parse_cypher_return_clause(
        return_clause,
        &planner.node_bindings,
        &planner.edge_bindings,
        &planner.row_node_bindings,
        &planner.row_edge_match_bindings,
        &planner.row_edge_bindings,
        &planner.row_path_bindings,
        &planner.parameters,
    )?;
    Ok(CypherPlannedMutationWithReturn {
        plan,
        generated_node_ids: planner.generated_node_ids,
        node_bindings: planner.node_bindings,
        edge_bindings: planner.edge_bindings,
        row_node_bindings: planner.row_node_bindings,
        row_edge_match_bindings: planner.row_edge_match_bindings,
        row_edge_bindings: planner.row_edge_bindings,
        row_path_bindings: planner.row_path_bindings,
        return_clause,
    })
}

pub fn sail_cypher_mutation_plan_with_return_options(
    cypher: &str,
    options: CypherMutationOptions,
) -> Result<CypherPlannedMutationWithReturn> {
    cypher_mutation_plan_with_return_options(cypher, options)
}

#[derive(Default)]
pub(crate) struct CypherMutationPlanner {
    pub(crate) node_bindings: HashMap<String, NodeId>,
    pub(crate) edge_bindings: HashMap<String, CypherBoundEdgeIdentity>,
    pub(crate) row_node_bindings: HashMap<String, GraphNodeMatch>,
    pub(crate) row_edge_match_bindings: HashMap<String, GraphRelationshipMatch>,
    pub(crate) row_edge_bindings: HashMap<String, CypherRowProducedEdgeBinding>,
    pub(crate) row_path_bindings: HashMap<String, CypherRowProducedPathBinding>,
    pub(crate) node_id_policy: CypherNodeIdPolicy,
    pub(crate) relationship_id_policy: CypherRelationshipIdPolicy,
    pub(crate) null_assignment: CypherNullAssignment,
    pub(crate) parameters: CypherParameters,
    pub(crate) generated_node_ids: Vec<CypherGeneratedNodeId>,
    pub(crate) bind_delete_return_rows: bool,
}

impl CypherMutationPlanner {
    fn plan_statement(&mut self, cypher: &str) -> Result<GraphMutationPlan> {
        let cypher = cypher.trim();
        match cypher_parser::classify_statement(cypher)? {
            cypher_parser::CypherStatement::Match(rest) => self.parse_match(rest),
            cypher_parser::CypherStatement::Create(rest) => {
                self.parse_upsert(rest, GraphMutationPlanKind::Create)
            }
            cypher_parser::CypherStatement::Merge(rest) => {
                self.parse_upsert(rest, GraphMutationPlanKind::Merge)
            }
            cypher_parser::CypherStatement::Delete(rest) => self.parse_delete(rest),
        }
    }

    fn parse_upsert(
        &mut self,
        pattern: &str,
        kind: GraphMutationPlanKind,
    ) -> Result<GraphMutationPlan> {
        if find_unquoted_sequence(pattern, "->").is_some() {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpsertEdge {
                    kind,
                    edge: parsed.edge,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher node pattern suffix: {}",
                rest.trim()
            )));
        }
        let label = node
            .label
            .clone()
            .ok_or_else(|| cypher_syntax("node CREATE/MERGE requires a label"))?;
        let id = match optional_string_prop(&node.props, "id") {
            Some(id) => id,
            None if kind == GraphMutationPlanKind::Create
                && self.node_id_policy == CypherNodeIdPolicy::GenerateForCreate =>
            {
                let id = format!("node-{}", uuid::Uuid::new_v4());
                self.generated_node_ids.push(CypherGeneratedNodeId {
                    variable: node.variable.clone(),
                    id: NodeId::new(id.clone()),
                });
                id
            }
            None => {
                return Err(cypher_unresolved_identity(
                    "node CREATE/MERGE requires explicit string property 'id'",
                ));
            }
        };
        self.bind_node_variable(&node, &NodeId::new(id.clone()))?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertNode {
                kind,
                node: Node::new(label, id, node.props),
            },
        ]))
    }

    fn parse_delete(&mut self, pattern: &str) -> Result<GraphMutationPlan> {
        if find_unquoted_sequence(pattern, "->").is_some() {
            let parsed = self.parse_edge_pattern(pattern)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteEdge {
                    from: parsed.from_id,
                    label: parsed.edge.label,
                    to: parsed.to_id,
                },
            ]));
        }

        let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher delete pattern suffix: {}",
                rest.trim()
            )));
        }
        let id = self.resolve_node_id(&node, "node DELETE")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteNode(id),
        ]))
    }

    fn parse_match(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        if find_unquoted_keyword(statement, "DELETE").is_some() {
            return self.parse_match_delete(statement);
        }
        if find_unquoted_keyword(statement, "MERGE").is_some() {
            return self.parse_match_merge(statement);
        }
        if find_unquoted_keyword(statement, "CREATE").is_some() {
            return self.parse_match_create(statement);
        }
        if find_unquoted_keyword(statement, "SET").is_some() {
            return self.parse_match_set(statement);
        }
        if find_unquoted_keyword(statement, "REMOVE").is_some() {
            return self.parse_match_remove(statement);
        }
        Err(cypher_syntax(
            "only ID-resolved MATCH ... DELETE, MATCH ... CREATE/MERGE edge, MATCH ... SET, and MATCH ... REMOVE forms are supported in writable Cypher".to_string(),
        ))
    }

    fn parse_match_delete(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, target) = split_match_delete(statement)?;
        let targets = parse_match_delete_targets(target)?;
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH DELETE")?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge DELETE")?;
            let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires the relationship pattern to bind the DELETE target"
                        .to_string(),
                ));
            };
            let edge_variable = edge_variable.clone();
            let path_forces_endpoint_rows =
                path_variable.is_some() && targets.iter().any(|target| target == &edge_variable);
            let mut delete_relationship_rows = false;
            let mut row_endpoint_targets = Vec::new();
            let mut resolved_endpoint_ops = Vec::new();
            for target in &targets {
                if target == &edge_variable {
                    delete_relationship_rows = true;
                } else if parsed.from.variable.as_deref() == Some(target.as_str()) {
                    if path_forces_endpoint_rows
                        || self.endpoint_delete_requires_relationship_rows(&parsed.from)?
                    {
                        row_endpoint_targets.push(GraphRelationshipEndpoint::From);
                    } else {
                        resolved_endpoint_ops.extend(
                            self.lower_match_edge_endpoint_delete(
                                &parsed,
                                GraphRelationshipEndpoint::From,
                                target,
                            )?
                            .operations,
                        );
                    }
                } else if parsed.to.variable.as_deref() == Some(target.as_str()) {
                    if path_forces_endpoint_rows
                        || self.endpoint_delete_requires_relationship_rows(&parsed.to)?
                    {
                        row_endpoint_targets.push(GraphRelationshipEndpoint::To);
                    } else {
                        resolved_endpoint_ops.extend(
                            self.lower_match_edge_endpoint_delete(
                                &parsed,
                                GraphRelationshipEndpoint::To,
                                target,
                            )?
                            .operations,
                        );
                    }
                } else {
                    return Err(cypher_syntax(format!(
                        "MATCH edge DELETE target '{target}' is not bound by the relationship pattern"
                    )));
                }
            }
            if path_variable.is_some()
                && (!delete_relationship_rows || !resolved_endpoint_ops.is_empty())
            {
                return Err(cypher_syntax(
                    "MATCH edge DELETE path variables require deleting the matched relationship variable and do not support separately resolved endpoint deletes",
                ));
            }

            if !row_endpoint_targets.is_empty() {
                row_endpoint_targets.sort_by_key(|endpoint| match endpoint {
                    GraphRelationshipEndpoint::From => 0,
                    GraphRelationshipEndpoint::To => 1,
                });
                row_endpoint_targets.dedup();
                let relationship =
                    self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
                if path_variable.is_some() {
                    for endpoint in &row_endpoint_targets {
                        let node = match endpoint {
                            GraphRelationshipEndpoint::From => &parsed.from,
                            GraphRelationshipEndpoint::To => &parsed.to,
                        };
                        self.promote_node_variable_to_row_binding(node)?;
                    }
                }
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge DELETE",
                )?;
                if self.bind_delete_return_rows || path_variable.is_some() {
                    self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                }
                let mut operations = vec![GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: delete_relationship_rows,
                    endpoint_nodes: row_endpoint_targets.clone(),
                    target_count: usize::from(delete_relationship_rows)
                        + row_endpoint_targets.len(),
                    cardinality: GraphMutationCardinality::BoundedMany,
                }];
                operations.extend(resolved_endpoint_ops);
                return Ok(GraphMutationPlan::new(operations));
            }

            let mut plan = GraphMutationPlan::default();
            for target in targets {
                if target == edge_variable {
                    plan.operations.extend(
                        self.lower_match_edge_delete(
                            parsed.clone(),
                            &edge_variable,
                            path_variable.as_deref(),
                        )?
                        .operations,
                    );
                } else if parsed.from.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(
                            &parsed,
                            GraphRelationshipEndpoint::From,
                            &target,
                        )?
                        .operations,
                    );
                } else if parsed.to.variable.as_deref() == Some(target.as_str()) {
                    plan.operations.extend(
                        self.lower_match_edge_endpoint_delete(
                            &parsed,
                            GraphRelationshipEndpoint::To,
                            &target,
                        )?
                        .operations,
                    );
                } else {
                    return Err(cypher_syntax(format!(
                        "MATCH edge DELETE target '{target}' is not bound by the relationship pattern"
                    )));
                }
            }
            if plan.operations.is_empty() {
                return Err(cypher_syntax(
                    "MATCH edge DELETE requires at least one target",
                ));
            }
            return Ok(plan);
        }

        if path_variable.is_some() {
            return Err(cypher_syntax(
                "MATCH node DELETE does not support path variables",
            ));
        }
        if targets.len() != 1 {
            return Err(cypher_syntax(
                "MATCH node DELETE supports one target for a single-node pattern",
            ));
        }
        let target = targets
            .into_iter()
            .next()
            .expect("checked one delete target");
        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node DELETE")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH DELETE pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH node DELETE requires the node pattern to bind the DELETE target".to_string(),
            ));
        };
        if node_variable != &target {
            return Err(cypher_syntax(format!(
                "MATCH node DELETE target '{target}' does not match node variable '{node_variable}'"
            )));
        }
        if let Some(id) = optional_string_prop(&node.props, "id")
            && node.predicates.is_empty()
        {
            self.bind_node_variable(&node, &NodeId::new(id.clone()))?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteNode(NodeId::new(id)),
            ]));
        }
        if node.variable.is_some()
            && node
                .variable
                .as_ref()
                .and_then(|variable| self.node_bindings.get(variable))
                .is_some()
            && node.predicates.is_empty()
        {
            let id = self.resolve_node_id(&node, "MATCH node DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteNode(id),
            ]));
        }
        let cardinality = match_node_cardinality(&node);
        if self.bind_delete_return_rows {
            self.bind_row_node_variable(&node)?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingNodes {
                label: node.label,
                props: node.props,
                predicates: node.predicates,
                cardinality,
            },
        ]))
    }

    fn lower_match_edge_delete(
        &mut self,
        parsed: ParsedCypherEdgeMatch,
        edge_variable: &str,
        path_variable: Option<&str>,
    ) -> Result<GraphMutationPlan> {
        let from_id = self.resolved_endpoint_id(&parsed.from)?;
        let to_id = self.resolved_endpoint_id(&parsed.to)?;
        let edge_id = optional_string_prop(&parsed.relationship.props, "id");
        if path_variable.is_none()
            && let (Some(from), Some(to), None) = (from_id, to_id, edge_id)
            && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
            && parsed.from.predicates.is_empty()
            && parsed.to.predicates.is_empty()
            && parsed.relationship.predicates.is_empty()
        {
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteEdge {
                    from,
                    label: parsed.relationship.label,
                    to,
                },
            ]));
        }
        self.bind_path_variable_for_edge_match(path_variable, &parsed, "MATCH edge DELETE")?;
        let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge DELETE")?;
        if self.bind_delete_return_rows || path_variable.is_some() {
            self.bind_row_edge_match_variable(edge_variable, &relationship)?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteMatchingEdges {
                relationship,
                cardinality: GraphMutationCardinality::BoundedMany,
            },
        ]))
    }

    fn lower_match_edge_endpoint_delete(
        &mut self,
        parsed: &ParsedCypherEdgeMatch,
        endpoint: GraphRelationshipEndpoint,
        _target: &str,
    ) -> Result<GraphMutationPlan> {
        let node = match endpoint {
            GraphRelationshipEndpoint::From => &parsed.from,
            GraphRelationshipEndpoint::To => &parsed.to,
        };
        if !node.predicates.is_empty() {
            let relationship =
                self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: false,
                    endpoint_nodes: vec![endpoint],
                    target_count: 1,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }
        let Some(id) = self.resolved_endpoint_id(node)? else {
            let relationship =
                self.relationship_match_from_pattern(parsed.clone(), "MATCH edge DELETE")?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::DeleteRelationshipRows {
                    relationship,
                    delete_edges: false,
                    endpoint_nodes: vec![endpoint],
                    target_count: 1,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        };
        self.bind_node_variable(node, &id)?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::DeleteNode(id),
        ]))
    }

    fn endpoint_delete_requires_relationship_rows(
        &mut self,
        node: &ParsedCypherNode,
    ) -> Result<bool> {
        if !node.predicates.is_empty() {
            return Ok(true);
        }
        Ok(self.resolved_endpoint_id(node)?.is_none())
    }

    fn parse_match_merge(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        self.parse_match_edge_upsert(statement, "MERGE", GraphMutationPlanKind::Merge)
    }

    fn parse_match_create(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        self.parse_match_edge_upsert(statement, "CREATE", GraphMutationPlanKind::Create)
    }

    fn parse_match_edge_upsert(
        &mut self,
        statement: &str,
        keyword: &str,
        kind: GraphMutationPlanKind,
    ) -> Result<GraphMutationPlan> {
        let (match_clause, edge_pattern) = split_match_edge_upsert(statement, keyword)?;
        let (match_clause, where_predicates) = split_match_where(match_clause, &self.parameters)?;
        let mut matched_nodes = BTreeMap::new();
        for pattern in split_top_level_patterns(match_clause)? {
            let (node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
            if !rest.trim().is_empty() {
                return Err(cypher_syntax(format!(
                    "unsupported writable Cypher MATCH pattern suffix: {}",
                    rest.trim()
                )));
            }
            if node.variable.is_none() {
                return Err(cypher_syntax(format!(
                    "MATCH {keyword} requires each matched node pattern to bind a variable"
                )));
            }
            let variable = node.variable.clone().expect("checked above");
            if matched_nodes.insert(variable.clone(), node).is_some() {
                return Err(cypher_unresolved_identity(format!(
                    "MATCH {keyword} cannot bind variable '{variable}' more than once"
                )));
            }
        }
        apply_match_where_predicates(
            &mut matched_nodes,
            where_predicates,
            &format!("MATCH {keyword}"),
        )?;

        let (path_variable, edge_pattern) = parse_row_path_binding(edge_pattern)?;
        if find_unquoted_sequence(edge_pattern, "->").is_none() {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} currently supports one relationship pattern only",
            )));
        }
        let parsed = self.parse_edge_match_pattern(edge_pattern)?;
        let Some(from_variable) = parsed.from.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must be bound variables"
            )));
        };
        let Some(to_variable) = parsed.to.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must be bound variables"
            )));
        };
        if parsed.from.label.is_some()
            || !parsed.from.props.is_empty()
            || !parsed.from.predicates.is_empty()
            || parsed.to.label.is_some()
            || !parsed.to.props.is_empty()
            || !parsed.to.predicates.is_empty()
        {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship endpoints must reference bound variables only"
            )));
        }
        let Some(from_node) = matched_nodes.get(from_variable).cloned() else {
            return Err(cypher_unresolved_identity(format!(
                "MATCH {keyword} relationship source variable '{from_variable}' is not bound"
            )));
        };
        let Some(to_node) = matched_nodes.get(to_variable).cloned() else {
            return Err(cypher_unresolved_identity(format!(
                "MATCH {keyword} relationship destination variable '{to_variable}' is not bound"
            )));
        };
        if !parsed.relationship.predicates.is_empty() {
            return Err(cypher_syntax(format!(
                "MATCH {keyword} relationship creation does not accept relationship WHERE predicates"
            )));
        }

        let from_id = self.resolved_endpoint_id(&from_node)?;
        let to_id = self.resolved_endpoint_id(&to_node)?;
        if let (Some(from), Some(to)) = (from_id, to_id)
            && from_node.predicates.is_empty()
            && to_node.predicates.is_empty()
        {
            let mut edge = Edge::new(
                parsed.relationship.label.clone(),
                from,
                to,
                parsed.relationship.props.clone(),
            );
            if let Some(id) = edge
                .props
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                edge = edge.with_id(id);
            }
            self.bind_edge_variable(
                &parsed.relationship,
                CypherBoundEdgeIdentity {
                    from: edge.from.clone(),
                    label: edge.label.clone(),
                    to: edge.to.clone(),
                    id: edge.id.clone(),
                },
            )?;
            self.bind_node_variable(&from_node, &edge.from)?;
            self.bind_node_variable(&to_node, &edge.to)?;
            if let Some(path_variable) = path_variable {
                let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                    return Err(cypher_syntax(format!(
                        "MATCH {keyword} path variables require the relationship pattern to bind a variable"
                    )));
                };
                self.bind_row_path_variable(
                    &path_variable,
                    CypherRowProducedPathBinding {
                        from_variable: from_variable.clone(),
                        edge_variable: edge_variable.clone(),
                        to_variable: to_variable.clone(),
                    },
                )?;
            }
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpsertEdge { kind, edge },
            ]));
        }
        validate_optional_edge_id_property(&parsed.relationship.props)?;
        let from = self.node_match_from_pattern(from_node, "MATCH CREATE source")?;
        let to = self.node_match_from_pattern(to_node, "MATCH CREATE destination")?;
        let edge_id_policy = self.row_edge_id_policy(kind);
        if let Some(variable) = &parsed.relationship.variable {
            self.bind_row_edge_variable(
                variable,
                CypherRowProducedEdgeBinding {
                    kind,
                    from_variable: from_variable.clone(),
                    from: from.clone(),
                    to_variable: to_variable.clone(),
                    to: to.clone(),
                    label: parsed.relationship.label.clone(),
                    props: parsed.relationship.props.clone(),
                    edge_id_policy,
                },
            )?;
        }
        if let Some(path_variable) = path_variable {
            let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
                return Err(cypher_syntax(format!(
                    "MATCH {keyword} path variables require the relationship pattern to bind a variable"
                )));
            };
            self.bind_row_path_variable(
                &path_variable,
                CypherRowProducedPathBinding {
                    from_variable: from_variable.clone(),
                    edge_variable: edge_variable.clone(),
                    to_variable: to_variable.clone(),
                },
            )?;
        }
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpsertEdgesFromNodeMatches {
                kind,
                from,
                to,
                label: parsed.relationship.label,
                props: parsed.relationship.props,
                edge_id_policy,
                cardinality: GraphMutationCardinality::BoundedMany,
            },
        ]))
    }

    fn row_edge_id_policy(&self, kind: GraphMutationPlanKind) -> GraphRowEdgeIdPolicy {
        match (kind, self.relationship_id_policy) {
            (GraphMutationPlanKind::Create, CypherRelationshipIdPolicy::GenerateForRowCreate) => {
                GraphRowEdgeIdPolicy::GenerateForCreate
            }
            (
                GraphMutationPlanKind::Create | GraphMutationPlanKind::Merge,
                CypherRelationshipIdPolicy::GenerateForRowCreateAndMerge,
            ) => GraphRowEdgeIdPolicy::GenerateForCreateAndMerge,
            _ => GraphRowEdgeIdPolicy::ExplicitOnly,
        }
    }

    fn parse_match_set(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, assignments) = split_match_set(statement)?;
        let assignments =
            parse_patch_assignments(assignments, &self.parameters, self.null_assignment)?;
        let mut plan = GraphMutationPlan::default();
        for assignment in assignments {
            plan.operations.extend(
                self.lower_match_set_assignment(pattern, assignment)?
                    .operations,
            );
        }
        Ok(plan)
    }

    fn lower_match_set_assignment(
        &mut self,
        pattern: &str,
        assignment: PatchAssignment,
    ) -> Result<GraphMutationPlan> {
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH SET")?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge SET")?;
            let Some(edge_variable) = parsed.relationship.variable.clone() else {
                return Err(cypher_syntax(
                    "MATCH edge SET requires the relationship pattern to bind the patch target",
                ));
            };
            if edge_variable != assignment.target {
                return Err(cypher_syntax(format!(
                    "MATCH edge SET target '{}' does not match relationship variable '{edge_variable}'",
                    assignment.target
                )));
            }
            let kind = assignment.kind;
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let PatchAssignmentKind::NumericExpression {
                key,
                source_target,
                source_key,
                op,
                operand,
            } = kind
            {
                if source_target != assignment.target {
                    return Err(cypher_unsupported_cardinality(
                        "MATCH edge SET numeric expressions cannot reference another variable",
                    ));
                }
                let (relationship, cardinality) = if let (Some(from), Some(to)) =
                    (from_id.clone(), to_id.clone())
                    && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                    && parsed.from.predicates.is_empty()
                    && parsed.to.predicates.is_empty()
                    && parsed.relationship.predicates.is_empty()
                {
                    let id =
                        optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                    self.bind_edge_variable(
                        &parsed.relationship,
                        CypherBoundEdgeIdentity {
                            from: from.clone(),
                            label: parsed.relationship.label.clone(),
                            to: to.clone(),
                            id: id.clone(),
                        },
                    )?;
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
                    )?;
                    (
                        GraphRelationshipMatch {
                            from: GraphNodeMatch {
                                label: None,
                                props: Props::from([(
                                    "id".to_string(),
                                    Value::from(from.as_str()),
                                )]),
                                predicates: Vec::new(),
                            },
                            label: parsed.relationship.label,
                            to: GraphNodeMatch {
                                label: None,
                                props: Props::from([("id".to_string(), Value::from(to.as_str()))]),
                                predicates: Vec::new(),
                            },
                            id,
                            props: Props::new(),
                            predicates: Vec::new(),
                        },
                        GraphMutationCardinality::SingleIdentity,
                    )
                } else {
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
                    )?;
                    let relationship =
                        self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
                    self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                    (relationship, GraphMutationCardinality::BoundedMany)
                };
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::UpdateMatchingEdgeProperty {
                        relationship,
                        target_key: key,
                        source_key,
                        op,
                        operand,
                        cardinality,
                    },
                ]));
            }
            if let PatchAssignmentKind::RemoveProperty { key } = kind {
                if let (Some(from), Some(to)) = (from_id, to_id)
                    && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                    && parsed.from.predicates.is_empty()
                    && parsed.to.predicates.is_empty()
                    && parsed.relationship.predicates.is_empty()
                {
                    let id =
                        optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                    self.bind_edge_variable(
                        &parsed.relationship,
                        CypherBoundEdgeIdentity {
                            from: from.clone(),
                            label: parsed.relationship.label.clone(),
                            to: to.clone(),
                            id: id.clone(),
                        },
                    )?;
                    self.bind_path_variable_for_edge_match(
                        path_variable.as_deref(),
                        &parsed,
                        "MATCH edge SET",
                    )?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::RemoveEdgeProps {
                            from,
                            label: parsed.relationship.label,
                            to,
                            id,
                            keys: vec![key],
                        },
                    ]));
                }
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge SET",
                )?;
                let relationship =
                    self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
                self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveMatchingEdgeProps {
                        relationship,
                        keys: vec![key],
                        cardinality: GraphMutationCardinality::BoundedMany,
                    },
                ]));
            };
            let PatchAssignmentKind::Props(props) = kind else {
                unreachable!("numeric expression handled above");
            };
            if let (Some(from), Some(to)) = (from_id, to_id)
                && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                && parsed.from.predicates.is_empty()
                && parsed.to.predicates.is_empty()
                && parsed.relationship.predicates.is_empty()
            {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                self.bind_edge_variable(
                    &parsed.relationship,
                    CypherBoundEdgeIdentity {
                        from: from.clone(),
                        label: parsed.relationship.label.clone(),
                        to: to.clone(),
                        id: id.clone(),
                    },
                )?;
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge SET",
                )?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::PatchEdge {
                        from,
                        label: parsed.relationship.label,
                        to,
                        id,
                        props,
                    },
                ]));
            }
            self.bind_path_variable_for_edge_match(
                path_variable.as_deref(),
                &parsed,
                "MATCH edge SET",
            )?;
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge SET")?;
            self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::PatchMatchingEdges {
                    relationship,
                    patch: props,
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node SET")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH SET pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH SET requires the node pattern to bind the patch target".to_string(),
            ));
        };
        if node_variable != &assignment.target {
            return Err(cypher_syntax(format!(
                "MATCH SET target '{}' does not match node variable '{node_variable}'",
                assignment.target
            )));
        }
        let numeric_expression = match assignment.kind {
            PatchAssignmentKind::NumericExpression {
                key,
                source_target,
                source_key,
                op,
                operand,
            } => {
                if source_target != assignment.target {
                    return Err(cypher_unsupported_cardinality(
                        "MATCH SET numeric expressions cannot reference another variable",
                    ));
                }
                Some((key, source_key, op, operand))
            }
            PatchAssignmentKind::RemoveProperty { key } => {
                if (optional_string_prop(&node.props, "id").is_none()
                    && node
                        .variable
                        .as_ref()
                        .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
                    || !node.predicates.is_empty()
                {
                    let cardinality = match_node_cardinality(&node);
                    self.bind_row_node_variable(&node)?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::RemoveMatchingNodeProps {
                            label: node.label,
                            props: node.props,
                            predicates: node.predicates,
                            keys: vec![key],
                            cardinality,
                        },
                    ]));
                }
                let id = self.resolve_node_id(&node, "MATCH node SET")?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveNodeProps {
                        id,
                        keys: vec![key],
                    },
                ]));
            }
            PatchAssignmentKind::Props(props) => {
                if (optional_string_prop(&node.props, "id").is_none()
                    && node
                        .variable
                        .as_ref()
                        .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
                    || !node.predicates.is_empty()
                {
                    let cardinality = match_node_cardinality(&node);
                    self.bind_row_node_variable(&node)?;
                    return Ok(GraphMutationPlan::new(vec![
                        GraphMutationPlanOp::PatchMatchingNodes {
                            label: node.label,
                            props: node.props,
                            predicates: node.predicates,
                            patch: props,
                            cardinality,
                        },
                    ]));
                }
                let id = self.resolve_node_id(&node, "MATCH node SET")?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::PatchNode { id, props },
                ]));
            }
        };
        let (target_key, source_key, op, operand) = numeric_expression.expect("expression checked");
        if (optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
            || !node.predicates.is_empty()
        {
            let cardinality = match_node_cardinality(&node);
            self.bind_row_node_variable(&node)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::UpdateMatchingNodeProperty {
                    label: node.label,
                    props: node.props,
                    predicates: node.predicates,
                    target_key,
                    source_key,
                    op,
                    operand,
                    cardinality,
                },
            ]));
        }
        let id = self.resolve_node_id(&node, "MATCH node SET")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::UpdateMatchingNodeProperty {
                label: None,
                props: Props::from([("id".to_string(), Value::from(id.as_str()))]),
                predicates: Vec::new(),
                target_key,
                source_key,
                op,
                operand,
                cardinality: GraphMutationCardinality::SingleIdentity,
            },
        ]))
    }

    fn parse_match_remove(&mut self, statement: &str) -> Result<GraphMutationPlan> {
        let (pattern, target) = split_match_remove(statement)?;
        let (pattern, where_predicates) = split_match_where(pattern, &self.parameters)?;
        let (path_variable, pattern) = parse_path_binding(pattern, "MATCH REMOVE")?;
        let (target, key) = parse_property_ref(target, "MATCH REMOVE target")?;

        if find_unquoted_sequence(pattern, "->").is_some() {
            let mut parsed = self.parse_edge_match_pattern(pattern)?;
            apply_edge_where_predicates(&mut parsed, where_predicates, "MATCH edge REMOVE")?;
            let Some(edge_variable) = parsed.relationship.variable.clone() else {
                return Err(cypher_syntax(
                    "MATCH edge REMOVE requires the relationship pattern to bind the remove target",
                ));
            };
            if edge_variable != target {
                return Err(cypher_syntax(format!(
                    "MATCH edge REMOVE target '{target}' does not match relationship variable '{edge_variable}'"
                )));
            }
            let from_id = self.resolved_endpoint_id(&parsed.from)?;
            let to_id = self.resolved_endpoint_id(&parsed.to)?;
            if let (Some(from), Some(to)) = (from_id, to_id)
                && !has_relationship_predicates_beyond_id(&parsed.relationship.props)
                && parsed.from.predicates.is_empty()
                && parsed.to.predicates.is_empty()
                && parsed.relationship.predicates.is_empty()
            {
                let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
                self.bind_edge_variable(
                    &parsed.relationship,
                    CypherBoundEdgeIdentity {
                        from: from.clone(),
                        label: parsed.relationship.label.clone(),
                        to: to.clone(),
                        id: id.clone(),
                    },
                )?;
                self.bind_path_variable_for_edge_match(
                    path_variable.as_deref(),
                    &parsed,
                    "MATCH edge REMOVE",
                )?;
                return Ok(GraphMutationPlan::new(vec![
                    GraphMutationPlanOp::RemoveEdgeProps {
                        from,
                        label: parsed.relationship.label,
                        to,
                        id,
                        keys: vec![key],
                    },
                ]));
            }
            self.bind_path_variable_for_edge_match(
                path_variable.as_deref(),
                &parsed,
                "MATCH edge REMOVE",
            )?;
            let relationship = self.relationship_match_from_pattern(parsed, "MATCH edge REMOVE")?;
            self.bind_row_edge_match_variable(&edge_variable, &relationship)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingEdgeProps {
                    relationship,
                    keys: vec![key],
                    cardinality: GraphMutationCardinality::BoundedMany,
                },
            ]));
        }

        let (mut node, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        apply_node_where_predicates(&mut node, where_predicates, "MATCH node REMOVE")?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher MATCH REMOVE pattern suffix: {}",
                rest.trim()
            )));
        }
        let Some(node_variable) = &node.variable else {
            return Err(cypher_syntax(
                "MATCH REMOVE requires the node pattern to bind the remove target",
            ));
        };
        if node_variable != &target {
            return Err(cypher_syntax(format!(
                "MATCH REMOVE target '{target}' does not match node variable '{node_variable}'"
            )));
        }
        if (optional_string_prop(&node.props, "id").is_none()
            && node
                .variable
                .as_ref()
                .is_none_or(|variable| !self.node_bindings.contains_key(variable)))
            || !node.predicates.is_empty()
        {
            let cardinality = match_node_cardinality(&node);
            self.bind_row_node_variable(&node)?;
            return Ok(GraphMutationPlan::new(vec![
                GraphMutationPlanOp::RemoveMatchingNodeProps {
                    label: node.label,
                    props: node.props,
                    predicates: node.predicates,
                    keys: vec![key],
                    cardinality,
                },
            ]));
        }
        let id = self.resolve_node_id(&node, "MATCH node REMOVE")?;
        Ok(GraphMutationPlan::new(vec![
            GraphMutationPlanOp::RemoveNodeProps {
                id,
                keys: vec![key],
            },
        ]))
    }

    fn parse_edge_pattern(&mut self, pattern: &str) -> Result<ParsedCypherEdge> {
        let (from, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("-[")
            .ok_or_else(|| cypher_syntax("edge mutation requires a directed -[...]-> pattern"))?;
        let rel_end = find_matching(rest, '[', ']')?;
        let rel = &rest[..rel_end];
        let rest = rest[rel_end + 1..].trim_start();
        let rest = rest
            .strip_prefix("->")
            .ok_or_else(|| cypher_syntax("edge mutation requires outgoing '->' direction"))?;
        let (to, rest) = parse_cypher_node_pattern(rest, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }

        let from_id = self.resolve_node_id(&from, "edge mutation source node")?;
        let to_id = self.resolve_node_id(&to, "edge mutation destination node")?;
        let relationship = parse_cypher_relationship(rel, &self.parameters)?;
        let mut edge = Edge::new(
            relationship.label.clone(),
            from_id.clone(),
            to_id.clone(),
            relationship.props.clone(),
        );
        if let Some(id) = edge
            .props
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        {
            edge = edge.with_id(id);
        }
        self.bind_edge_variable(
            &relationship,
            CypherBoundEdgeIdentity {
                from: edge.from.clone(),
                label: edge.label.clone(),
                to: edge.to.clone(),
                id: edge.id.clone(),
            },
        )?;
        Ok(ParsedCypherEdge {
            from_id,
            to_id,
            edge,
        })
    }

    fn parse_edge_match_pattern(&self, pattern: &str) -> Result<ParsedCypherEdgeMatch> {
        let (from, rest) = parse_cypher_node_pattern(pattern, &self.parameters)?;
        let rest = rest.trim_start();
        let rest = rest
            .strip_prefix("-[")
            .ok_or_else(|| cypher_syntax("edge mutation requires a directed -[...]-> pattern"))?;
        let rel_end = find_matching(rest, '[', ']')?;
        let rel = &rest[..rel_end];
        let rest = rest[rel_end + 1..].trim_start();
        let rest = rest
            .strip_prefix("->")
            .ok_or_else(|| cypher_syntax("edge mutation requires outgoing '->' direction"))?;
        let (to, rest) = parse_cypher_node_pattern(rest, &self.parameters)?;
        if !rest.trim().is_empty() {
            return Err(cypher_syntax(format!(
                "unsupported writable Cypher edge pattern suffix: {}",
                rest.trim()
            )));
        }
        let relationship = parse_cypher_relationship(rel, &self.parameters)?;
        Ok(ParsedCypherEdgeMatch {
            from,
            relationship,
            to,
        })
    }

    fn relationship_match_from_pattern(
        &mut self,
        parsed: ParsedCypherEdgeMatch,
        context: &str,
    ) -> Result<GraphRelationshipMatch> {
        let id = optional_string_prop(&parsed.relationship.props, "id").map(EdgeId::new);
        let mut props = parsed.relationship.props;
        props.remove("id");
        let from = self.node_match_from_pattern(parsed.from, context)?;
        let to = self.node_match_from_pattern(parsed.to, context)?;
        Ok(GraphRelationshipMatch {
            from,
            label: parsed.relationship.label,
            to,
            id,
            props,
            predicates: parsed.relationship.predicates,
        })
    }

    fn node_match_from_pattern(
        &mut self,
        node: ParsedCypherNode,
        context: &str,
    ) -> Result<GraphNodeMatch> {
        if node.props.contains_key("id") && optional_string_prop(&node.props, "id").is_none() {
            return Err(cypher_unresolved_identity(format!(
                "{context} endpoint id must be a string"
            )));
        }
        let mut props = node.props.clone();
        if let Some(id) = self.resolved_endpoint_id(&node)?
            && !props.contains_key("id")
        {
            props.insert("id".to_string(), Value::from(id.as_str()));
        }
        Ok(GraphNodeMatch {
            label: node.label,
            props,
            predicates: node.predicates,
        })
    }

    fn resolved_endpoint_id(&mut self, node: &ParsedCypherNode) -> Result<Option<NodeId>> {
        if node.props.contains_key("id") && optional_string_prop(&node.props, "id").is_none() {
            return Err(cypher_unresolved_identity(
                "edge mutation endpoint id must be a string",
            ));
        }
        if let Some(id) = optional_string_prop(&node.props, "id") {
            let id = NodeId::new(id);
            self.bind_node_variable(node, &id)?;
            return Ok(Some(id));
        }
        if let Some(variable) = &node.variable
            && let Some(id) = self.node_bindings.get(variable)
        {
            return Ok(Some(id.clone()));
        }
        Ok(None)
    }

    fn resolve_node_id(&mut self, node: &ParsedCypherNode, context: &str) -> Result<NodeId> {
        if let Some(id) = optional_string_prop(&node.props, "id") {
            let id = NodeId::new(id);
            self.bind_node_variable(node, &id)?;
            return Ok(id);
        }
        if let Some(variable) = &node.variable {
            if let Some(id) = self.node_bindings.get(variable) {
                return Ok(id.clone());
            }
            return Err(cypher_unresolved_identity(format!(
                "{context} variable '{variable}' is not bound to a node id"
            )));
        }
        Err(cypher_unresolved_identity(format!(
            "{context} requires explicit string property 'id'"
        )))
    }

    fn bind_node_variable(&mut self, node: &ParsedCypherNode, id: &NodeId) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        if let Some(existing) = self.node_bindings.get(variable) {
            if existing != id {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher variable '{variable}' is already bound to node id '{}'",
                    existing.as_str()
                )));
            }
            return Ok(());
        }
        self.node_bindings.insert(variable.clone(), id.clone());
        Ok(())
    }

    fn bind_edge_variable(
        &mut self,
        relationship: &ParsedCypherRelationship,
        identity: CypherBoundEdgeIdentity,
    ) -> Result<()> {
        let Some(variable) = &relationship.variable else {
            return Ok(());
        };
        if let Some(existing) = self.edge_bindings.get(variable) {
            if existing != &identity {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher relationship variable '{variable}' is already bound to a different edge identity"
                )));
            }
            return Ok(());
        }
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        self.edge_bindings.insert(variable.clone(), identity);
        Ok(())
    }

    fn bind_row_node_variable(&mut self, node: &ParsedCypherNode) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
            || self.row_path_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a relationship"
            )));
        }
        let binding = GraphNodeMatch {
            label: node.label.clone(),
            props: node.props.clone(),
            predicates: node.predicates.clone(),
        };
        if let Some(existing) = self.row_node_bindings.get(variable) {
            if existing != &binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher variable '{variable}' is already bound to a different node match"
                )));
            }
            return Ok(());
        }
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        self.row_node_bindings.insert(variable.clone(), binding);
        Ok(())
    }

    fn bind_row_edge_match_variable(
        &mut self,
        variable: &str,
        binding: &GraphRelationshipMatch,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable)
            || self.row_node_bindings.contains_key(variable)
            || self.row_path_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher relationship variable '{variable}' is already bound"
            )));
        }
        if let Some(existing) = self.row_edge_match_bindings.get(variable) {
            if existing != binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher relationship variable '{variable}' is already bound to a different relationship match"
                )));
            }
            return Ok(());
        }
        self.row_edge_match_bindings
            .insert(variable.to_string(), binding.clone());
        Ok(())
    }

    fn bind_row_edge_variable(
        &mut self,
        variable: &str,
        binding: CypherRowProducedEdgeBinding,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher variable '{variable}' is already bound to a node id"
            )));
        }
        if self.row_path_bindings.contains_key(variable) {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher relationship variable '{variable}' is already bound"
            )));
        }
        self.row_edge_bindings.insert(variable.to_string(), binding);
        Ok(())
    }

    fn bind_row_path_variable(
        &mut self,
        variable: &str,
        binding: CypherRowProducedPathBinding,
    ) -> Result<()> {
        if self.node_bindings.contains_key(variable)
            || self.row_node_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound to a node"
            )));
        }
        if self.edge_bindings.contains_key(variable)
            || self.row_edge_match_bindings.contains_key(variable)
            || self.row_edge_bindings.contains_key(variable)
        {
            return Err(cypher_unresolved_identity(format!(
                "Cypher path variable '{variable}' is already bound to a relationship"
            )));
        }
        if let Some(existing) = self.row_path_bindings.get(variable) {
            if existing != &binding {
                return Err(cypher_unresolved_identity(format!(
                    "Cypher path variable '{variable}' is already bound to a different path"
                )));
            }
            return Ok(());
        }
        self.row_path_bindings.insert(variable.to_string(), binding);
        Ok(())
    }

    fn bind_path_variable_for_edge_match(
        &mut self,
        path_variable: Option<&str>,
        parsed: &ParsedCypherEdgeMatch,
        context: &str,
    ) -> Result<()> {
        let Some(path_variable) = path_variable else {
            return Ok(());
        };
        let Some(from_variable) = parsed.from.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the source node pattern to bind a variable"
            )));
        };
        let Some(edge_variable) = parsed.relationship.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the relationship pattern to bind a variable"
            )));
        };
        let Some(to_variable) = parsed.to.variable.as_ref() else {
            return Err(cypher_syntax(format!(
                "{context} path variables require the destination node pattern to bind a variable"
            )));
        };
        if !self.node_bindings.contains_key(from_variable) {
            self.bind_row_node_variable(&parsed.from)?;
        }
        if !self.node_bindings.contains_key(to_variable) {
            self.bind_row_node_variable(&parsed.to)?;
        }
        self.bind_row_path_variable(
            path_variable,
            CypherRowProducedPathBinding {
                from_variable: from_variable.clone(),
                edge_variable: edge_variable.clone(),
                to_variable: to_variable.clone(),
            },
        )
    }

    fn promote_node_variable_to_row_binding(&mut self, node: &ParsedCypherNode) -> Result<()> {
        let Some(variable) = &node.variable else {
            return Ok(());
        };
        self.node_bindings.remove(variable);
        self.bind_row_node_variable(node)
    }
}

