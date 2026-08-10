use std::collections::{BTreeMap, BTreeSet};

use artist_formal::{GraphBuilder, GraphError, Literal, ObjectGraph, ObjectId, ObjectNode, Symbol};
use thiserror::Error;

use crate::{Name, Term};

const ROOT: &str = "main";
const SYMBOL: &str = "artist.core/symbol";
const DOMAIN: &str = "artist.core/domain";
const CODOMAIN: &str = "artist.core/codomain";
const BODY: &str = "artist.core/body";
const FUNCTION: &str = "artist.core/function";
const ARGUMENT: &str = "artist.core/argument";
const VALUE_TYPE: &str = "artist.core/value-type";
const VALUE: &str = "artist.core/value";
const FIRST: &str = "artist.core/first";
const SECOND: &str = "artist.core/second";
const PAIR_VALUE: &str = "artist.core/pair";
const TYPE_VALUE: &str = "artist.core/type";
const LHS: &str = "artist.core/lhs";
const RHS: &str = "artist.core/rhs";
const MOTIVE: &str = "artist.core/motive";
const REFL_CASE: &str = "artist.core/refl-case";
const EQUALITY: &str = "artist.core/equality";
const PARAMETER: &str = "artist.core/parameter";
const FAMILY_INDEX: &str = "artist.core/family-index";
const BRANCH: &str = "artist.core/branch";
const SCRUTINEE: &str = "artist.core/scrutinee";
const LEVEL: &str = "artist.core/level";
const INDEX: &str = "artist.core/index";
const NAME: &str = "artist.core/name";
const INDUCTIVE_NAME: &str = "artist.core/inductive-name";

const UNIVERSE: &str = "artist.kernel/universe";
const VAR: &str = "artist.kernel/var";
const CONST: &str = "artist.kernel/const";
const PI: &str = "artist.kernel/pi";
const SIGMA: &str = "artist.kernel/sigma";
const LAM: &str = "artist.kernel/lam";
const APP: &str = "artist.kernel/app";
const PAIR: &str = "artist.kernel/pair";
const FST: &str = "artist.kernel/fst";
const SND: &str = "artist.kernel/snd";
const ID: &str = "artist.kernel/id";
const REFL: &str = "artist.kernel/refl";
const J: &str = "artist.kernel/j";
const ELIM: &str = "artist.kernel/elim";
const LET: &str = "artist.kernel/let";

/// Errors while elaborating the standard core graph encoding.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum CoreCodecError {
    /// Malformed graph references.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The main root is absent.
    #[error("core graph has no main root")]
    MissingRoot,
    /// A core term node has no operator.
    #[error("core term object {0} has no operator")]
    MissingOperator(ObjectId),
    /// An operator object lacks its symbolic name.
    #[error("operator object {0} has no text artist.core/symbol property")]
    MalformedOperator(ObjectId),
    /// The standard elaborator does not understand the operator. The graph remains
    /// representable and may be handled by another theory-specific elaborator.
    #[error("unknown core operator {symbol} at object {object}")]
    UnknownOperator { object: ObjectId, symbol: String },
    /// A required edge is missing or non-singleton.
    #[error("object {object} requires exactly one {role} edge")]
    EdgeArity { object: ObjectId, role: Symbol },
    /// A required scalar property is malformed.
    #[error("object {object} has malformed property {property}")]
    Property { object: ObjectId, property: Symbol },
    /// Core terms are finite trees/DAGs, so a cycle cannot elaborate into the trusted
    /// calculus even though it remains valid universal syntax.
    #[error("cycle encountered while elaborating core term at {0}")]
    CyclicCoreTerm(ObjectId),
}

/// Standard, lossless encoding between trusted kernel terms and the open graph.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreGraphCodec;

impl CoreGraphCodec {
    /// Encodes a core term. Operator objects remain first-class graph objects.
    pub fn encode(term: &Term) -> Result<ObjectGraph, GraphError> {
        let mut builder = GraphBuilder::new();
        let operators = install_operators(&mut builder);
        let root = encode_term(&mut builder, &operators, term);
        builder.root(ROOT, root);
        builder.finish()
    }

    /// Elaborates only the explicit `artist.kernel/*` graph vocabulary into one
    /// finite kernel term. This is not the graph's structural interpretation.
    pub fn elaborate(graph: &ObjectGraph) -> Result<Term, CoreCodecError> {
        graph.validate()?;
        let root = graph
            .roots
            .get(&Symbol::from(ROOT))
            .ok_or(CoreCodecError::MissingRoot)?;
        let mut visiting = BTreeSet::new();
        let mut memo = BTreeMap::new();
        decode_term(graph, root, &mut visiting, &mut memo)
    }

