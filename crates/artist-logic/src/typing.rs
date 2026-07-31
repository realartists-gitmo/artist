//! Advisory type inference.
//!
//! Deliberately advisory, never a gate. Rejecting an ill-typed expression at
//! construction would be exactly the ceiling this language exists to remove —
//! representation stays total, and a type error is a *finding* the caller may
//! act on rather than a refusal to store.
//!
//! Types are ordinary objects built from the reserved type operators, so the
//! result of inference is itself an expression: quantifiable over, storable,
//! and printable through the same canonical forms as anything else.

use crate::object::{CoreNode, LiteralValue, ObjectGraph, ObjectId, wk};

/// What inference concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typing {
    /// A type expression.
    Known(ObjectId),
    /// Well-formed, but this build cannot infer a type — an unregistered
    /// operator, an external reference, an opaque node. Not an error.
    Unknown,
    /// A genuine inconsistency, with a human-readable reason.
    Mismatch(String),
}

impl Typing {
    pub fn is_mismatch(&self) -> bool {
        matches!(self, Typing::Mismatch(_))
    }
}

/// Infer the type of `node`.
pub fn type_of(g: &mut ObjectGraph, node: ObjectId) -> Typing {
    infer(g, node, 0)
}

fn infer(g: &mut ObjectGraph, node: ObjectId, depth: u32) -> Typing {
    if depth > 128 {
        // A cyclic expression has no finite type derivation; that is a limit of
        // inference, not of representation.
        return Typing::Unknown;
    }
    match g.get(node).cloned() {
        None => Typing::Unknown,
        Some(CoreNode::Literal(v)) => Typing::Known(match v {
            LiteralValue::Int(_) => wk::INT_TYPE,
            LiteralValue::Decimal { .. } => wk::INT_TYPE,
            LiteralValue::Bool(_) => wk::PROP_TYPE,
            LiteralValue::Text(_) | LiteralValue::Bytes(_) => {
                g.atom("Text")
            }
        }),
        Some(CoreNode::External(_)) | Some(CoreNode::Opaque { .. }) => Typing::Unknown,
        Some(CoreNode::Atom { .. }) => {
            if node == wk::TOP || node == wk::BOT {
                Typing::Known(wk::PROP_TYPE)
            } else {
                Typing::Unknown
            }
        }
        Some(CoreNode::Apply { operator, operands }) => {
            apply_rule(g, operator, &operands, depth)
        }
        Some(CoreNode::Bind { binder, vars, bodies }) => match binder {
            wk::FORALL | wk::EXISTS => {
                for b in &bodies {
                    if let Typing::Known(t) = infer(g, *b, depth + 1)
                        && t != wk::PROP_TYPE
                    {
                        return Typing::Mismatch(
                            "quantifier body must be a proposition".into(),
                        );
                    }
                }
                Typing::Known(wk::PROP_TYPE)
            }
            wk::COUNT | wk::SUM => Typing::Known(wk::INT_TYPE),
            wk::LAMBDA => {
                // λ over `n` parameters denotes a relation of those domains.
                let doms: Vec<ObjectId> =
                    vars.iter().filter_map(|b| b.domain).collect();
                if doms.len() != vars.len() {
                    return Typing::Unknown;
                }
                Typing::Known(g.apply(wk::RELATION_TYPE, doms))
            }
            wk::LETREC => bodies
                .last()
                .map(|b| infer(g, *b, depth + 1))
                .unwrap_or(Typing::Unknown),
            _ => Typing::Unknown,
        },
    }
}

fn apply_rule(
    g: &mut ObjectGraph,
    operator: ObjectId,
    operands: &[ObjectId],
    depth: u32,
) -> Typing {
    match operator {
        // Connectives take and return propositions.
        wk::NOT | wk::AND | wk::OR | wk::IMPLIES => {
            for o in operands {
                if let Typing::Known(t) = infer(g, *o, depth + 1)
                    && t != wk::PROP_TYPE
                {
                    return Typing::Mismatch(format!(
                        "connective operand is not a proposition: {o}"
                    ));
                }
            }
            Typing::Known(wk::PROP_TYPE)
        }
        wk::EQ => Typing::Known(wk::PROP_TYPE),
        wk::LEQ => {
            for o in operands {
                if let Typing::Known(t) = infer(g, *o, depth + 1)
                    && t == wk::PROP_TYPE
                {
                    return Typing::Mismatch("<= compares values, not propositions".into());
                }
            }
            Typing::Known(wk::PROP_TYPE)
        }
        wk::ADD | wk::MUL | wk::NEG | wk::DIV | wk::MOD => {
            for o in operands {
                if let Typing::Known(t) = infer(g, *o, depth + 1)
                    && t == wk::PROP_TYPE
                {
                    return Typing::Mismatch("arithmetic on a proposition".into());
                }
            }
            Typing::Known(wk::INT_TYPE)
        }
        wk::LEN => Typing::Known(wk::INT_TYPE),
        wk::CONCAT | wk::SUBSTR => Typing::Known(g.atom("Text")),
        wk::CONTAINS | wk::STARTS_WITH | wk::ENDS_WITH => Typing::Known(wk::PROP_TYPE),
        // Quoting lifts a proposition to an expression; `holds` lowers it back.
        wk::QUOTE => Typing::Known(wk::EXPR_TYPE),
        wk::HOLDS => {
            match operands.first().map(|o| infer(g, *o, depth + 1)) {
                Some(Typing::Known(t)) if t != wk::EXPR_TYPE => {
                    Typing::Mismatch("holds expects a quoted expression".into())
                }
                _ => Typing::Known(wk::PROP_TYPE),
            }
        }
        wk::AT | wk::IN_WORLD | wk::NECESSARILY | wk::POSSIBLY | wk::ALWAYS
        | wk::EVENTUALLY | wk::SINCE | wk::COUNTERFACTUAL | wk::BEFORE | wk::DURING => {
            Typing::Known(wk::PROP_TYPE)
        }
        wk::RELATION_TYPE | wk::FUNCTION_TYPE | wk::PRODUCT_TYPE | wk::SUM_TYPE
        | wk::REFINEMENT_TYPE => {
            // A type expression inhabits the next universe up.
            let zero = g.int(0);
            Typing::Known(g.apply(wk::UNIVERSE, vec![zero]))
        }
        wk::UNIVERSE => {
            // Universe levels are values, so `Universe(n) : Universe(n+1)`.
            let level = operands
                .first()
                .and_then(|o| match g.get(*o) {
                    Some(CoreNode::Literal(LiteralValue::Int(n))) => {
                        num_traits::ToPrimitive::to_i64(n)
                    }
                    _ => None,
                })
                .unwrap_or(0);
            let next = g.int(level + 1);
            Typing::Known(g.apply(wk::UNIVERSE, vec![next]))
        }
        // Applying a lambda yields a proposition; anything else is a predicate
        // application, which is also a proposition.
        _ => Typing::Known(wk::PROP_TYPE),
    }
}
