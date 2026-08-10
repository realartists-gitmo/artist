use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// Qualified name of a theory declaration.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Name(pub String);

impl Name {
    /// Creates a name.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the name text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Name {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Fixed dependent core term. De Bruijn index 0 refers to the nearest binder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "term", rename_all = "snake_case")]
pub enum Term {
    /// Predicative universe `Type level`.
    Universe { level: u32 },
    /// Bound variable.
    Var { index: u32 },
    /// Theory declaration.
    Const { name: Name },
    /// Dependent function type. `codomain` is under one binder.
    Pi {
        domain: Box<Term>,
        codomain: Box<Term>,
    },
    /// Dependent pair type. `codomain` is under one binder.
    Sigma {
        domain: Box<Term>,
        codomain: Box<Term>,
    },
    /// Typed lambda. `body` is under one binder.
    Lam { domain: Box<Term>, body: Box<Term> },
    /// Function application.
    App {
        function: Box<Term>,
        argument: Box<Term>,
    },
    /// Dependent pair introduction. Its type is supplied bidirectionally.
    Pair { first: Box<Term>, second: Box<Term> },
    /// First projection.
    Fst { pair: Box<Term> },
    /// Dependent second projection.
    Snd { pair: Box<Term> },
    /// Propositional identity `Id ty lhs rhs`.
    Id {
        ty: Box<Term>,
        lhs: Box<Term>,
        rhs: Box<Term>,
    },
    /// Reflexivity witness.
    Refl { value: Box<Term> },
    /// Identity elimination.
    ///
    /// `motive` has type `Π x:ty. Π y:ty. Id ty x y -> Type u`;
    /// `refl_case` has type `Π x:ty. motive x x (refl x)`.
    J {
        ty: Box<Term>,
        motive: Box<Term>,
        refl_case: Box<Term>,
        lhs: Box<Term>,
        rhs: Box<Term>,
        equality: Box<Term>,
    },
    /// Dependent eliminator for a checked strictly-positive inductive family.
    Elim {
        inductive: Name,
        parameters: Vec<Term>,
        indices: Vec<Term>,
        motive: Box<Term>,
        branches: Vec<Term>,
        scrutinee: Box<Term>,
    },
    /// Local definition. `body` is under one binder.
    Let {
        value_type: Box<Term>,
        value: Box<Term>,
        body: Box<Term>,
    },
}

impl Term {
    /// Constructs a universe.
    #[must_use]
    pub const fn universe(level: u32) -> Self {
        Self::Universe { level }
    }

    /// Constructs a variable.
    #[must_use]
    pub const fn var(index: u32) -> Self {
        Self::Var { index }
    }

    /// Constructs a constant.
    #[must_use]
    pub fn constant(name: impl Into<Name>) -> Self {
        Self::Const { name: name.into() }
    }

    /// Constructs a dependent function type.
    #[must_use]
    pub fn pi(domain: Self, codomain: Self) -> Self {
        Self::Pi {
            domain: Box::new(domain),
            codomain: Box::new(codomain),
        }
    }

    /// Constructs a dependent pair type.
    #[must_use]
    pub fn sigma(domain: Self, codomain: Self) -> Self {
        Self::Sigma {
            domain: Box::new(domain),
            codomain: Box::new(codomain),
        }
    }

    /// Constructs a typed lambda.
    #[must_use]
    pub fn lam(domain: Self, body: Self) -> Self {
        Self::Lam {
            domain: Box::new(domain),
            body: Box::new(body),
        }
    }

    /// Constructs an application.
    #[must_use]
    pub fn app(function: Self, argument: Self) -> Self {
        Self::App {
            function: Box::new(function),
            argument: Box::new(argument),
        }
    }

