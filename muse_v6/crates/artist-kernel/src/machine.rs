use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::theory::{ConstructorField, InductiveConstructor, InductiveDeclaration};
use crate::{
    Certificate, Context, Name, Term, Theory, TheoryId, instantiate, instantiate_under, shift,
    subst_top,
};

/// A kernel operation. All requests first validate their local context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum KernelRequest {
    /// Infer a term's type.
    Infer { context: Context, term: Term },
    /// Check a term against an expected type.
    Check {
        context: Context,
        term: Term,
        expected: Term,
    },
    /// Check that a term is a type and return its universe level.
    EnsureUniverse { context: Context, term: Term },
    /// Verify an exact theory-relative certificate.
    Certificate { certificate: Certificate },
    /// Check definitional equality under the fixed beta/zeta/delta/iota/projection/J computation rules.
    DefEq {
        context: Context,
        left: Term,
        right: Term,
    },
}

impl KernelRequest {
    fn context(&self) -> &Context {
        match self {
            Self::Infer { context, .. }
            | Self::Check { context, .. }
            | Self::EnsureUniverse { context, .. }
            | Self::DefEq { context, .. } => context,
            Self::Certificate { certificate } => &certificate.context,
        }
    }
}

/// Successful kernel output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum KernelResult {
    /// Inferred type.
    Inferred(Term),
    /// Checked inhabitation.
    Checked,
    /// Universe level containing a type.
    Universe(u32),
    /// Accepted certificate judgment.
    Certified,
    /// Accepted definitional equality.
    DefinitionallyEqual,
}

/// Kernel rejection. None of these variants means "proof search failed"; the
/// kernel checks only the supplied object.
#[derive(Clone, Debug, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum KernelError {
    /// The certificate names another theory version.
    #[error("certificate targets theory {claimed}, checker loaded {loaded}")]
    TheoryMismatch { claimed: TheoryId, loaded: TheoryId },
    /// The serialized or directly constructed theory did not reconstruct through the checked builder.
    #[error("invalid theory snapshot: {0}")]
    InvalidTheory(String),
    /// A de Bruijn variable has no binding.
    #[error("unbound variable #{index} in context of length {context_len}")]
    UnboundVariable { index: u32, context_len: usize },
    /// A constant is not declared in this theory.
    #[error("unknown constant {0}")]
    UnknownConstant(Name),
    /// A universe level overflowed.
    #[error("universe level overflow at Type{0}")]
    UniverseOverflow(u32),
    /// An expression expected to be a type did not reduce to a universe.
    #[error("expected a universe, found {found}")]
    ExpectedUniverse { found: Box<Term> },
    /// A function application targeted a non-function.
    #[error("expected a dependent function type, found {found}")]
    ExpectedFunction { found: Box<Term> },
    /// A projection or pair check expected a dependent pair type.
    #[error("expected a dependent pair type, found {found}")]
    ExpectedSigma { found: Box<Term> },
    /// A pair requires an expected Sigma type in the bidirectional checker.
    #[error("cannot infer a type for a dependent pair without an expected Sigma type")]
    CannotInferPair,
    /// The supplied identity motive is not a three-argument dependent family.
    #[error("invalid identity eliminator motive: {0}")]
    InvalidIdentityMotive(String),
    /// An inductive eliminator names an unknown family.
    #[error("unknown inductive family {0}")]
    UnknownInductive(Name),
    /// An inductive eliminator has malformed parameters, indices, motive, or branches.
    #[error("invalid eliminator for {family}: {message}")]
    InvalidInductiveEliminator { family: Name, message: String },
    /// Two terms are not definitionally equal under the fixed computation rules.
    #[error("terms are not definitionally equal: {left} != {right}")]
    NotDefinitionallyEqual { left: Box<Term>, right: Box<Term> },
    /// A term's inferred type does not match its expected type.
    #[error("type mismatch: expected {expected}, inferred {actual}")]
    TypeMismatch {
        expected: Box<Term>,
        actual: Box<Term>,
    },
    /// Serialized machine state violated an internal invariant.
    #[error("invalid kernel checkpoint: {0}")]
    InvalidCheckpoint(String),
}

/// Observable status of a resumable checking session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionStatus {
    /// More deterministic transitions remain.
    Running { steps: u64 },
    /// The request was accepted.
    Accepted { steps: u64, result: KernelResult },
    /// The request was rejected.
    Rejected { steps: u64, error: KernelError },
}

/// Entry point for the trusted checker.
#[derive(Clone, Copy, Debug, Default)]
pub struct Kernel;

impl Kernel {
    /// Starts a serializable, resumable session.
    #[must_use]
    pub fn start(theory: Theory, request: KernelRequest) -> CheckSession {
        match theory.validate() {
            Ok(()) => Self::start_assuming_valid_theory(theory, request),
            Err(error) => CheckSession {
                theory,
                control: None,
                frames: Vec::new(),
                steps: 0,
                terminal: Some(Err(KernelError::InvalidTheory(error.to_string()))),
                theory_validated: true,
            },
        }
    }

    pub(crate) fn start_assuming_valid_theory(
        theory: Theory,
        request: KernelRequest,
    ) -> CheckSession {
        CheckSession {
            theory,
            control: Some(Control::Begin { request }),
            frames: Vec::new(),
            steps: 0,
            terminal: None,
            theory_validated: true,
        }
    }

    /// Runs a finite request until acceptance or rejection. The calculus and theory
    /// construction rules guarantee termination for finite inputs, though no fixed
    /// runtime bound is imposed.
    pub fn run_to_completion(
        theory: Theory,
        request: KernelRequest,
    ) -> Result<KernelResult, KernelError> {
        Self::finish_session(Self::start(theory, request))
    }

    pub(crate) fn run_to_completion_assuming_valid_theory(
        theory: Theory,
        request: KernelRequest,
    ) -> Result<KernelResult, KernelError> {
        Self::finish_session(Self::start_assuming_valid_theory(theory, request))
    }

    fn finish_session(mut session: CheckSession) -> Result<KernelResult, KernelError> {
        loop {
            match session.run_slice(16_384) {
                SessionStatus::Running { .. } => {}
                SessionStatus::Accepted { result, .. } => return Ok(result),
                SessionStatus::Rejected { error, .. } => return Err(error),
            }
        }
    }
}

/// Exact continuation of a kernel computation. It can be serialized after any
/// transition and resumed without replaying completed transitions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckSession {
    theory: Theory,
    control: Option<Control>,
    frames: Vec<Frame>,
    steps: u64,
    terminal: Option<Result<KernelResult, KernelError>>,
    /// Deserialization deliberately clears this flag so a restored continuation
    /// revalidates its embedded theory before another transition.
    #[serde(skip)]
    theory_validated: bool,
}

impl CheckSession {
    /// Theory loaded by this session.
    #[must_use]
    pub fn theory(&self) -> &Theory {
        &self.theory
    }

    /// Number of completed machine transitions.
    #[must_use]
    pub const fn steps(&self) -> u64 {
        self.steps
    }