    /// Backward-compatible alias for [`Self::elaborate`].
    pub fn decode(graph: &ObjectGraph) -> Result<Term, CoreCodecError> {
        Self::elaborate(graph)
    }
}

fn install_operators(builder: &mut GraphBuilder) -> BTreeMap<&'static str, ObjectId> {
    [
        UNIVERSE, VAR, CONST, PI, SIGMA, LAM, APP, PAIR, FST, SND, ID, REFL, J, ELIM, LET,
    ]
    .into_iter()
    .map(|name| {
        let id = builder.insert(
            format!("op:{name}"),
            ObjectNode::new().with_property(SYMBOL, Literal::Text(name.to_owned())),
        );
        (name, id)
    })
    .collect()
}

fn encode_term(
    builder: &mut GraphBuilder,
    operators: &BTreeMap<&'static str, ObjectId>,
    term: &Term,
) -> ObjectId {
    match term {
        Term::Universe { level } => builder.alloc(
            ObjectNode::new()
                .with_operator(operators[UNIVERSE].clone())
                .with_property(LEVEL, Literal::Integer(level.to_string())),
        ),
        Term::Var { index } => builder.alloc(
            ObjectNode::new()
                .with_operator(operators[VAR].clone())
                .with_property(INDEX, Literal::Integer(index.to_string())),
        ),
        Term::Const { name } => builder.alloc(
            ObjectNode::new()
                .with_operator(operators[CONST].clone())
                .with_property(NAME, Literal::Text(name.0.clone())),
        ),
        Term::Pi { domain, codomain } => {
            encode_binary(builder, operators, PI, DOMAIN, domain, CODOMAIN, codomain)
        }
        Term::Sigma { domain, codomain } => encode_binary(
            builder, operators, SIGMA, DOMAIN, domain, CODOMAIN, codomain,
        ),
        Term::Lam { domain, body } => {
            encode_binary(builder, operators, LAM, DOMAIN, domain, BODY, body)
        }
        Term::App { function, argument } => encode_binary(
            builder, operators, APP, FUNCTION, function, ARGUMENT, argument,
        ),
        Term::Pair { first, second } => {
            encode_binary(builder, operators, PAIR, FIRST, first, SECOND, second)
        }
        Term::Fst { pair } => encode_unary(builder, operators, FST, PAIR_VALUE, pair),
        Term::Snd { pair } => encode_unary(builder, operators, SND, PAIR_VALUE, pair),
        Term::Id { ty, lhs, rhs } => {
            let ty = encode_term(builder, operators, ty);
            let lhs = encode_term(builder, operators, lhs);
            let rhs = encode_term(builder, operators, rhs);
            builder.alloc(
                ObjectNode::new()
                    .with_operator(operators[ID].clone())
                    .with_edge(TYPE_VALUE, ty)
                    .with_edge(LHS, lhs)
                    .with_edge(RHS, rhs),
            )
        }
        Term::Refl { value } => encode_unary(builder, operators, REFL, VALUE, value),
        Term::J {
            ty,
            motive,
            refl_case,
            lhs,
            rhs,
            equality,
        } => {
            let ty = encode_term(builder, operators, ty);
            let motive = encode_term(builder, operators, motive);
            let refl_case = encode_term(builder, operators, refl_case);
            let lhs = encode_term(builder, operators, lhs);
            let rhs = encode_term(builder, operators, rhs);
            let equality = encode_term(builder, operators, equality);
            builder.alloc(
                ObjectNode::new()
                    .with_operator(operators[J].clone())
                    .with_edge(TYPE_VALUE, ty)
                    .with_edge(MOTIVE, motive)
                    .with_edge(REFL_CASE, refl_case)
                    .with_edge(LHS, lhs)
                    .with_edge(RHS, rhs)
                    .with_edge(EQUALITY, equality),
            )
        }
        Term::Elim {
            inductive,
            parameters,
            indices,
            motive,
            branches,
            scrutinee,
        } => {
            let mut node = ObjectNode::new()
                .with_operator(operators[ELIM].clone())
                .with_property(INDUCTIVE_NAME, Literal::Text(inductive.0.clone()));
            for parameter in parameters {
                node = node.with_edge(PARAMETER, encode_term(builder, operators, parameter));
            }
            for index in indices {
                node = node.with_edge(FAMILY_INDEX, encode_term(builder, operators, index));
            }
            node = node.with_edge(MOTIVE, encode_term(builder, operators, motive));
            for branch in branches {
                node = node.with_edge(BRANCH, encode_term(builder, operators, branch));
            }
            node = node.with_edge(SCRUTINEE, encode_term(builder, operators, scrutinee));
            builder.alloc(node)
        }
        Term::Let {
            value_type,
            value,
            body,
        } => {
            let value_type = encode_term(builder, operators, value_type);
            let value = encode_term(builder, operators, value);
            let body = encode_term(builder, operators, body);
            builder.alloc(
                ObjectNode::new()
                    .with_operator(operators[LET].clone())
                    .with_edge(VALUE_TYPE, value_type)
                    .with_edge(VALUE, value)
                    .with_edge(BODY, body),
            )
        }
    }
}