    /// Constructs a dependent pair.
    #[must_use]
    pub fn pair(first: Self, second: Self) -> Self {
        Self::Pair {
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    /// Constructs a first projection.
    #[must_use]
    pub fn fst(pair: Self) -> Self {
        Self::Fst {
            pair: Box::new(pair),
        }
    }

    /// Constructs a second projection.
    #[must_use]
    pub fn snd(pair: Self) -> Self {
        Self::Snd {
            pair: Box::new(pair),
        }
    }

    /// Constructs an identity type.
    #[must_use]
    pub fn id(ty: Self, lhs: Self, rhs: Self) -> Self {
        Self::Id {
            ty: Box::new(ty),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    /// Constructs reflexivity.
    #[must_use]
    pub fn refl(term: Self) -> Self {
        Self::Refl {
            value: Box::new(term),
        }
    }

    /// Constructs identity elimination.
    #[must_use]
    pub fn j(
        ty: Self,
        motive: Self,
        refl_case: Self,
        lhs: Self,
        rhs: Self,
        equality: Self,
    ) -> Self {
        Self::J {
            ty: Box::new(ty),
            motive: Box::new(motive),
            refl_case: Box::new(refl_case),
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            equality: Box::new(equality),
        }
    }

    /// Constructs dependent induction/elimination for a checked inductive family.
    #[must_use]
    pub fn elim(
        inductive: impl Into<Name>,
        parameters: Vec<Self>,
        indices: Vec<Self>,
        motive: Self,
        branches: Vec<Self>,
        scrutinee: Self,
    ) -> Self {
        Self::Elim {
            inductive: inductive.into(),
            parameters,
            indices,
            motive: Box::new(motive),
            branches,
            scrutinee: Box::new(scrutinee),
        }
    }

    /// Applies a term to several arguments from left to right.
    #[must_use]
    pub fn apply_many(function: Self, arguments: impl IntoIterator<Item = Self>) -> Self {
        arguments.into_iter().fold(function, Self::app)
    }

    /// Constructs nested dependent functions. Domains are ordered outermost first.
    #[must_use]
    pub fn pi_many(domains: impl IntoIterator<Item = Self>, result: Self) -> Self {
        let domains: Vec<_> = domains.into_iter().collect();
        domains
            .into_iter()
            .rev()
            .fold(result, |codomain, domain| Self::pi(domain, codomain))
    }

    /// Returns all referenced theory constants.
    #[must_use]
    pub fn constants(&self) -> BTreeSet<Name> {
        let mut output = BTreeSet::new();
        self.collect_constants(&mut output);
        output
    }

    fn collect_constants(&self, output: &mut BTreeSet<Name>) {
        match self {
            Self::Universe { .. } | Self::Var { .. } => {}
            Self::Const { name } => {
                output.insert(name.clone());
            }
            Self::Pi { domain, codomain }
            | Self::Sigma { domain, codomain }
            | Self::Lam {
                domain,
                body: codomain,
            } => {
                domain.collect_constants(output);
                codomain.collect_constants(output);
            }
            Self::App { function, argument } => {
                function.collect_constants(output);
                argument.collect_constants(output);
            }
            Self::Pair { first, second } => {
                first.collect_constants(output);
                second.collect_constants(output);
            }
            Self::Fst { pair } | Self::Snd { pair } | Self::Refl { value: pair } => {
                pair.collect_constants(output);
            }
            Self::Id { ty, lhs, rhs } => {
                ty.collect_constants(output);
                lhs.collect_constants(output);
                rhs.collect_constants(output);
            }
            Self::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => {
                ty.collect_constants(output);
                motive.collect_constants(output);
                refl_case.collect_constants(output);
                lhs.collect_constants(output);
                rhs.collect_constants(output);
                equality.collect_constants(output);
            }
            Self::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => {
                output.insert(inductive.clone());
                for term in parameters.iter().chain(indices).chain(branches) {
                    term.collect_constants(output);
                }
                motive.collect_constants(output);
                scrutinee.collect_constants(output);
            }
            Self::Let {
                value_type,
                value,
                body,
            } => {
                value_type.collect_constants(output);
                value.collect_constants(output);
                body.collect_constants(output);
            }
        }
    }

    /// Replaces constants using a theory interpretation.
    #[must_use]
    pub fn replace_constants(&self, replacements: &BTreeMap<Name, Term>) -> Self {
        match self {
            Self::Universe { level } => Self::universe(*level),
            Self::Var { index } => Self::var(*index),
            Self::Const { name } => replacements
                .get(name)
                .cloned()
                .unwrap_or_else(|| Self::constant(name.clone())),
            Self::Pi { domain, codomain } => Self::pi(
                domain.replace_constants(replacements),
                codomain.replace_constants(replacements),
            ),
            Self::Sigma { domain, codomain } => Self::sigma(
                domain.replace_constants(replacements),
                codomain.replace_constants(replacements),
            ),
            Self::Lam { domain, body } => Self::lam(
                domain.replace_constants(replacements),
                body.replace_constants(replacements),
            ),
            Self::App { function, argument } => Self::app(
                function.replace_constants(replacements),
                argument.replace_constants(replacements),
            ),
            Self::Pair { first, second } => Self::pair(
                first.replace_constants(replacements),
                second.replace_constants(replacements),
            ),
            Self::Fst { pair } => Self::fst(pair.replace_constants(replacements)),
            Self::Snd { pair } => Self::snd(pair.replace_constants(replacements)),
            Self::Id { ty, lhs, rhs } => Self::id(
                ty.replace_constants(replacements),
                lhs.replace_constants(replacements),
                rhs.replace_constants(replacements),
            ),
            Self::Refl { value } => Self::refl(value.replace_constants(replacements)),
            Self::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => Self::j(
                ty.replace_constants(replacements),
                motive.replace_constants(replacements),
                refl_case.replace_constants(replacements),
                lhs.replace_constants(replacements),
                rhs.replace_constants(replacements),
                equality.replace_constants(replacements),
            ),
            Self::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => {
                let renamed = match replacements.get(inductive) {
                    Some(Self::Const { name }) => name.clone(),
                    _ => inductive.clone(),
                };
                Self::elim(
                    renamed,
                    parameters
                        .iter()
                        .map(|term| term.replace_constants(replacements))
                        .collect(),
                    indices
                        .iter()
                        .map(|term| term.replace_constants(replacements))
                        .collect(),
                    motive.replace_constants(replacements),
                    branches
                        .iter()
                        .map(|term| term.replace_constants(replacements))
                        .collect(),
                    scrutinee.replace_constants(replacements),
                )
            }
            Self::Let {
                value_type,
                value,
                body,
            } => Self::Let {
                value_type: Box::new(value_type.replace_constants(replacements)),
                value: Box::new(value.replace_constants(replacements)),
                body: Box::new(body.replace_constants(replacements)),
            },
        }
    }

    /// Returns the head constant and application arguments, if the term is headed
    /// by a constant.
    #[must_use]
    pub fn constant_spine(&self) -> Option<(&Name, Vec<&Term>)> {
        let mut arguments = Vec::new();
        let mut cursor = self;
        while let Self::App { function, argument } = cursor {
            arguments.push(argument.as_ref());
            cursor = function.as_ref();
        }
        arguments.reverse();
        match cursor {
            Self::Const { name } => Some((name, arguments)),
            _ => None,
        }
    }

    pub(crate) fn write_canonical(&self, output: &mut Vec<u8>) {
        match self {
            Self::Universe { level } => {
                output.push(0);
                output.extend_from_slice(&level.to_be_bytes());
            }
            Self::Var { index } => {
                output.push(1);
                output.extend_from_slice(&index.to_be_bytes());
            }
            Self::Const { name } => {
                output.push(2);
                write_string(output, name.as_str());
            }
            Self::Pi { domain, codomain } => {
                output.push(3);
                domain.write_canonical(output);
                codomain.write_canonical(output);
            }
            Self::Lam { domain, body } => {
                output.push(4);
                domain.write_canonical(output);
                body.write_canonical(output);
            }
            Self::App { function, argument } => {
                output.push(5);
                function.write_canonical(output);
                argument.write_canonical(output);
            }
            Self::Sigma { domain, codomain } => {
                output.push(7);
                domain.write_canonical(output);
                codomain.write_canonical(output);
            }
            Self::Pair { first, second } => {
                output.push(8);
                first.write_canonical(output);
                second.write_canonical(output);
            }
            Self::Fst { pair } => {
                output.push(9);
                pair.write_canonical(output);
            }
            Self::Snd { pair } => {
                output.push(10);
                pair.write_canonical(output);
            }
            Self::Id { ty, lhs, rhs } => {
                output.push(11);
                ty.write_canonical(output);
                lhs.write_canonical(output);
                rhs.write_canonical(output);
            }
            Self::Refl { value } => {
                output.push(12);
                value.write_canonical(output);
            }
            Self::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => {
                output.push(13);
                ty.write_canonical(output);
                motive.write_canonical(output);
                refl_case.write_canonical(output);
                lhs.write_canonical(output);
                rhs.write_canonical(output);
                equality.write_canonical(output);
            }
            Self::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => {
                output.push(14);
                write_string(output, inductive.as_str());
                write_terms(output, parameters);
                write_terms(output, indices);
                motive.write_canonical(output);
                write_terms(output, branches);
                scrutinee.write_canonical(output);
            }
            Self::Let {
                value_type,
                value,
                body,
            } => {
                output.push(6);
                value_type.write_canonical(output);
                value.write_canonical(output);
                body.write_canonical(output);
            }
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Universe { level } => write!(formatter, "Type{level}"),
            Self::Var { index } => write!(formatter, "#{index}"),
            Self::Const { name } => write!(formatter, "{name}"),
            Self::Pi { domain, codomain } => write!(formatter, "(Π {domain}. {codomain})"),
            Self::Sigma { domain, codomain } => write!(formatter, "(Σ {domain}. {codomain})"),
            Self::Lam { domain, body } => write!(formatter, "(λ {domain}. {body})"),
            Self::App { function, argument } => write!(formatter, "({function} {argument})"),
            Self::Pair { first, second } => write!(formatter, "⟨{first}, {second}⟩"),
            Self::Fst { pair } => write!(formatter, "(fst {pair})"),
            Self::Snd { pair } => write!(formatter, "(snd {pair})"),
            Self::Id { ty, lhs, rhs } => write!(formatter, "(Id {ty} {lhs} {rhs})"),
            Self::Refl { value } => write!(formatter, "(refl {value})"),
            Self::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => write!(
                formatter,
                "(J {ty} {motive} {refl_case} {lhs} {rhs} {equality})"
            ),
            Self::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => write!(
                formatter,
                "(elim {inductive} params={parameters:?} indices={indices:?} {motive} branches={branches:?} {scrutinee})"
            ),
            Self::Let {
                value_type,
                value,
                body,
            } => write!(formatter, "(let : {value_type} := {value}; {body})"),
        }
    }
}

/// Local context ordered oldest to newest. `Var(0)` addresses the final entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Context(pub Vec<Term>);

impl Context {
    /// Empty context.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Number of local assumptions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the context is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Extends the context with a newest binding.
    #[must_use]
    pub fn extend(&self, ty: Term) -> Self {
        let mut entries = self.0.clone();
        entries.push(ty);
        Self(entries)
    }

    /// Returns the prefix preceding an entry.
    #[must_use]
    pub fn prefix(&self, length: usize) -> Self {
        Self(self.0[..length].to_vec())
    }

    /// Looks up a de Bruijn variable and shifts its stored type into the current
    /// context.
    #[must_use]
    pub fn lookup(&self, index: u32) -> Option<Term> {
        let index = usize::try_from(index).ok()?;
        let position = self.0.len().checked_sub(index + 1)?;
        Some(shift(&self.0[position], i64::try_from(index + 1).ok()?, 0))
    }
}

/// Shifts free de Bruijn indices by `amount` at or above `cutoff`.
#[must_use]
pub fn shift(term: &Term, amount: i64, cutoff: u32) -> Term {
    fn walk(term: &Term, amount: i64, cutoff: u32, depth: u32) -> Term {
        match term {
            Term::Universe { level } => Term::universe(*level),
            Term::Var { index } => {
                if *index >= cutoff + depth {
                    let shifted = i64::from(*index) + amount;
                    assert!(shifted >= 0, "de Bruijn shift underflow");
                    Term::var(u32::try_from(shifted).expect("de Bruijn shift overflow"))
                } else {
                    Term::var(*index)
                }
            }
            Term::Const { name } => Term::constant(name.clone()),
            Term::Pi { domain, codomain } => Term::pi(
                walk(domain, amount, cutoff, depth),
                walk(codomain, amount, cutoff, depth + 1),
            ),
            Term::Sigma { domain, codomain } => Term::sigma(
                walk(domain, amount, cutoff, depth),
                walk(codomain, amount, cutoff, depth + 1),
            ),
            Term::Lam { domain, body } => Term::lam(
                walk(domain, amount, cutoff, depth),
                walk(body, amount, cutoff, depth + 1),
            ),
            Term::App { function, argument } => Term::app(
                walk(function, amount, cutoff, depth),
                walk(argument, amount, cutoff, depth),
            ),
            Term::Pair { first, second } => Term::pair(
                walk(first, amount, cutoff, depth),
                walk(second, amount, cutoff, depth),
            ),
            Term::Fst { pair } => Term::fst(walk(pair, amount, cutoff, depth)),
            Term::Snd { pair } => Term::snd(walk(pair, amount, cutoff, depth)),
            Term::Id { ty, lhs, rhs } => Term::id(
                walk(ty, amount, cutoff, depth),
                walk(lhs, amount, cutoff, depth),
                walk(rhs, amount, cutoff, depth),
            ),
            Term::Refl { value } => Term::refl(walk(value, amount, cutoff, depth)),
            Term::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => Term::j(
                walk(ty, amount, cutoff, depth),
                walk(motive, amount, cutoff, depth),
                walk(refl_case, amount, cutoff, depth),
                walk(lhs, amount, cutoff, depth),
                walk(rhs, amount, cutoff, depth),
                walk(equality, amount, cutoff, depth),
            ),
            Term::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => Term::elim(
                inductive.clone(),
                parameters
                    .iter()
                    .map(|term| walk(term, amount, cutoff, depth))
                    .collect(),
                indices
                    .iter()
                    .map(|term| walk(term, amount, cutoff, depth))
                    .collect(),
                walk(motive, amount, cutoff, depth),
                branches
                    .iter()
                    .map(|term| walk(term, amount, cutoff, depth))
                    .collect(),
                walk(scrutinee, amount, cutoff, depth),
            ),
            Term::Let {
                value_type,
                value,
                body,
            } => Term::Let {
                value_type: Box::new(walk(value_type, amount, cutoff, depth)),
                value: Box::new(walk(value, amount, cutoff, depth)),
                body: Box::new(walk(body, amount, cutoff, depth + 1)),
            },
        }
    }
    walk(term, amount, cutoff, 0)
}

fn substitute(term: &Term, index: u32, replacement: &Term) -> Term {
    fn walk(term: &Term, index: u32, replacement: &Term, depth: u32) -> Term {
        match term {
            Term::Universe { level } => Term::universe(*level),
            Term::Var { index: variable } if *variable == index + depth => {
                shift(replacement, i64::from(depth), 0)
            }
            Term::Var { index: variable } => Term::var(*variable),
            Term::Const { name } => Term::constant(name.clone()),
            Term::Pi { domain, codomain } => Term::pi(
                walk(domain, index, replacement, depth),
                walk(codomain, index, replacement, depth + 1),
            ),
            Term::Sigma { domain, codomain } => Term::sigma(
                walk(domain, index, replacement, depth),
                walk(codomain, index, replacement, depth + 1),
            ),
            Term::Lam { domain, body } => Term::lam(
                walk(domain, index, replacement, depth),
                walk(body, index, replacement, depth + 1),
            ),
            Term::App { function, argument } => Term::app(
                walk(function, index, replacement, depth),
                walk(argument, index, replacement, depth),
            ),
            Term::Pair { first, second } => Term::pair(
                walk(first, index, replacement, depth),
                walk(second, index, replacement, depth),
            ),
            Term::Fst { pair } => Term::fst(walk(pair, index, replacement, depth)),
            Term::Snd { pair } => Term::snd(walk(pair, index, replacement, depth)),
            Term::Id { ty, lhs, rhs } => Term::id(
                walk(ty, index, replacement, depth),
                walk(lhs, index, replacement, depth),
                walk(rhs, index, replacement, depth),
            ),
            Term::Refl { value } => Term::refl(walk(value, index, replacement, depth)),
            Term::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => Term::j(
                walk(ty, index, replacement, depth),
                walk(motive, index, replacement, depth),
                walk(refl_case, index, replacement, depth),
                walk(lhs, index, replacement, depth),
                walk(rhs, index, replacement, depth),
                walk(equality, index, replacement, depth),
            ),
            Term::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => Term::elim(
                inductive.clone(),
                parameters
                    .iter()
                    .map(|term| walk(term, index, replacement, depth))
                    .collect(),
                indices
                    .iter()
                    .map(|term| walk(term, index, replacement, depth))
                    .collect(),
                walk(motive, index, replacement, depth),
                branches
                    .iter()
                    .map(|term| walk(term, index, replacement, depth))
                    .collect(),
                walk(scrutinee, index, replacement, depth),
            ),
            Term::Let {
                value_type,
                value,
                body,
            } => Term::Let {
                value_type: Box::new(walk(value_type, index, replacement, depth)),
                value: Box::new(walk(value, index, replacement, depth)),
                body: Box::new(walk(body, index, replacement, depth + 1)),
            },
        }
    }
    walk(term, index, replacement, 0)
}

/// Substitutes a binder at `index` and removes it.
#[must_use]
pub fn subst_at(replacement: &Term, index: u32, body: &Term) -> Term {
    let lifted = shift(replacement, 1, 0);
    let substituted = substitute(body, index, &lifted);
    shift(&substituted, -1, index.saturating_add(1))
}

/// Substitutes the nearest binder and removes it.
#[must_use]
pub fn subst_top(replacement: &Term, body: &Term) -> Term {
    subst_at(replacement, 0, body)
}

/// Instantiates a term under a telescope. Witnesses are ordered oldest to newest.
#[must_use]
pub fn instantiate(term: &Term, witnesses: &[Term]) -> Term {
    instantiate_under(term, witnesses, 0)
}

/// Instantiates outer telescope binders while preserving `inner_binders` nearest
/// binders. Witnesses are ordered oldest to newest.
#[must_use]
pub fn instantiate_under(term: &Term, witnesses: &[Term], inner_binders: u32) -> Term {
    witnesses
        .iter()
        .rev()
        .fold(term.clone(), |current, witness| {
            subst_at(witness, inner_binders, &current)
        })
}

pub(crate) fn write_string(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

pub(crate) fn write_terms(output: &mut Vec<u8>, terms: &[Term]) {
    output.extend_from_slice(&(terms.len() as u64).to_be_bytes());
    for term in terms {
        term.write_canonical(output);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_substitution_preserves_outer_variables() {
        // (λ A. λ x:A. x) Type0  ==> λ x:Type0. x
        let body = Term::lam(Term::var(0), Term::var(0));
        let reduced = subst_top(&Term::universe(0), &body);
        assert_eq!(reduced, Term::lam(Term::universe(0), Term::var(0)));
    }

    #[test]
    fn context_lookup_shifts_older_types() {
        let context = Context(vec![Term::universe(0), Term::var(0)]);
        assert_eq!(context.lookup(0), Some(Term::var(1)));
        assert_eq!(context.lookup(1), Some(Term::universe(0)));
    }

    #[test]
    fn telescope_instantiation_runs_newest_first() {
        let goal = Term::app(Term::var(1), Term::var(0));
        let instantiated = instantiate(&goal, &[Term::constant("f"), Term::constant("x")]);
        assert_eq!(
            instantiated,
            Term::app(Term::constant("f"), Term::constant("x"))
        );
    }
}