    /// Current status without executing work.
    #[must_use]
    pub fn status(&self) -> SessionStatus {
        match &self.terminal {
            None => SessionStatus::Running { steps: self.steps },
            Some(Ok(result)) => SessionStatus::Accepted {
                steps: self.steps,
                result: result.clone(),
            },
            Some(Err(error)) => SessionStatus::Rejected {
                steps: self.steps,
                error: error.clone(),
            },
        }
    }

    /// Executes at most `maximum_steps` transitions.
    pub fn run_slice(&mut self, maximum_steps: u64) -> SessionStatus {
        if !self.theory_validated {
            match self.theory.validate() {
                Ok(()) => self.theory_validated = true,
                Err(error) => {
                    self.reject(KernelError::InvalidTheory(error.to_string()));
                    return self.status();
                }
            }
        }
        for _ in 0..maximum_steps {
            if self.terminal.is_some() {
                break;
            }
            self.step();
        }
        self.status()
    }

    fn step(&mut self) {
        let Some(control) = self.control.take() else {
            self.reject(KernelError::InvalidCheckpoint(
                "running session has no control state".to_owned(),
            ));
            return;
        };
        self.steps = self.steps.saturating_add(1);
        if let Err(error) = self.execute(control) {
            self.reject(error);
        }
    }

    fn execute(&mut self, control: Control) -> Result<(), KernelError> {
        match control {
            Control::Begin { request } => {
                let context = request.context().clone();
                if context.is_empty() {
                    self.launch(request)?;
                } else {
                    self.frames.push(Frame::ContextEntry {
                        request,
                        next_index: 1,
                    });
                    self.control = Some(Control::EnsureUniverse {
                        context: Context::new(),
                        term: context.0[0].clone(),
                    });
                }
            }
            Control::Infer { context, term } => self.infer(context, term)?,
            Control::Check {
                context,
                term,
                expected,
            } => {
                if matches!(term, Term::Pair { .. }) {
                    self.check_now(&context, &term, &expected)?;
                    self.deliver(Value::Unit)?;
                } else {
                    self.frames.push(Frame::CheckAfterInfer {
                        expected,
                        context: context.clone(),
                    });
                    self.control = Some(Control::Infer { context, term });
                }
            }
            Control::EnsureUniverse { context, term } => {
                self.frames.push(Frame::EnsureUniverseAfterInfer);
                self.control = Some(Control::Infer { context, term });
            }
            Control::Whnf { term } => self.whnf(term)?,
            Control::DefEq {
                context,
                left,
                right,
            } => {
                self.frames.push(Frame::DefEqAfterLeft { right, context });
                self.control = Some(Control::Whnf { term: left });
            }
        }
        Ok(())
    }

    fn launch(&mut self, request: KernelRequest) -> Result<(), KernelError> {
        match request {
            KernelRequest::Infer { context, term } => {
                self.frames.push(Frame::TopInfer);
                self.control = Some(Control::Infer { context, term });
            }
            KernelRequest::EnsureUniverse { context, term } => {
                self.frames.push(Frame::TopUniverse);
                self.control = Some(Control::EnsureUniverse { context, term });
            }
            KernelRequest::Check {
                context,
                term,
                expected,
            } => {
                self.frames.push(Frame::ExpectedWellFormed {
                    context: context.clone(),
                    term,
                    expected: expected.clone(),
                    top: TopKind::Checked,
                });
                self.control = Some(Control::EnsureUniverse {
                    context,
                    term: expected,
                });
            }
            KernelRequest::Certificate { certificate } => {
                let loaded = self.theory.id();
                if certificate.theory != loaded {
                    return Err(KernelError::TheoryMismatch {
                        claimed: certificate.theory,
                        loaded,
                    });
                }
                self.frames.push(Frame::ExpectedWellFormed {
                    context: certificate.context.clone(),
                    term: certificate.proof,
                    expected: certificate.proposition.clone(),
                    top: TopKind::Certified,
                });
                self.control = Some(Control::EnsureUniverse {
                    context: certificate.context,
                    term: certificate.proposition,
                });
            }
            KernelRequest::DefEq {
                context,
                left,
                right,
            } => {
                self.frames.push(Frame::TopDefEq);
                self.control = Some(Control::DefEq {
                    context,
                    left,
                    right,
                });
            }
        }
        Ok(())
    }

    fn infer(&mut self, context: Context, term: Term) -> Result<(), KernelError> {
        match term {
            Term::Universe { level } => {
                let next = level
                    .checked_add(1)
                    .ok_or(KernelError::UniverseOverflow(level))?;
                self.deliver(Value::Term(Term::universe(next)))?;
            }
            Term::Var { index } => {
                let ty = context.lookup(index).ok_or(KernelError::UnboundVariable {
                    index,
                    context_len: context.len(),
                })?;
                self.deliver(Value::Term(ty))?;
            }
            Term::Const { name } => {
                let ty = self
                    .theory
                    .declaration(&name)
                    .map(|declaration| declaration.ty.clone())
                    .ok_or(KernelError::UnknownConstant(name))?;
                self.deliver(Value::Term(ty))?;
            }
            Term::Pi { domain, codomain } | Term::Sigma { domain, codomain } => {
                let domain = *domain;
                let codomain = *codomain;
                self.frames.push(Frame::InferPiAfterDomain {
                    domain: domain.clone(),
                    codomain,
                    context: context.clone(),
                });
                self.control = Some(Control::EnsureUniverse {
                    context,
                    term: domain,
                });
            }
            Term::Lam { domain, body } => {
                let domain = *domain;
                let body = *body;
                self.frames.push(Frame::InferLamAfterDomain {
                    domain: domain.clone(),
                    body,
                    context: context.clone(),
                });
                self.control = Some(Control::EnsureUniverse {
                    context,
                    term: domain,
                });
            }
            Term::App { function, argument } => {
                self.frames.push(Frame::InferAppAfterFunction {
                    argument: *argument,
                    context: context.clone(),
                });
                self.control = Some(Control::Infer {
                    context,
                    term: *function,
                });
            }
            Term::Pair { .. } => return Err(KernelError::CannotInferPair),
            Term::Fst { .. }
            | Term::Snd { .. }
            | Term::Id { .. }
            | Term::Refl { .. }
            | Term::J { .. }
            | Term::Elim { .. } => {
                let ty = self.infer_now(&context, &term)?;
                self.deliver(Value::Term(ty))?;
            }
            Term::Let {
                value_type,
                value,
                body,
            } => {
                let value_type = *value_type;
                self.frames.push(Frame::InferLetAfterType {
                    value_type: value_type.clone(),
                    value: *value,
                    body: *body,
                    context: context.clone(),
                });
                self.control = Some(Control::EnsureUniverse {
                    context,
                    term: value_type,
                });
            }
        }
        Ok(())
    }

