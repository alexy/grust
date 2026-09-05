//! Opt-in scalar aggregation over already-supported SQL match sources.

use super::*;

/// A single server-side `COUNT(*)`, with final projection/pagination in Rust.
///
/// This is deliberately separate from [`ReadPushdown`]: existing consumers,
/// including Sail, retain their row-source execution contract unless they
/// explicitly call [`plan_scalar_count_read`]. The SQL emits exactly one
/// non-null integer cell, including zero when the match source is empty.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarCountReadPushdown {
    source: ReadPushdown,
    projection: Projection,
    equalities: Vec<StringEquality>,
}

#[derive(Clone, Debug, PartialEq)]
struct StringEquality {
    props_column: String,
    key: String,
    value: String,
}

/// Plan one non-optional `MATCH ... RETURN count(*) [AS alias] [SKIP ...]
/// [LIMIT ...]` using the existing node, fixed-segment, or directed multi-pattern
/// SQL lowering. Returns `Ok(None)` when this conservative subset is not proven.
///
/// No grouping, `DISTINCT`, `ORDER BY`, `WITH`, `UNION`, repeated `MATCH`,
/// `OPTIONAL MATCH`, variable-length paths, or new filter forms are introduced.
/// Filters are restricted to conjunctions of real property equality against
/// string literals/parameters. Numeric, boolean, synthetic-label and all other
/// predicates remain on the existing row-source/reference routes. A dialect
/// must opt into exact, JSON-type-checked string equality via
/// [`SqlDialect::exact_string_property_eq`]; call [`ScalarCountReadPushdown::supported_by`]
/// before selecting this execution route. Structural node labels are supported.
/// Multiple relationship positions must have
/// pairwise-disjoint type sets: otherwise the existing SQL joins do not prove
/// relationship uniqueness. A fixed-segment path may be undirected; the
/// existing OR joins count each self-loop once and other orientations once
/// each. Undirected multi-pattern matches retain the existing fallback.
///
/// Pagination is applied to the aggregate's one output row, never to its match
/// source, using the shared reference projection and the execution parameters.
pub fn plan_scalar_count_read(
    cypher: &str,
    params: &CypherParameters,
    hints: &dyn TypeHints,
) -> Result<Option<ScalarCountReadPushdown>> {
    Ok(plan_read(cypher, params, hints)?.and_then(|source| source.scalar_count_read()))
}

impl ReadPushdown {
    /// Opt into scalar aggregation for an already-planned match source, with
    /// the same eligibility as [`plan_scalar_count_read`]. Backends trying both
    /// scalar and row-source plans can parse/analyze the query just once.
    pub fn scalar_count_read(&self) -> Option<ScalarCountReadPushdown> {
        let mut equalities = Vec::new();
        let (projection, edge_types): (&Projection, Vec<&[String]>) = match self {
            Self::Node(source) => {
                if let Some(filter) = &source.filter {
                    node_equalities(filter, &mut equalities)?;
                }
                (&source.projection, vec![])
            }
            Self::Segment(source) => {
                if let Some(filter) = &source.filter {
                    segment_equalities(filter, &mut equalities)?;
                }
                (
                    &source.projection,
                    source
                        .segments
                        .iter()
                        .map(|segment| segment.rel_types.as_slice())
                        .collect(),
                )
            }
            Self::MultiPattern(source) => {
                if let Some(filter) = &source.filter {
                    segment_equalities(filter, &mut equalities)?;
                }
                (
                    &source.projection,
                    source
                        .edges
                        .iter()
                        .map(|edge| edge.rel_types.as_slice())
                        .collect(),
                )
            }
            _ => return None,
        };
        if projection.star || projection.distinct || !projection.order_by.is_empty() {
            return None;
        }
        let [item] = projection.items.as_slice() else {
            return None;
        };
        if !matches!(&item.expr, Expr::Function { name, distinct: false, star: true, args }
            if name.eq_ignore_ascii_case("count") && args.is_empty())
        {
            return None;
        }

        if !pairwise_disjoint_types(edge_types) {
            return None;
        }
        Some(ScalarCountReadPushdown {
            source: self.clone(),
            projection: projection.clone(),
            equalities,
        })
    }
}