fn encode_binary(
    builder: &mut GraphBuilder,
    operators: &BTreeMap<&'static str, ObjectId>,
    operator: &'static str,
    left_role: &'static str,
    left: &Term,
    right_role: &'static str,
    right: &Term,
) -> ObjectId {
    let left = encode_term(builder, operators, left);
    let right = encode_term(builder, operators, right);
    builder.alloc(
        ObjectNode::new()
            .with_operator(operators[operator].clone())
            .with_edge(left_role, left)
            .with_edge(right_role, right),
    )
}

fn encode_unary(
    builder: &mut GraphBuilder,
    operators: &BTreeMap<&'static str, ObjectId>,
    operator: &'static str,
    role: &'static str,
    value: &Term,
) -> ObjectId {
    let value = encode_term(builder, operators, value);
    builder.alloc(
        ObjectNode::new()
            .with_operator(operators[operator].clone())
            .with_edge(role, value),
    )
}

fn decode_term(
    graph: &ObjectGraph,
    id: &ObjectId,
    visiting: &mut BTreeSet<ObjectId>,
    memo: &mut BTreeMap<ObjectId, Term>,
) -> Result<Term, CoreCodecError> {
    if let Some(term) = memo.get(id) {
        return Ok(term.clone());
    }
    if !visiting.insert(id.clone()) {
        return Err(CoreCodecError::CyclicCoreTerm(id.clone()));
    }
    let node = &graph.nodes[id];
    let operator_id = node
        .operator
        .as_ref()
        .ok_or_else(|| CoreCodecError::MissingOperator(id.clone()))?;
    let operator = &graph.nodes[operator_id];
    let symbol = match operator.properties.get(&Symbol::from(SYMBOL)) {
        Some(Literal::Text(symbol)) => symbol.as_str(),
        _ => return Err(CoreCodecError::MalformedOperator(operator_id.clone())),
    };
    let term = match symbol {
        UNIVERSE => Term::universe(integer_property(node, id, LEVEL)?),
        VAR => Term::var(integer_property(node, id, INDEX)?),
        CONST => Term::constant(name_property(node, id, NAME)?),
        PI => Term::pi(
            decode_edge(graph, node, id, DOMAIN, visiting, memo)?,
            decode_edge(graph, node, id, CODOMAIN, visiting, memo)?,
        ),
        SIGMA => Term::sigma(
            decode_edge(graph, node, id, DOMAIN, visiting, memo)?,
            decode_edge(graph, node, id, CODOMAIN, visiting, memo)?,
        ),
        LAM => Term::lam(
            decode_edge(graph, node, id, DOMAIN, visiting, memo)?,
            decode_edge(graph, node, id, BODY, visiting, memo)?,
        ),
        APP => Term::app(
            decode_edge(graph, node, id, FUNCTION, visiting, memo)?,
            decode_edge(graph, node, id, ARGUMENT, visiting, memo)?,
        ),
        PAIR => Term::pair(
            decode_edge(graph, node, id, FIRST, visiting, memo)?,
            decode_edge(graph, node, id, SECOND, visiting, memo)?,
        ),
        FST => Term::fst(decode_edge(graph, node, id, PAIR_VALUE, visiting, memo)?),
        SND => Term::snd(decode_edge(graph, node, id, PAIR_VALUE, visiting, memo)?),
        ID => Term::id(
            decode_edge(graph, node, id, TYPE_VALUE, visiting, memo)?,
            decode_edge(graph, node, id, LHS, visiting, memo)?,
            decode_edge(graph, node, id, RHS, visiting, memo)?,
        ),
        REFL => Term::refl(decode_edge(graph, node, id, VALUE, visiting, memo)?),
        J => Term::j(
            decode_edge(graph, node, id, TYPE_VALUE, visiting, memo)?,
            decode_edge(graph, node, id, MOTIVE, visiting, memo)?,
            decode_edge(graph, node, id, REFL_CASE, visiting, memo)?,
            decode_edge(graph, node, id, LHS, visiting, memo)?,
            decode_edge(graph, node, id, RHS, visiting, memo)?,
            decode_edge(graph, node, id, EQUALITY, visiting, memo)?,
        ),
        ELIM => Term::elim(
            name_property(node, id, INDUCTIVE_NAME)?,
            decode_edges(graph, node, PARAMETER, visiting, memo)?,
            decode_edges(graph, node, FAMILY_INDEX, visiting, memo)?,
            decode_edge(graph, node, id, MOTIVE, visiting, memo)?,
            decode_edges(graph, node, BRANCH, visiting, memo)?,
            decode_edge(graph, node, id, SCRUTINEE, visiting, memo)?,
        ),
        LET => Term::Let {
            value_type: Box::new(decode_edge(graph, node, id, VALUE_TYPE, visiting, memo)?),
            value: Box::new(decode_edge(graph, node, id, VALUE, visiting, memo)?),
            body: Box::new(decode_edge(graph, node, id, BODY, visiting, memo)?),
        },
        _ => {
            return Err(CoreCodecError::UnknownOperator {
                object: id.clone(),
                symbol: symbol.to_owned(),
            });
        }
    };
    visiting.remove(id);
    memo.insert(id.clone(), term.clone());
    Ok(term)
}