    fn whnf(&mut self, term: Term) -> Result<(), KernelError> {
        match term {
            Term::Const { name } => {
                if let Some(body) = self.theory.unfold(&name) {
                    self.control = Some(Control::Whnf { term: body.clone() });
                } else {
                    self.deliver(Value::Term(Term::constant(name)))?;
                }
            }
            Term::Let { value, body, .. } => {
                self.control = Some(Control::Whnf {
                    term: subst_top(&value, &body),
                });
            }
            Term::App { function, argument } => {
                self.frames.push(Frame::WhnfAfterFunction {
                    argument: *argument,
                });
                self.control = Some(Control::Whnf { term: *function });
            }
            Term::Fst { pair } => match self.whnf_now(*pair)? {
                Term::Pair { first, .. } => {
                    self.control = Some(Control::Whnf { term: *first });
                }
                other => self.deliver(Value::Term(Term::fst(other)))?,
            },
            Term::Snd { pair } => match self.whnf_now(*pair)? {
                Term::Pair { second, .. } => {
                    self.control = Some(Control::Whnf { term: *second });
                }
                other => self.deliver(Value::Term(Term::snd(other)))?,
            },
            Term::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => {
                let equality = self.whnf_now(*equality)?;
                if self.reflexivity_matches(&lhs, &rhs, &equality)? {
                    self.control = Some(Control::Whnf {
                        term: Term::app(*refl_case, *lhs),
                    });
                } else {
                    self.deliver(Value::Term(Term::j(
                        *ty, *motive, *refl_case, *lhs, *rhs, equality,
                    )))?;
                }
            }
            Term::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => {
                if let Some(reduced) = self.reduce_elim_now(
                    &inductive,
                    &parameters,
                    &indices,
                    &motive,
                    &branches,
                    &scrutinee,
                )? {
                    self.control = Some(Control::Whnf { term: reduced });
                } else {
                    self.deliver(Value::Term(Term::elim(
                        inductive, parameters, indices, *motive, branches, *scrutinee,
                    )))?;
                }
            }
            other => self.deliver(Value::Term(other))?,
        }
        Ok(())
    }

    fn deliver(&mut self, value: Value) -> Result<(), KernelError> {
        let Some(frame) = self.frames.pop() else {
            return Err(KernelError::InvalidCheckpoint(
                "value returned without a continuation frame".to_owned(),
            ));
        };
        match frame {
            Frame::ContextEntry {
                request,
                next_index,
            } => {
                value.expect_universe()?;
                let context = request.context().clone();
                if next_index < context.len() {
                    self.frames.push(Frame::ContextEntry {
                        request,
                        next_index: next_index + 1,
                    });
                    self.control = Some(Control::EnsureUniverse {
                        context: context.prefix(next_index),
                        term: context.0[next_index].clone(),
                    });
                } else {
                    self.launch(request)?;
                }
            }
            Frame::TopInfer => {
                self.frames.push(Frame::TopInferAfterWhnf);
                self.control = Some(Control::Whnf {
                    term: value.expect_term()?,
                });
            }
            Frame::TopInferAfterWhnf => {
                self.accept(KernelResult::Inferred(value.expect_term()?));
            }
            Frame::TopUniverse => {
                self.accept(KernelResult::Universe(value.expect_universe()?));
            }
            Frame::TopChecked => {
                value.expect_unit()?;
                self.accept(KernelResult::Checked);
            }
            Frame::TopCertified => {
                value.expect_unit()?;
                self.accept(KernelResult::Certified);
            }
            Frame::TopDefEq => {
                value.expect_unit()?;
                self.accept(KernelResult::DefinitionallyEqual);
            }
            Frame::ExpectedWellFormed {
                context,
                term,
                expected,
                top,
            } => {
                value.expect_universe()?;
                self.frames.push(match top {
                    TopKind::Checked => Frame::TopChecked,
                    TopKind::Certified => Frame::TopCertified,
                });
                self.control = Some(Control::Check {
                    context,
                    term,
                    expected,
                });
            }
            Frame::EnsureUniverseAfterInfer => {
                self.frames.push(Frame::EnsureUniverseAfterWhnf);
                self.control = Some(Control::Whnf {
                    term: value.expect_term()?,
                });
            }
            Frame::EnsureUniverseAfterWhnf => match value.expect_term()? {
                Term::Universe { level } => self.deliver(Value::Universe(level))?,
                found => {
                    return Err(KernelError::ExpectedUniverse {
                        found: Box::new(found),
                    });
                }
            },
            Frame::CheckAfterInfer { expected, context } => {
                let actual = value.expect_term()?;
                if self.compatible_now(&context, &actual, &expected)? {
                    self.deliver(Value::Unit)?;
                } else {
                    return Err(KernelError::TypeMismatch {
                        expected: Box::new(expected),
                        actual: Box::new(actual),
                    });
                }
            }
            Frame::InferPiAfterDomain {
                domain,
                codomain,
                context,
            } => {
                let domain_level = value.expect_universe()?;
                self.frames
                    .push(Frame::InferPiAfterCodomain { domain_level });
                self.control = Some(Control::EnsureUniverse {
                    context: context.extend(domain),
                    term: codomain,
                });
            }
            Frame::InferPiAfterCodomain { domain_level } => {
                let codomain_level = value.expect_universe()?;
                self.deliver(Value::Term(Term::universe(
                    domain_level.max(codomain_level),
                )))?;
            }
            Frame::InferLamAfterDomain {
                domain,
                body,
                context,
            } => {
                value.expect_universe()?;
                self.frames.push(Frame::InferLamAfterBody {
                    domain: domain.clone(),
                });
                self.control = Some(Control::Infer {
                    context: context.extend(domain),
                    term: body,
                });
            }
            Frame::InferLamAfterBody { domain } => {
                self.deliver(Value::Term(Term::pi(domain, value.expect_term()?)))?;
            }
            Frame::InferAppAfterFunction { argument, context } => {
                self.frames
                    .push(Frame::InferAppAfterFunctionWhnf { argument, context });
                self.control = Some(Control::Whnf {
                    term: value.expect_term()?,
                });
            }
            Frame::InferAppAfterFunctionWhnf { argument, context } => match value.expect_term()? {
                Term::Pi { domain, codomain } => {
                    self.frames.push(Frame::InferAppAfterArgument {
                        argument: argument.clone(),
                        codomain: *codomain,
                    });
                    self.control = Some(Control::Check {
                        context,
                        term: argument,
                        expected: *domain,
                    });
                }
                found => {
                    return Err(KernelError::ExpectedFunction {
                        found: Box::new(found),
                    });
                }
            },
            Frame::InferAppAfterArgument { argument, codomain } => {
                value.expect_unit()?;
                self.deliver(Value::Term(subst_top(&argument, &codomain)))?;
            }
            Frame::InferLetAfterType {
                value_type,
                value: let_value,
                body,
                context,
            } => {
                value.expect_universe()?;
                self.frames.push(Frame::InferLetAfterValue {
                    value_type: value_type.clone(),
                    value: let_value.clone(),
                    body,
                    context: context.clone(),
                });
                self.control = Some(Control::Check {
                    context,
                    term: let_value,
                    expected: value_type,
                });
            }
            Frame::InferLetAfterValue {
                value_type,
                value: let_value,
                body,
                context,
            } => {
                value.expect_unit()?;
                self.frames
                    .push(Frame::InferLetAfterBody { value: let_value });
                self.control = Some(Control::Infer {
                    context: context.extend(value_type),
                    term: body,
                });
            }
            Frame::InferLetAfterBody { value: let_value } => {
                self.deliver(Value::Term(subst_top(&let_value, &value.expect_term()?)))?;
            }
            Frame::WhnfAfterFunction { argument } => {
                let function = value.expect_term()?;
                match function {
                    Term::Lam { body, .. } => {
                        self.control = Some(Control::Whnf {
                            term: subst_top(&argument, &body),
                        });
                    }
                    other => self.deliver(Value::Term(Term::app(other, argument)))?,
                }
            }
            Frame::DefEqAfterLeft { right, context } => {
                self.frames.push(Frame::DefEqAfterRight {
                    left: value.expect_term()?,
                    context,
                });
                self.control = Some(Control::Whnf { term: right });
            }
            Frame::DefEqAfterRight { left, context } => {
                let right = value.expect_term()?;
                self.compare_whnf(left, right, &context)?;
            }
        }
        Ok(())
    }

