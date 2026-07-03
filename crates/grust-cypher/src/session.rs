//! Named graph selection helpers for the portable single-graph executor.

use std::collections::BTreeMap;

use crate::ast::{Clause, Query};
use crate::lexer::{Keyword, Token, tokenize};
use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub struct CypherSession {
    pub current_graph: String,
    pub settings: BTreeMap<String, Value>,
}

impl Default for CypherSession {
    fn default() -> Self {
        Self {
            current_graph: "default".to_string(),
            settings: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionCommand {
    UseGraph(String),
    Set { name: String, value: Value },
    Reset { name: String },
    ResetAll,
}

impl SessionCommand {
    pub fn parse(source: &str) -> Result<Option<Self>> {
        let tokens = tokenize_session(source)?;
        let Some(first) = tokens.first() else {
            return Ok(None);
        };
        match first {
            Token::Keyword(Keyword::Use) => parse_use_command(&tokens).map(Some),
            Token::Keyword(Keyword::Set) => parse_set_command(&tokens).map(Some),
            Token::Identifier(word) if word.eq_ignore_ascii_case("reset") => {
                parse_reset_command(&tokens).map(Some)
            }
            _ => Ok(None),
        }
    }

    pub fn apply(
        self,
        session: &mut CypherSession,
        catalog: Option<&CypherCatalogSnapshot>,
    ) -> Result<()> {
        match self {
            SessionCommand::UseGraph(graph) => {
                if let Some(catalog) = catalog {
                    ensure_catalog_graph_selection(catalog, &graph)?;
                }
                session.current_graph = graph;
            }
            SessionCommand::Set { name, value } => {
                session.settings.insert(name, value);
            }
            SessionCommand::Reset { name } => {
                session.settings.remove(&name);
            }
            SessionCommand::ResetAll => {
                session.settings.clear();
            }
        }
        Ok(())
    }
}

pub fn query_graph_selection(query: &Query) -> Result<Option<String>> {
    let mut selected: Option<String> = None;
    for part in &query.parts {
        for clause in &part.query.clauses {
            let Clause::Use(use_clause) = clause else {
                continue;
            };
            if let Some(existing) = &selected {
                if existing != &use_clause.graph {
                    return Err(gql_execution(format!(
                        "query selects multiple graphs: `{existing}` and `{}`",
                        use_clause.graph
                    )));
                }
            } else {
                selected = Some(use_clause.graph.clone());
            }
        }
    }
    Ok(selected)
}

fn tokenize_session(source: &str) -> Result<Vec<Token>> {
    tokenize(source)
        .map_err(|err| err.into_grust(source))
        .map(|spanned| {
            spanned
                .into_iter()
                .map(|item| item.token)
                .filter(|token| !matches!(token, Token::Eof | Token::Semicolon))
                .collect()
        })
}

fn parse_use_command(tokens: &[Token]) -> Result<SessionCommand> {
    if tokens.len() != 2 {
        return Err(gql_syntax(
            "USE session command expects exactly one graph name",
        ));
    }
    Ok(SessionCommand::UseGraph(token_name(
        &tokens[1],
        "graph name",
    )?))
}

fn parse_set_command(tokens: &[Token]) -> Result<SessionCommand> {
    if tokens.len() != 4 || !matches!(tokens[2], Token::Eq) {
        return Err(gql_syntax(
            "SET session command expects `SET name = literal`",
        ));
    }
    Ok(SessionCommand::Set {
        name: token_name(&tokens[1], "setting name")?,
        value: token_literal(&tokens[3])?,
    })
}

fn parse_reset_command(tokens: &[Token]) -> Result<SessionCommand> {
    if tokens.len() != 2 {
        return Err(gql_syntax(
            "RESET session command expects one setting name or ALL",
        ));
    }
    if tokens[1].is_keyword(Keyword::All) {
        Ok(SessionCommand::ResetAll)
    } else {
        Ok(SessionCommand::Reset {
            name: token_name(&tokens[1], "setting name")?,
        })
    }
}

fn token_name(token: &Token, what: &str) -> Result<String> {
    match token {
        Token::Identifier(name) | Token::QuotedIdentifier(name) => Ok(name.clone()),
        _ => Err(gql_syntax(format!("expected {what}"))),
    }
}

fn token_literal(token: &Token) -> Result<Value> {
    match token {
        Token::String(value) => Ok(Value::from(value.clone())),
        Token::Integer(value) => Ok(Value::from(*value)),
        Token::Float(value) => Ok(Value::from(*value)),
        Token::Keyword(Keyword::True) => Ok(Value::from(true)),
        Token::Keyword(Keyword::False) => Ok(Value::from(false)),
        Token::Keyword(Keyword::Null) => Ok(Value::Null),
        _ => Err(gql_syntax("SET session command value must be a literal")),
    }
}

pub fn ensure_query_uses_graph(query: &Query, graph_name: &str) -> Result<()> {
    let Some(selected) = query_graph_selection(query)? else {
        return Ok(());
    };
    if selected == graph_name {
        Ok(())
    } else {
        Err(unsupported_gql_feature(
            GqlFeature::NamedGraphSelection,
            GqlConformanceProfile::Full39075,
            format!(
                "single-graph execution is bound to `{graph_name}` but query requested `{selected}`"
            ),
        ))
    }
}

pub fn ensure_catalog_graph_selection(
    catalog: &CypherCatalogSnapshot,
    graph_name: &str,
) -> Result<NamedGraphCatalog> {
    catalog
        .graphs
        .iter()
        .find(|graph| graph.name == graph_name)
        .cloned()
        .ok_or_else(|| {
            unsupported_gql_feature(
                GqlFeature::NamedGraphSelection,
                GqlConformanceProfile::Full39075,
                format!("catalog does not contain graph `{graph_name}`"),
            )
        })
}