fn decode_edge(
    graph: &ObjectGraph,
    node: &ObjectNode,
    object: &ObjectId,
    role: &'static str,
    visiting: &mut BTreeSet<ObjectId>,
    memo: &mut BTreeMap<ObjectId, Term>,
) -> Result<Term, CoreCodecError> {
    let role_symbol = Symbol::from(role);
    let targets = node
        .edges
        .get(&role_symbol)
        .filter(|targets| targets.len() == 1)
        .ok_or_else(|| CoreCodecError::EdgeArity {
            object: object.clone(),
            role: role_symbol.clone(),
        })?;
    decode_term(graph, &targets[0], visiting, memo)
}

fn decode_edges(
    graph: &ObjectGraph,
    node: &ObjectNode,
    role: &'static str,
    visiting: &mut BTreeSet<ObjectId>,
    memo: &mut BTreeMap<ObjectId, Term>,
) -> Result<Vec<Term>, CoreCodecError> {
    node.edges
        .get(&Symbol::from(role))
        .into_iter()
        .flatten()
        .map(|target| decode_term(graph, target, visiting, memo))
        .collect()
}

fn integer_property(
    node: &ObjectNode,
    object: &ObjectId,
    property: &'static str,
) -> Result<u32, CoreCodecError> {
    let property_symbol = Symbol::from(property);
    match node.properties.get(&property_symbol) {
        Some(Literal::Integer(value)) => value.parse().map_err(|_| CoreCodecError::Property {
            object: object.clone(),
            property: property_symbol,
        }),
        _ => Err(CoreCodecError::Property {
            object: object.clone(),
            property: property_symbol,
        }),
    }
}

fn name_property(
    node: &ObjectNode,
    object: &ObjectId,
    property: &'static str,
) -> Result<Name, CoreCodecError> {
    let property_symbol = Symbol::from(property);
    match node.properties.get(&property_symbol) {
        Some(Literal::Text(value)) => Ok(Name::new(value.clone())),
        _ => Err(CoreCodecError::Property {
            object: object.clone(),
            property: property_symbol,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_core_constructor_roundtrips() {
        let term = Term::elim(
            "Nat",
            vec![],
            vec![],
            Term::lam(Term::constant("Nat"), Term::universe(0)),
            vec![
                Term::constant("zero"),
                Term::lam(
                    Term::constant("Nat"),
                    Term::lam(Term::constant("Nat"), Term::var(0)),
                ),
            ],
            Term::j(
                Term::sigma(Term::universe(0), Term::var(0)),
                Term::lam(
                    Term::sigma(Term::universe(0), Term::var(0)),
                    Term::lam(
                        Term::sigma(Term::universe(0), Term::var(0)),
                        Term::lam(
                            Term::id(
                                Term::sigma(Term::universe(0), Term::var(0)),
                                Term::var(1),
                                Term::var(0),
                            ),
                            Term::universe(0),
                        ),
                    ),
                ),
                Term::lam(
                    Term::sigma(Term::universe(0), Term::var(0)),
                    Term::constant("Nat"),
                ),
                Term::pair(Term::universe(0), Term::universe(0)),
                Term::pair(Term::universe(0), Term::universe(0)),
                Term::refl(Term::pair(Term::universe(0), Term::universe(0))),
            ),
        );
        let graph = CoreGraphCodec::encode(&term).unwrap();
        assert_eq!(CoreGraphCodec::decode(&graph).unwrap(), term);
    }

    #[test]
    fn unknown_graph_operator_remains_representable_but_not_elaborated() {
        let mut builder = GraphBuilder::new();
        let operator = builder.insert(
            "op",
            ObjectNode::new().with_property(SYMBOL, Literal::Text("future/op".into())),
        );
        let term = builder.alloc(ObjectNode::new().with_operator(operator));
        builder.root(ROOT, term);
        let graph = builder.finish().unwrap();
        assert!(matches!(
            CoreGraphCodec::decode(&graph),
            Err(CoreCodecError::UnknownOperator { .. })
        ));
    }
}