    fn compare_whnf(
        &mut self,
        left: Term,
        right: Term,
        context: &Context,
    ) -> Result<(), KernelError> {
        if self.defeq_now(context, &left, &right)? {
            self.deliver(Value::Unit)?;
            Ok(())
        } else {
            Err(KernelError::NotDefinitionallyEqual {
                left: Box::new(left),
                right: Box::new(right),
            })
        }
    }

    fn ensure_universe_now(&self, context: &Context, term: &Term) -> Result<u32, KernelError> {
        let inferred = self.infer_now(context, term)?;
        match self.whnf_now(inferred)? {
            Term::Universe { level } => Ok(level),
            found => Err(KernelError::ExpectedUniverse {
                found: Box::new(found),
            }),
        }
    }

    fn check_now(
        &self,
        context: &Context,
        term: &Term,
        expected: &Term,
    ) -> Result<(), KernelError> {
        if let Term::Pair { first, second } = term {
            let sigma = self.whnf_now(expected.clone())?;
            if let Term::Sigma { domain, codomain } = sigma {
                self.check_now(context, first, &domain)?;
                let second_type = subst_top(first, &codomain);
                self.check_now(context, second, &second_type)?;
                return Ok(());
            }
            return Err(KernelError::ExpectedSigma {
                found: Box::new(sigma),
            });
        }
        let actual = self.infer_now(context, term)?;
        if self.compatible_now(context, &actual, expected)? {
            Ok(())
        } else {
            Err(KernelError::TypeMismatch {
                expected: Box::new(expected.clone()),
                actual: Box::new(actual),
            })
        }
    }

    fn compatible_now(
        &self,
        context: &Context,
        actual: &Term,
        expected: &Term,
    ) -> Result<bool, KernelError> {
        let actual_whnf = self.whnf_now(actual.clone())?;
        let expected_whnf = self.whnf_now(expected.clone())?;
        if let (
            Term::Universe {
                level: actual_level,
            },
            Term::Universe {
                level: expected_level,
            },
        ) = (&actual_whnf, &expected_whnf)
        {
            return Ok(actual_level <= expected_level);
        }
        self.defeq_now(context, &actual_whnf, &expected_whnf)
    }