impl ScalarCountReadPushdown {
    /// Whether every scalar predicate has exact semantics on this dialect.
    /// This is the same capability proof used by the fail-closed renderer.
    pub fn supported_by(&self, dialect: &dyn SqlDialect) -> bool {
        self.render_filter(dialect).is_some()
    }

    fn render_filter(&self, dialect: &dyn SqlDialect) -> Option<Option<String>> {
        if !self.source.supported_by(dialect) {
            return None;
        }
        let predicates = self
            .equalities
            .iter()
            .map(|equality| {
                dialect.exact_string_property_eq(
                    &equality.props_column,
                    &equality.key,
                    &equality.value,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        Some((!predicates.is_empty()).then(|| predicates.join(" AND ")))
    }

    /// Render exactly one SQL aggregate column over the proven match source.
    /// Unsupported exact predicates return an error; they never silently use
    /// the older row-source coercion rules.
    pub fn to_sql(&self, dialect: &dyn SqlDialect) -> Result<String> {
        let filter = self.render_filter(dialect).ok_or_else(|| {
            crate::gql::gql_execution("dialect cannot render exact scalar-count predicates")
        })?;
        Ok(match &self.source {
            ReadPushdown::Node(source) => {
                source.to_sql_select_with_filter(dialect, "COUNT(*)", filter.as_deref())
            }
            ReadPushdown::Segment(source) => {
                source.to_sql_select_with_filter(dialect, "COUNT(*)", filter.as_deref())
            }
            ReadPushdown::MultiPattern(source) => {
                source.to_sql_select_with_filter(dialect, "COUNT(*)", filter.as_deref())
            }
            _ => unreachable!("scalar count only admits fixed match sources"),
        })
    }

    /// The SQL result is always one column, regardless of bound variables.
    pub fn column_count(&self) -> usize {
        1
    }

    /// Decode a nonnegative SQL bigint, then apply the original alias and
    /// final `SKIP`/`LIMIT` using the reference projection on one scalar row.
    /// Malformed, null, negative, or overflowing backend counts are errors.
    pub fn project_text_rows(
        &self,
        rows: Vec<Vec<Option<String>>>,
        params: &CypherParameters,
    ) -> Result<CypherResultTable> {
        let count = match rows.as_slice() {
            [row] => match row.as_slice() {
                [Some(value)] => value.parse::<i64>().ok().filter(|count| *count >= 0),
                _ => None,
            },
            _ => None,
        }
        .ok_or_else(|| {
            crate::gql::gql_execution("SQL COUNT(*) requires one nonnegative bigint cell")
        })?;
        let mut projection = self.projection.clone();
        // Both a function and an integer have the reference default column
        // name `expr`; an explicit alias remains unchanged.
        projection.items[0].expr = Expr::Integer(count);
        crate::read::project_bindings(vec![vec![]], &projection, params)
    }
}

fn node_equalities(predicate: &Predicate, equalities: &mut Vec<StringEquality>) -> Option<()> {
    match predicate {
        Predicate::And(left, right) => {
            node_equalities(left, equalities)?;
            node_equalities(right, equalities)
        }
        Predicate::Compare {
            prop: PropRef::Key(key),
            op: CmpOp::Eq,
            value: Scalar::Str(value),
        } => {
            equalities.push(StringEquality {
                props_column: "props".into(),
                key: key.clone(),
                value: value.clone(),
            });
            Some(())
        }
        // Inline {label: ...} and WHERE n.label share a lowered operand.
        // Reject both rather than confusing real properties with node labels.
        _ => None,
    }
}

fn segment_equalities(
    predicate: &SegPredicate,
    equalities: &mut Vec<StringEquality>,
) -> Option<()> {
    match predicate {
        SegPredicate::And(left, right) => {
            segment_equalities(left, equalities)?;
            segment_equalities(right, equalities)
        }
        SegPredicate::Compare {
            operand,
            op: CmpOp::Eq,
            value: Scalar::Str(value),
        } => {
            let (props_column, key) = match operand {
                SegOperand::NodeProp(node, key) => (format!("n{node}.props"), key),
                SegOperand::EdgeProp(edge, key) => (format!("e{edge}.props"), key),
                SegOperand::NodeLabel(_) => return None,
            };
            equalities.push(StringEquality {
                props_column,
                key: key.clone(),
                value: value.clone(),
            });
            Some(())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
