use super::*;

pub(super) struct Triangle<'a> {
    pub(super) person_label: &'a str,
    pub(super) city_label: &'a str,
    pub(super) country_label: &'a str,
    pub(super) located_type: &'a str,
    pub(super) part_type: &'a str,
    pub(super) knows_type: &'a str,
    pub(super) projection: &'a Projection,
}

#[derive(Clone, Copy)]
struct Arm<'a> {
    person: &'a str,
    person_label: &'a str,
    city: &'a str,
    city_label: &'a str,
    located_type: &'a str,
    part_type: &'a str,
}

pub(super) fn scalar_bound(expr: Option<&Expr>) -> Option<u64> {
    match expr {
        None => Some(0),
        Some(Expr::Integer(value)) if *value >= 0 => Some(*value as u64),
        _ => None,
    }
}

fn plain_path(path: &PathPattern) -> bool {
    path.variable.is_none() && path.shortest.is_none()
}

fn labelled_node(node: &NodePattern) -> Option<(&str, &str)> {
    let [label] = node.labels.as_slice() else {
        return None;
    };
    if node.properties.is_some() {
        return None;
    }
    Some((node.variable.as_deref()?, label.as_str()))
}

fn reference_node(node: &NodePattern) -> Option<&str> {
    if !node.labels.is_empty() || node.properties.is_some() {
        return None;
    }
    node.variable.as_deref()
}

fn relationship(rel: &RelationshipPattern, direction: ast::Direction) -> Option<&str> {
    let [kind] = rel.types.as_slice() else {
        return None;
    };
    if rel.variable.is_some()
        || rel.length.is_some()
        || rel.properties.is_some()
        || rel.direction != direction
    {
        return None;
    }
    Some(kind)
}

fn arm<'a>(clause: &'a MatchClause, country: &str) -> Option<Arm<'a>> {
    if clause.optional || clause.where_clause.is_some() {
        return None;
    }
    let [path] = clause.patterns.as_slice() else {
        return None;
    };
    let [located, part] = path.segments.as_slice() else {
        return None;
    };
    if !plain_path(path) || reference_node(&part.node)? != country {
        return None;
    }
    let (person, person_label) = labelled_node(&path.start)?;
    let (city, city_label) = labelled_node(&located.node)?;
    Some(Arm {
        person,
        person_label,
        city,
        city_label,
        located_type: relationship(&located.relationship, ast::Direction::Outgoing)?,
        part_type: relationship(&part.relationship, ast::Direction::Outgoing)?,
    })
}

fn scalar_projection(ret: &ReturnClause) -> Option<&Projection> {
    let projection = &ret.projection;
    if projection.star
        || projection.distinct
        || projection.items.len() != 1
        || !projection.order_by.is_empty()
        || scalar_bound(projection.skip.as_ref()).is_none()
        || scalar_bound(projection.limit.as_ref()).is_none()
        || !matches!(&projection.items[0].expr,
            Expr::Function { name, distinct: false, star: true, args }
            if name.eq_ignore_ascii_case("count") && args.is_empty())
    {
        return None;
    }
    Some(projection)
}

/// Prove the symmetric q3 family directly from the AST. Names and type labels
/// are not hard-coded, but clause boundaries and every symmetry used by the
/// algebra are part of the proof. The seven variables fit fixed arrays, so
/// classification cannot allocate based on query-controlled cardinality.
pub(super) fn plan(query: &Query) -> Result<Option<Triangle<'_>>> {
    read_budget::checkpoint()?;
    if query.parts.len() != 1 || query.parts[0].union.is_some() {
        return Ok(None);
    }
    let [
        Clause::Match(root),
        Clause::Match(first),
        Clause::Match(second),
        Clause::Match(third),
        Clause::Match(close),
        Clause::Return(ret),
    ] = query.parts[0].query.clauses.as_slice()
    else {
        return Ok(None);
    };
    read_budget::charge_candidate_work(16, "proving count triangle")?;

    if root.optional || root.where_clause.is_some() {
        return Ok(None);
    }
    let [root_path] = root.patterns.as_slice() else {
        return Ok(None);
    };
    if !plain_path(root_path) || !root_path.segments.is_empty() {
        return Ok(None);
    }
    let Some((country, country_label)) = labelled_node(&root_path.start) else {
        return Ok(None);
    };

    let [Some(first), Some(second), Some(third)] = [
        arm(first, country),
        arm(second, country),
        arm(third, country),
    ] else {
        return Ok(None);
    };
    let arms = [first, second, third];
    let model = &arms[0];
    if model.located_type == model.part_type
        || arms.iter().any(|candidate| {
            candidate.person_label != model.person_label
                || candidate.city_label != model.city_label
                || candidate.located_type != model.located_type
                || candidate.part_type != model.part_type
        })
    {
        return Ok(None);
    }
    let variables = [
        country,
        arms[0].person,
        arms[0].city,
        arms[1].person,
        arms[1].city,
        arms[2].person,
        arms[2].city,
    ];
    if variables
        .iter()
        .enumerate()
        .any(|(index, name)| variables[..index].contains(name))
    {
        return Ok(None);
    }

    if close.optional || close.where_clause.is_some() {
        return Ok(None);
    }
    let [path] = close.patterns.as_slice() else {
        return Ok(None);
    };
    let [ab, bc, ca] = path.segments.as_slice() else {
        return Ok(None);
    };
    if !plain_path(path) {
        return Ok(None);
    }
    let Some(start) = reference_node(&path.start) else {
        return Ok(None);
    };
    let Some(b) = reference_node(&ab.node) else {
        return Ok(None);
    };
    let Some(c) = reference_node(&bc.node) else {
        return Ok(None);
    };
    if reference_node(&ca.node) != Some(start) || start == b || start == c || b == c {
        return Ok(None);
    }
    let people = [arms[0].person, arms[1].person, arms[2].person];
    if ![start, b, c].iter().all(|name| people.contains(name)) {
        return Ok(None);
    }
    let Some(knows_type) = relationship(&ab.relationship, ast::Direction::Undirected) else {
        return Ok(None);
    };
    if relationship(&bc.relationship, ast::Direction::Undirected) != Some(knows_type)
        || relationship(&ca.relationship, ast::Direction::Undirected) != Some(knows_type)
        || knows_type == model.located_type
        || knows_type == model.part_type
    {
        return Ok(None);
    }
    let Some(projection) = scalar_projection(ret) else {
        return Ok(None);
    };
    Ok(Some(Triangle {
        person_label: model.person_label,
        city_label: model.city_label,
        country_label,
        located_type: model.located_type,
        part_type: model.part_type,
        knows_type,
        projection,
    }))
}