    fn infer_now(&self, context: &Context, term: &Term) -> Result<Term, KernelError> {
        match term {
            Term::Universe { level } => level
                .checked_add(1)
                .map(Term::universe)
                .ok_or(KernelError::UniverseOverflow(*level)),
            Term::Var { index } => context.lookup(*index).ok_or(KernelError::UnboundVariable {
                index: *index,
                context_len: context.len(),
            }),
            Term::Const { name } => self
                .theory
                .declaration(name)
                .map(|declaration| declaration.ty.clone())
                .ok_or_else(|| KernelError::UnknownConstant(name.clone())),
            Term::Pi { domain, codomain } | Term::Sigma { domain, codomain } => {
                let domain_level = self.ensure_universe_now(context, domain)?;
                let codomain_level =
                    self.ensure_universe_now(&context.extend((**domain).clone()), codomain)?;
                Ok(Term::universe(domain_level.max(codomain_level)))
            }
            Term::Lam { domain, body } => {
                self.ensure_universe_now(context, domain)?;
                let body_type = self.infer_now(&context.extend((**domain).clone()), body)?;
                Ok(Term::pi((**domain).clone(), body_type))
            }
            Term::App { function, argument } => {
                let function_type = self.whnf_now(self.infer_now(context, function)?)?;
                match function_type {
                    Term::Pi { domain, codomain } => {
                        self.check_now(context, argument, &domain)?;
                        Ok(subst_top(argument, &codomain))
                    }
                    found => Err(KernelError::ExpectedFunction {
                        found: Box::new(found),
                    }),
                }
            }
            Term::Pair { .. } => Err(KernelError::CannotInferPair),
            Term::Fst { pair } => {
                let pair_type = self.whnf_now(self.infer_now(context, pair)?)?;
                match pair_type {
                    Term::Sigma { domain, .. } => Ok(*domain),
                    found => Err(KernelError::ExpectedSigma {
                        found: Box::new(found),
                    }),
                }
            }
            Term::Snd { pair } => {
                let pair_type = self.whnf_now(self.infer_now(context, pair)?)?;
                match pair_type {
                    Term::Sigma { codomain, .. } => {
                        Ok(subst_top(&Term::fst((**pair).clone()), &codomain))
                    }
                    found => Err(KernelError::ExpectedSigma {
                        found: Box::new(found),
                    }),
                }
            }
            Term::Id { ty, lhs, rhs } => {
                let level = self.ensure_universe_now(context, ty)?;
                self.check_now(context, lhs, ty)?;
                self.check_now(context, rhs, ty)?;
                Ok(Term::universe(level))
            }
            Term::Refl { value: term } => {
                let ty = self.infer_now(context, term)?;
                Ok(Term::id(ty, (**term).clone(), (**term).clone()))
            }
            Term::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => self.infer_j_now(context, ty, motive, refl_case, lhs, rhs, equality),
            Term::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => self.infer_elim_now(
                context, inductive, parameters, indices, motive, branches, scrutinee,
            ),
            Term::Let {
                value_type,
                value,
                body,
            } => {
                self.ensure_universe_now(context, value_type)?;
                self.check_now(context, value, value_type)?;
                let body_type = self.infer_now(&context.extend((**value_type).clone()), body)?;
                Ok(subst_top(value, &body_type))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_j_now(
        &self,
        context: &Context,
        ty: &Term,
        motive: &Term,
        refl_case: &Term,
        lhs: &Term,
        rhs: &Term,
        equality: &Term,
    ) -> Result<Term, KernelError> {
        self.ensure_universe_now(context, ty)?;

        let motive_type = self.whnf_now(self.infer_now(context, motive)?)?;
        let Term::Pi {
            domain: x_domain,
            codomain: after_x,
        } = motive_type
        else {
            return Err(KernelError::InvalidIdentityMotive(
                "motive is not a function of the left endpoint".to_owned(),
            ));
        };
        if !self.defeq_now(context, &x_domain, ty)? {
            return Err(KernelError::InvalidIdentityMotive(
                "left endpoint domain differs from the identity carrier".to_owned(),
            ));
        }

        let left_endpoint_context = context.extend(ty.clone());
        let after_x = self.whnf_now(*after_x)?;
        let Term::Pi {
            domain: y_domain,
            codomain: after_y,
        } = after_x
        else {
            return Err(KernelError::InvalidIdentityMotive(
                "motive is not a function of the right endpoint".to_owned(),
            ));
        };
        let shifted_ty_one = shift(ty, 1, 0);
        if !self.defeq_now(&left_endpoint_context, &y_domain, &shifted_ty_one)? {
            return Err(KernelError::InvalidIdentityMotive(
                "right endpoint domain differs from the identity carrier".to_owned(),
            ));
        }

        let endpoint_context = left_endpoint_context.extend((*y_domain).clone());
        let after_y = self.whnf_now(*after_y)?;
        let Term::Pi {
            domain: equality_domain,
            codomain: result_family,
        } = after_y
        else {
            return Err(KernelError::InvalidIdentityMotive(
                "motive is not a function of an equality witness".to_owned(),
            ));
        };
        let expected_equality_domain = Term::id(shift(ty, 2, 0), Term::var(1), Term::var(0));
        if !self.defeq_now(
            &endpoint_context,
            &equality_domain,
            &expected_equality_domain,
        )? {
            return Err(KernelError::InvalidIdentityMotive(
                "third motive argument is not the endpoint identity".to_owned(),
            ));
        }
        let motive_context = endpoint_context.extend((*equality_domain).clone());
        self.ensure_universe_now(&motive_context, &result_family)?;

        let shifted_motive = shift(motive, 1, 0);
        let reflexive_result = Term::apply_many(
            shifted_motive,
            [Term::var(0), Term::var(0), Term::refl(Term::var(0))],
        );
        let expected_refl_case = Term::pi(ty.clone(), reflexive_result);
        self.check_now(context, refl_case, &expected_refl_case)?;

        self.check_now(context, lhs, ty)?;
        self.check_now(context, rhs, ty)?;
        self.check_now(
            context,
            equality,
            &Term::id(ty.clone(), lhs.clone(), rhs.clone()),
        )?;

        let result = Term::apply_many(
            (*motive).clone(),
            [lhs.clone(), rhs.clone(), equality.clone()],
        );
        self.ensure_universe_now(context, &result)?;
        Ok(result)
    }

    fn reflexivity_matches(
        &self,
        lhs: &Term,
        rhs: &Term,
        equality: &Term,
    ) -> Result<bool, KernelError> {
        let Term::Refl { value } = equality else {
            return Ok(false);
        };
        let context = Context::new();
        Ok(self.defeq_now(&context, value, lhs)? && self.defeq_now(&context, value, rhs)?)
    }

    #[allow(clippy::too_many_arguments)]
    fn infer_elim_now(
        &self,
        context: &Context,
        inductive_name: &Name,
        parameters: &[Term],
        indices: &[Term],
        motive: &Term,
        branches: &[Term],
        scrutinee: &Term,
    ) -> Result<Term, KernelError> {
        let inductive = self
            .theory
            .inductive(inductive_name)
            .cloned()
            .ok_or_else(|| KernelError::UnknownInductive(inductive_name.clone()))?;
        if parameters.len() != inductive.parameter_count() {
            return Err(Self::invalid_elim(
                inductive_name,
                format!(
                    "expected {expected} parameters, found {actual}",
                    expected = inductive.parameter_count(),
                    actual = parameters.len(),
                ),
            ));
        }
        if indices.len() != inductive.index_count() {
            return Err(Self::invalid_elim(
                inductive_name,
                format!(
                    "expected {expected} indices, found {actual}",
                    expected = inductive.index_count(),
                    actual = indices.len(),
                ),
            ));
        }
        if branches.len() != inductive.constructors.len() {
            return Err(Self::invalid_elim(
                inductive_name,
                format!(
                    "expected {expected} branches, found {actual}",
                    expected = inductive.constructors.len(),
                    actual = branches.len(),
                ),
            ));
        }

        self.check_telescope_arguments(context, &inductive.parameters, &[], parameters)?;
        self.check_telescope_arguments(context, &inductive.indices, parameters, indices)?;

        let expected_scrutinee = Term::apply_many(
            Term::constant(inductive.name.clone()),
            parameters.iter().chain(indices).cloned(),
        );
        self.check_now(context, scrutinee, &expected_scrutinee)?;

        let motive_type = self.whnf_now(self.infer_now(context, motive)?)?;
        let motive_level = self.pi_result_universe(motive_type.clone(), indices.len() + 1)?;
        let expected_motive = Self::motive_type(&inductive, parameters, motive_level);
        if !self.defeq_now(context, &motive_type, &expected_motive)? {
            return Err(Self::invalid_elim(
                inductive_name,
                format!("motive type {motive_type} does not match {expected_motive}"),
            ));
        }

        for (position, (branch, constructor)) in
            branches.iter().zip(&inductive.constructors).enumerate()
        {
            let expected = Self::branch_type(&inductive, constructor, parameters, motive);
            self.check_now(context, branch, &expected)
                .map_err(|error| {
                    Self::invalid_elim(
                        inductive_name,
                        format!(
                            "branch {position} ({constructor}) is invalid: {error}",
                            constructor = constructor.name,
                        ),
                    )
                })?;
        }

        Ok(Term::apply_many(
            motive.clone(),
            indices
                .iter()
                .cloned()
                .chain(std::iter::once(scrutinee.clone())),
        ))
    }

    fn invalid_elim(family: &Name, message: String) -> KernelError {
        KernelError::InvalidInductiveEliminator {
            family: family.clone(),
            message,
        }
    }

    fn check_telescope_arguments(
        &self,
        context: &Context,
        telescope: &[Term],
        prefix: &[Term],
        arguments: &[Term],
    ) -> Result<(), KernelError> {
        if telescope.len() != arguments.len() {
            return Err(KernelError::InvalidCheckpoint(
                "telescope argument count was checked inconsistently".to_owned(),
            ));
        }
        let mut witnesses = prefix.to_vec();
        for (binder, argument) in telescope.iter().zip(arguments) {
            let expected = instantiate(binder, &witnesses);
            self.check_now(context, argument, &expected)?;
            witnesses.push(argument.clone());
        }
        Ok(())
    }

    fn pi_result_universe(&self, mut ty: Term, binders: usize) -> Result<u32, KernelError> {
        for _ in 0..binders {
            ty = self.whnf_now(ty)?;
            let Term::Pi { codomain, .. } = ty else {
                return Err(KernelError::InvalidIdentityMotive(
                    "expected another dependent-function binder".to_owned(),
                ));
            };
            ty = *codomain;
        }
        match self.whnf_now(ty)? {
            Term::Universe { level } => Ok(level),
            found => Err(KernelError::ExpectedUniverse {
                found: Box::new(found),
            }),
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn motive_type(inductive: &InductiveDeclaration, parameters: &[Term], level: u32) -> Term {
        let index_count = inductive.indices.len();
        let index_domains: Vec<_> = inductive
            .indices
            .iter()
            .enumerate()
            .map(|(position, domain)| instantiate_under(domain, parameters, position as u32))
            .collect();
        let shifted_parameters = parameters
            .iter()
            .map(|parameter| shift(parameter, index_count as i64, 0));
        let index_variables =
            (0..index_count).map(|position| Term::var((index_count - position - 1) as u32));
        let scrutinee_type = Term::apply_many(
            Term::constant(inductive.name.clone()),
            shifted_parameters.chain(index_variables),
        );
        Term::pi_many(
            index_domains
                .into_iter()
                .chain(std::iter::once(scrutinee_type)),
            Term::universe(level),
        )
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn branch_type(
        inductive: &InductiveDeclaration,
        constructor: &InductiveConstructor,
        parameters: &[Term],
        motive: &Term,
    ) -> Term {
        let field_count = constructor.fields.len();
        let field_domains: Vec<_> = constructor
            .fields
            .iter()
            .enumerate()
            .map(|(position, field)| {
                instantiate_under(
                    &inductive.field_type(field, position),
                    parameters,
                    position as u32,
                )
            })
            .collect();

        let mut ih_domains = Vec::new();
        for (field_position, field) in constructor.fields.iter().enumerate() {
            let ih_count = ih_domains.len();
            match field {
                ConstructorField::Plain { .. } => {}
                ConstructorField::Recursive { indices } => {
                    let extra_fields = field_count - field_position;
                    let recursive_indices = indices.iter().map(|index| {
                        let instantiated =
                            instantiate_under(index, parameters, field_position as u32);
                        shift(&instantiated, (extra_fields + ih_count) as i64, 0)
                    });
                    let recursive_value =
                        Term::var((ih_count + field_count - field_position - 1) as u32);
                    let shifted_motive = shift(motive, (field_count + ih_count) as i64, 0);
                    ih_domains.push(Term::apply_many(
                        shifted_motive,
                        recursive_indices.chain(std::iter::once(recursive_value)),
                    ));
                }
                ConstructorField::RecursiveFunction { domain, indices } => {
                    let extra = field_count - field_position + ih_count;
                    let domain = instantiate_under(domain, parameters, field_position as u32);
                    let domain = shift(&domain, extra as i64, 0);
                    let recursive_indices = indices.iter().map(|index| {
                        let instantiated =
                            instantiate_under(index, parameters, (field_position + 1) as u32);
                        shift(&instantiated, extra as i64, 1)
                    });
                    let function =
                        Term::var((1 + ih_count + field_count - field_position - 1) as u32);
                    let recursive_value = Term::app(function, Term::var(0));
                    let shifted_motive = shift(motive, (field_count + ih_count + 1) as i64, 0);
                    let result = Term::apply_many(
                        shifted_motive,
                        recursive_indices.chain(std::iter::once(recursive_value)),
                    );
                    ih_domains.push(Term::pi(domain, result));
                }
            }
        }

        let ih_count = ih_domains.len();
        let result_indices = constructor.result_indices.iter().map(|index| {
            let instantiated = instantiate_under(index, parameters, field_count as u32);
            shift(&instantiated, ih_count as i64, 0)
        });
        let constructor_parameters = parameters
            .iter()
            .map(|parameter| shift(parameter, (field_count + ih_count) as i64, 0));
        let field_variables = (0..field_count)
            .map(|position| Term::var((ih_count + field_count - position - 1) as u32));
        let constructor_value = Term::apply_many(
            Term::constant(constructor.name.clone()),
            constructor_parameters.chain(field_variables),
        );
        let shifted_motive = shift(motive, (field_count + ih_count) as i64, 0);
        let result = Term::apply_many(
            shifted_motive,
            result_indices.chain(std::iter::once(constructor_value)),
        );
        Term::pi_many(field_domains.into_iter().chain(ih_domains), result)
    }

    fn reduce_elim_now(
        &self,
        inductive_name: &Name,
        parameters: &[Term],
        indices: &[Term],
        motive: &Term,
        branches: &[Term],
        scrutinee: &Term,
    ) -> Result<Option<Term>, KernelError> {
        let scrutinee_whnf = self.whnf_now(scrutinee.clone())?;
        let Some((constructor_name, arguments)) = scrutinee_whnf.constant_spine() else {
            return Ok(None);
        };
        let Some((owner, constructor_index, constructor)) =
            self.theory.constructor(constructor_name)
        else {
            return Ok(None);
        };
        if owner.name != *inductive_name {
            return Ok(None);
        }
        let parameter_count = owner.parameter_count();
        if arguments.len() != parameter_count + constructor.fields.len()
            || constructor_index >= branches.len()
        {
            return Ok(None);
        }
        let comparison_context = Context::new();
        for (actual, expected) in arguments[..parameter_count].iter().zip(parameters) {
            if !self.defeq_now(&comparison_context, actual, expected)? {
                return Ok(None);
            }
        }
        let field_arguments: Vec<Term> = arguments[parameter_count..]
            .iter()
            .map(|argument| (*argument).clone())
            .collect();
        let result_witnesses: Vec<Term> = parameters
            .iter()
            .cloned()
            .chain(field_arguments.iter().cloned())
            .collect();
        if constructor.result_indices.len() != indices.len() {
            return Ok(None);
        }
        for (declared, supplied) in constructor.result_indices.iter().zip(indices) {
            let actual = instantiate(declared, &result_witnesses);
            if !self.defeq_now(&comparison_context, &actual, supplied)? {
                return Ok(None);
            }
        }
        let mut induction_hypotheses = Vec::new();
        for (position, field) in constructor.fields.iter().enumerate() {
            let previous: Vec<Term> = parameters
                .iter()
                .cloned()
                .chain(field_arguments[..position].iter().cloned())
                .collect();
            match field {
                ConstructorField::Plain { .. } => {}
                ConstructorField::Recursive {
                    indices: recursive_indices,
                } => {
                    let recursive_indices = recursive_indices
                        .iter()
                        .map(|index| instantiate(index, &previous))
                        .collect();
                    induction_hypotheses.push(Term::elim(
                        inductive_name.clone(),
                        parameters.to_vec(),
                        recursive_indices,
                        motive.clone(),
                        branches.to_vec(),
                        field_arguments[position].clone(),
                    ));
                }
                ConstructorField::RecursiveFunction {
                    domain,
                    indices: recursive_indices,
                } => {
                    let domain = instantiate(domain, &previous);
                    let recursive_indices = recursive_indices
                        .iter()
                        .map(|index| instantiate_under(index, &previous, 1))
                        .collect();
                    let shifted_parameters = parameters
                        .iter()
                        .map(|parameter| shift(parameter, 1, 0))
                        .collect();
                    let shifted_branches =
                        branches.iter().map(|branch| shift(branch, 1, 0)).collect();
                    let recursive_value =
                        Term::app(shift(&field_arguments[position], 1, 0), Term::var(0));
                    induction_hypotheses.push(Term::lam(
                        domain,
                        Term::elim(
                            inductive_name.clone(),
                            shifted_parameters,
                            recursive_indices,
                            shift(motive, 1, 0),
                            shifted_branches,
                            recursive_value,
                        ),
                    ));
                }
            }
        }
        let branch = branches[constructor_index].clone();
        let reduced = Term::apply_many(
            branch,
            field_arguments.into_iter().chain(induction_hypotheses),
        );
        Ok(Some(reduced))
    }

    fn whnf_now(&self, term: Term) -> Result<Term, KernelError> {
        match term {
            Term::Const { name } => {
                if let Some(body) = self.theory.unfold(&name) {
                    self.whnf_now(body.clone())
                } else {
                    Ok(Term::constant(name))
                }
            }
            Term::Let { value, body, .. } => self.whnf_now(subst_top(&value, &body)),
            Term::App { function, argument } => {
                let function = self.whnf_now(*function)?;
                match function {
                    Term::Lam { body, .. } => self.whnf_now(subst_top(&argument, &body)),
                    other => Ok(Term::app(other, *argument)),
                }
            }
            Term::Fst { pair } => {
                let pair = self.whnf_now(*pair)?;
                match pair {
                    Term::Pair { first, .. } => self.whnf_now(*first),
                    other => Ok(Term::fst(other)),
                }
            }
            Term::Snd { pair } => {
                let pair = self.whnf_now(*pair)?;
                match pair {
                    Term::Pair { second, .. } => self.whnf_now(*second),
                    other => Ok(Term::snd(other)),
                }
            }
            Term::J {
                ty,
                motive,
                refl_case,
                lhs,
                rhs,
                equality,
            } => {
                let equality = self.whnf_now(*equality)?;
                if self.reflexivity_matches(&lhs, &rhs, &equality)? {
                    self.whnf_now(Term::app(*refl_case, *lhs))
                } else {
                    Ok(Term::j(*ty, *motive, *refl_case, *lhs, *rhs, equality))
                }
            }
            Term::Elim {
                inductive,
                parameters,
                indices,
                motive,
                branches,
                scrutinee,
            } => match self.reduce_elim_now(
                &inductive,
                &parameters,
                &indices,
                &motive,
                &branches,
                &scrutinee,
            )? {
                Some(reduced) => self.whnf_now(reduced),
                None => Ok(Term::elim(
                    inductive, parameters, indices, *motive, branches, *scrutinee,
                )),
            },
            other => Ok(other),
        }
    }

    fn defeq_now(&self, context: &Context, left: &Term, right: &Term) -> Result<bool, KernelError> {
        let left = self.whnf_now(left.clone())?;
        let right = self.whnf_now(right.clone())?;
        if left == right {
            return Ok(true);
        }
        match (&left, &right) {
            (
                Term::Pi {
                    domain: left_domain,
                    codomain: left_codomain,
                },
                Term::Pi {
                    domain: right_domain,
                    codomain: right_codomain,
                },
            )
            | (
                Term::Sigma {
                    domain: left_domain,
                    codomain: left_codomain,
                },
                Term::Sigma {
                    domain: right_domain,
                    codomain: right_codomain,
                },
            )
            | (
                Term::Lam {
                    domain: left_domain,
                    body: left_codomain,
                },
                Term::Lam {
                    domain: right_domain,
                    body: right_codomain,
                },
            ) => Ok(self.defeq_now(context, left_domain, right_domain)?
                && self.defeq_now(
                    &context.extend((**left_domain).clone()),
                    left_codomain,
                    right_codomain,
                )?),
            (
                Term::App {
                    function: left_function,
                    argument: left_argument,
                },
                Term::App {
                    function: right_function,
                    argument: right_argument,
                },
            ) => Ok(self.defeq_now(context, left_function, right_function)?
                && self.defeq_now(context, left_argument, right_argument)?),
            (
                Term::Pair {
                    first: left_first,
                    second: left_second,
                },
                Term::Pair {
                    first: right_first,
                    second: right_second,
                },
            ) => Ok(self.defeq_now(context, left_first, right_first)?
                && self.defeq_now(context, left_second, right_second)?),
            (Term::Fst { pair: left_pair }, Term::Fst { pair: right_pair })
            | (Term::Snd { pair: left_pair }, Term::Snd { pair: right_pair })
            | (Term::Refl { value: left_pair }, Term::Refl { value: right_pair }) => {
                self.defeq_now(context, left_pair, right_pair)
            }
            (
                Term::Id {
                    ty: left_ty,
                    lhs: left_lhs,
                    rhs: left_rhs,
                },
                Term::Id {
                    ty: right_ty,
                    lhs: right_lhs,
                    rhs: right_rhs,
                },
            ) => Ok(self.defeq_now(context, left_ty, right_ty)?
                && self.defeq_now(context, left_lhs, right_lhs)?
                && self.defeq_now(context, left_rhs, right_rhs)?),
            (
                Term::J {
                    ty: left_ty,
                    motive: left_motive,
                    refl_case: left_refl,
                    lhs: left_lhs,
                    rhs: left_rhs,
                    equality: left_equality,
                },
                Term::J {
                    ty: right_ty,
                    motive: right_motive,
                    refl_case: right_refl,
                    lhs: right_lhs,
                    rhs: right_rhs,
                    equality: right_equality,
                },
            ) => Ok(self.defeq_now(context, left_ty, right_ty)?
                && self.defeq_now(context, left_motive, right_motive)?
                && self.defeq_now(context, left_refl, right_refl)?
                && self.defeq_now(context, left_lhs, right_lhs)?
                && self.defeq_now(context, left_rhs, right_rhs)?
                && self.defeq_now(context, left_equality, right_equality)?),
            (
                Term::Elim {
                    inductive: left_inductive,
                    parameters: left_parameters,
                    indices: left_indices,
                    motive: left_motive,
                    branches: left_branches,
                    scrutinee: left_scrutinee,
                },
                Term::Elim {
                    inductive: right_inductive,
                    parameters: right_parameters,
                    indices: right_indices,
                    motive: right_motive,
                    branches: right_branches,
                    scrutinee: right_scrutinee,
                },
            ) => {
                if left_inductive != right_inductive
                    || left_parameters.len() != right_parameters.len()
                    || left_indices.len() != right_indices.len()
                    || left_branches.len() != right_branches.len()
                {
                    return Ok(false);
                }
                for (left, right) in left_parameters.iter().zip(right_parameters) {
                    if !self.defeq_now(context, left, right)? {
                        return Ok(false);
                    }
                }
                for (left, right) in left_indices.iter().zip(right_indices) {
                    if !self.defeq_now(context, left, right)? {
                        return Ok(false);
                    }
                }
                if !self.defeq_now(context, left_motive, right_motive)? {
                    return Ok(false);
                }
                for (left, right) in left_branches.iter().zip(right_branches) {
                    if !self.defeq_now(context, left, right)? {
                        return Ok(false);
                    }
                }
                self.defeq_now(context, left_scrutinee, right_scrutinee)
            }
            _ => Ok(false),
        }
    }

    fn accept(&mut self, result: KernelResult) {
        self.control = None;
        self.frames.clear();
        self.terminal = Some(Ok(result));
    }

    fn reject(&mut self, error: KernelError) {
        self.control = None;
        self.frames.clear();
        self.terminal = Some(Err(error));
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Control {
    Begin {
        request: KernelRequest,
    },
    Infer {
        context: Context,
        term: Term,
    },
    Check {
        context: Context,
        term: Term,
        expected: Term,
    },
    EnsureUniverse {
        context: Context,
        term: Term,
    },
    Whnf {
        term: Term,
    },
    DefEq {
        context: Context,
        left: Term,
        right: Term,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
enum TopKind {
    Checked,
    Certified,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Frame {
    ContextEntry {
        request: KernelRequest,
        next_index: usize,
    },
    TopInfer,
    TopInferAfterWhnf,
    TopUniverse,
    TopChecked,
    TopCertified,
    TopDefEq,
    ExpectedWellFormed {
        context: Context,
        term: Term,
        expected: Term,
        top: TopKind,
    },
    EnsureUniverseAfterInfer,
    EnsureUniverseAfterWhnf,
    CheckAfterInfer {
        expected: Term,
        context: Context,
    },
    InferPiAfterDomain {
        domain: Term,
        codomain: Term,
        context: Context,
    },
    InferPiAfterCodomain {
        domain_level: u32,
    },
    InferLamAfterDomain {
        domain: Term,
        body: Term,
        context: Context,
    },
    InferLamAfterBody {
        domain: Term,
    },
    InferAppAfterFunction {
        argument: Term,
        context: Context,
    },
    InferAppAfterFunctionWhnf {
        argument: Term,
        context: Context,
    },
    InferAppAfterArgument {
        argument: Term,
        codomain: Term,
    },
    InferLetAfterType {
        value_type: Term,
        value: Term,
        body: Term,
        context: Context,
    },
    InferLetAfterValue {
        value_type: Term,
        value: Term,
        body: Term,
        context: Context,
    },
    InferLetAfterBody {
        value: Term,
    },
    WhnfAfterFunction {
        argument: Term,
    },
    DefEqAfterLeft {
        right: Term,
        context: Context,
    },
    DefEqAfterRight {
        left: Term,
        context: Context,
    },
}

#[derive(Clone, Debug)]
enum Value {
    Term(Term),
    Universe(u32),
    Unit,
}

impl Value {
    fn expect_term(self) -> Result<Term, KernelError> {
        match self {
            Self::Term(term) => Ok(term),
            other => Err(KernelError::InvalidCheckpoint(format!(
                "expected term value, found {other:?}"
            ))),
        }
    }

    fn expect_universe(self) -> Result<u32, KernelError> {
        match self {
            Self::Universe(level) => Ok(level),
            other => Err(KernelError::InvalidCheckpoint(format!(
                "expected universe value, found {other:?}"
            ))),
        }
    }

    fn expect_unit(self) -> Result<(), KernelError> {
        match self {
            Self::Unit => Ok(()),
            other => Err(KernelError::InvalidCheckpoint(format!(
                "expected unit value, found {other:?}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Provenance, TheoryBuilder};

    fn empty() -> Theory {
        Theory::empty("test", "1")
    }

    #[test]
    fn identity_is_dependently_typed() {
        // λ A:Type0. λ x:A. x
        let term = Term::lam(Term::universe(0), Term::lam(Term::var(0), Term::var(0)));
        let expected = Term::pi(Term::universe(0), Term::pi(Term::var(0), Term::var(1)));
        assert_eq!(
            Kernel::run_to_completion(
                empty(),
                KernelRequest::Check {
                    context: Context::new(),
                    term,
                    expected,
                }
            ),
            Ok(KernelResult::Checked)
        );
    }

    #[test]
    fn wrong_certificate_is_rejected() {
        let proposition = Term::pi(Term::universe(0), Term::var(0));
        let wrong = Term::lam(Term::universe(0), Term::universe(0));
        let certificate = Certificate::new(&empty(), Context::new(), proposition, wrong);
        assert!(
            Kernel::run_to_completion(empty(), KernelRequest::Certificate { certificate }).is_err()
        );
    }

    #[test]
    fn transparent_definitions_reduce() {
        let mut builder = TheoryBuilder::new("test", "1");
        let identity_type = Term::pi(Term::universe(0), Term::pi(Term::var(0), Term::var(1)));
        let identity = Term::lam(Term::universe(0), Term::lam(Term::var(0), Term::var(0)));
        builder
            .define("id", identity_type, identity, Provenance::new("test"))
            .unwrap();
        builder
            .axiom("Nat", Term::universe(0), Provenance::new("test"))
            .unwrap();
        builder
            .axiom("zero", Term::constant("Nat"), Provenance::new("test"))
            .unwrap();
        let theory = builder.finish();
        let application = Term::apply_many(
            Term::constant("id"),
            [Term::constant("Nat"), Term::constant("zero")],
        );
        assert_eq!(
            Kernel::run_to_completion(
                theory,
                KernelRequest::Check {
                    context: Context::new(),
                    term: application,
                    expected: Term::constant("Nat"),
                }
            ),
            Ok(KernelResult::Checked)
        );
    }

    #[test]
    fn checkpoint_roundtrip_resumes_without_replay() {
        let term = Term::lam(Term::universe(0), Term::lam(Term::var(0), Term::var(0)));
        let expected = Term::pi(Term::universe(0), Term::pi(Term::var(0), Term::var(1)));
        let mut session = Kernel::start(
            empty(),
            KernelRequest::Check {
                context: Context::new(),
                term,
                expected,
            },
        );
        assert!(matches!(
            session.run_slice(3),
            SessionStatus::Running { .. }
        ));
        let steps_before = session.steps();
        let encoded = serde_json::to_vec(&session).unwrap();
        let mut restored: CheckSession = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored.steps(), steps_before);
        let status = restored.run_slice(10_000);
        assert!(matches!(status, SessionStatus::Accepted { .. }));
        assert!(restored.steps() > steps_before);
    }

    #[test]
    fn theory_mismatch_is_explicit() {
        let left = Theory::empty("left", "1");
        let right = Theory::empty("right", "1");
        let certificate = Certificate::new(
            &left,
            Context::new(),
            Term::pi(Term::universe(0), Term::universe(0)),
            Term::lam(Term::universe(0), Term::universe(0)),
        );
        assert!(matches!(
            Kernel::run_to_completion(right, KernelRequest::Certificate { certificate }),
            Err(KernelError::TheoryMismatch { .. })
        ));
    }
}
