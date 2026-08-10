use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Object, Pattern, Substitution, Symbol, match_pattern};

/// Register name in the universal finite-trace machine.
pub type Register = Symbol;

/// One deterministic instruction in the universal certificate machine.
///
/// Constructor trees, projection, pattern matching, equality, mutable registers,
/// a stack, and backward jumps are sufficient to compile ordinary finite proof
/// checkers and general partial recursive computations. Acceptance still requires
/// an explicit finite trace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "instruction", rename_all = "snake_case")]
pub enum UniversalInstruction {
    /// Store a literal reflected object.
    Set { target: Register, value: Object },
    /// Copy one register.
    Copy { target: Register, source: Register },
    /// Construct a node from ordered register values.
    Build {
        target: Register,
        head: Symbol,
        arguments: Vec<Register>,
    },
    /// Select one child from a constructor node.
    Project {
        target: Register,
        source: Register,
        index: usize,
    },
    /// Compare two exact reflected objects and jump.
    BranchEqual {
        left: Register,
        right: Register,
        equal: usize,
        different: usize,
    },
    /// Pattern-match one object, optionally writing selected pattern variables.
    BranchMatch {
        source: Register,
        pattern: Pattern,
        /// Pattern-variable to destination-register bindings.
        bindings: BTreeMap<Symbol, Register>,
        matched: usize,
        unmatched: usize,
    },
    /// Push one register value.
    Push { source: Register },
    /// Pop the stack into a register.
    Pop { target: Register },
    /// Unconditional jump. Backward jumps permit unbounded computation.
    Jump { target: usize },
    /// Halt successfully.
    Accept,
    /// Halt unsuccessfully with exact reflected reason data.
    Reject { reason: Object },
}

/// Exact finite program for a proof checker or computation verifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniversalProgram {
    /// Stable externally assigned identity.
    pub id: Symbol,
    /// Program/ABI version.
    pub version: String,
    /// Initial instruction.
    pub entry: usize,
    /// Complete deterministic instruction sequence.
    pub instructions: Vec<UniversalInstruction>,
}

/// Halt state of the universal machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum UniversalStatus {
    /// Another instruction may execute.
    Running,
    /// Program explicitly accepted.
    Accepted,
    /// Program explicitly rejected.
    Rejected(Object),
}

/// Complete state at one universal-machine transition boundary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniversalState {
    /// Next instruction index.
    pub pc: usize,
    /// Exact finite register file.
    pub registers: BTreeMap<Register, Object>,
    /// Exact finite stack.
    pub stack: Vec<Object>,
    /// Running or terminal state.
    pub status: UniversalStatus,
}

/// Finite execution trace for a universal program.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniversalRun {
    /// Exact target program identity.
    pub program: Symbol,
    /// Initial register contents.
    pub initial_registers: BTreeMap<Register, Object>,
    /// Initial state followed by every claimed successor state.
    pub states: Vec<UniversalState>,
}

/// Universal program or trace rejection.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum UniversalError {
    /// Program contains no executable entry point.
    #[error("universal program entry {entry} is outside {instructions} instructions")]
    InvalidEntry { entry: usize, instructions: usize },
    /// Instruction contains an invalid jump target.
    #[error("universal instruction {instruction} jumps outside the program to {target}")]
    InvalidJump { instruction: usize, target: usize },
    /// Trace names another program.
    #[error("universal run targets program {claimed:?}, loaded {loaded:?}")]
    ProgramMismatch { claimed: Symbol, loaded: Symbol },
    /// Trace has no initial state.
    #[error("universal run contains no states")]
    EmptyTrace,
    /// First trace state is not the canonical initial state.
    #[error("universal run has the wrong initial state")]
    WrongInitialState,
    /// A transition does not equal deterministic execution.
    #[error("universal run has an invalid successor at transition {0}")]
    WrongSuccessor(usize),
    /// Claimed trace continues after a terminal state.
    #[error("universal run continues after termination at transition {0}")]
    ContinuedAfterHalt(usize),
    /// Running state points outside the program.
    #[error("universal state points outside the program at pc {0}")]
    InvalidProgramCounter(usize),
    /// Required register is absent.
    #[error("universal instruction requires absent register {0:?}")]
    MissingRegister(Register),
    /// Projection source is not a node or has no requested child.
    #[error("universal projection from register {register:?} has no child {index}")]
    InvalidProjection { register: Register, index: usize },
    /// Pop executed with an empty stack.
    #[error("universal machine popped an empty stack")]
    EmptyStack,
    /// A requested match binding was not produced by the pattern.
    #[error("universal pattern did not bind variable {0:?}")]
    MissingPatternBinding(Symbol),
    /// Statement and certificate inputs would overwrite one another.
    #[error("universal verifier statement and certificate registers must be distinct")]
    InputRegisterCollision,
    /// Trace did not end in acceptance.
    #[error("universal verifier trace did not end in acceptance")]
    NotAccepted,
    /// Computation trace ended without an explicit terminal state.
    #[error("universal computation trace did not terminate")]
    NotTerminated,
    /// Computation output register differs from the certified value.
    #[error("universal computation output register {0:?} has the wrong value")]
    WrongOutput(Register),
}

impl UniversalProgram {
    /// Validates entry and every explicit jump target.
    pub fn validate(&self) -> Result<(), UniversalError> {
        if self.entry >= self.instructions.len() {
            return Err(UniversalError::InvalidEntry {
                entry: self.entry,
                instructions: self.instructions.len(),
            });
        }
        for (position, instruction) in self.instructions.iter().enumerate() {
            match instruction {
                UniversalInstruction::BranchEqual {
                    equal, different, ..
                }
                | UniversalInstruction::BranchMatch {
                    matched: equal,
                    unmatched: different,
                    ..
                } => {
                    validate_jump(self.instructions.len(), position, *equal)?;
                    validate_jump(self.instructions.len(), position, *different)?;
                }
                UniversalInstruction::Jump { target } => {
                    validate_jump(self.instructions.len(), position, *target)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Canonical initial state for a register file.
    #[must_use]
    pub fn initial_state(&self, registers: BTreeMap<Register, Object>) -> UniversalState {
        UniversalState {
            pc: self.entry,
            registers,
            stack: Vec::new(),
            status: UniversalStatus::Running,
        }
    }

    /// Executes exactly one deterministic transition.
    pub fn step(&self, state: &UniversalState) -> Result<UniversalState, UniversalError> {
        if !matches!(&state.status, UniversalStatus::Running) {
            return Ok(state.clone());
        }
        let instruction = self
            .instructions
            .get(state.pc)
            .ok_or(UniversalError::InvalidProgramCounter(state.pc))?;
        let mut next = state.clone();
        let advance = |state: &mut UniversalState| state.pc = state.pc.saturating_add(1);
        match instruction {
            UniversalInstruction::Set { target, value } => {
                next.registers.insert(target.clone(), value.clone());
                advance(&mut next);
            }
            UniversalInstruction::Copy { target, source } => {
                let value = required_register(state, source)?.clone();
                next.registers.insert(target.clone(), value);
                advance(&mut next);
            }
            UniversalInstruction::Build {
                target,
                head,
                arguments,
            } => {
                let children = arguments
                    .iter()
                    .map(|register| required_register(state, register).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                next.registers.insert(
                    target.clone(),
                    Object::Node {
                        head: head.clone(),
                        children,
                    },
                );
                advance(&mut next);
            }
            UniversalInstruction::Project {
                target,
                source,
                index,
            } => {
                let Object::Node { children, .. } = required_register(state, source)? else {
                    return Err(UniversalError::InvalidProjection {
                        register: source.clone(),
                        index: *index,
                    });
                };
                let value =
                    children
                        .get(*index)
                        .ok_or_else(|| UniversalError::InvalidProjection {
                            register: source.clone(),
                            index: *index,
                        })?;
                next.registers.insert(target.clone(), value.clone());
                advance(&mut next);
            }
            UniversalInstruction::BranchEqual {
                left,
                right,
                equal,
                different,
            } => {
                next.pc = if required_register(state, left)? == required_register(state, right)? {
                    *equal
                } else {
                    *different
                };
            }
            UniversalInstruction::BranchMatch {
                source,
                pattern,
                bindings,
                matched,
                unmatched,
            } => {
                let value = required_register(state, source)?;
                let mut substitution = Substitution::new();
                if match_pattern(pattern, value, &mut substitution) {
                    for (variable, register) in bindings {
                        let bound = substitution.get(variable).ok_or_else(|| {
                            UniversalError::MissingPatternBinding(variable.clone())
                        })?;
                        next.registers.insert(register.clone(), bound.clone());
                    }
                    next.pc = *matched;
                } else {
                    next.pc = *unmatched;
                }
            }
            UniversalInstruction::Push { source } => {
                next.stack.push(required_register(state, source)?.clone());
                advance(&mut next);
            }
            UniversalInstruction::Pop { target } => {
                let value = next.stack.pop().ok_or(UniversalError::EmptyStack)?;
                next.registers.insert(target.clone(), value);
                advance(&mut next);
            }
            UniversalInstruction::Jump { target } => next.pc = *target,
            UniversalInstruction::Accept => next.status = UniversalStatus::Accepted,
            UniversalInstruction::Reject { reason } => {
                next.status = UniversalStatus::Rejected(reason.clone());
            }
        }
        Ok(next)
    }

    /// Checks an explicit finite run and returns its final state.
    pub fn check_run<'a>(
        &self,
        run: &'a UniversalRun,
    ) -> Result<&'a UniversalState, UniversalError> {
        self.validate()?;
        if run.program != self.id {
            return Err(UniversalError::ProgramMismatch {
                claimed: run.program.clone(),
                loaded: self.id.clone(),
            });
        }
        let first = run.states.first().ok_or(UniversalError::EmptyTrace)?;
        if first != &self.initial_state(run.initial_registers.clone()) {
            return Err(UniversalError::WrongInitialState);
        }
        for (position, pair) in run.states.windows(2).enumerate() {
            if !matches!(&pair[0].status, UniversalStatus::Running) {
                return Err(UniversalError::ContinuedAfterHalt(position));
            }
            let expected = self.step(&pair[0])?;
            if expected != pair[1] {
                return Err(UniversalError::WrongSuccessor(position));
            }
        }
        run.states.last().ok_or(UniversalError::EmptyTrace)
    }
}

/// Standard contract turning a universal program into a proof-certificate checker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniversalVerifier {
    /// Exact verifier program.
    pub program: UniversalProgram,
    /// Register receiving the encoded statement.
    pub statement_register: Register,
    /// Register receiving the encoded certificate.
    pub certificate_register: Register,
}

/// One certificate accepted by a universal verifier.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UniversalCertificate {
    /// Encoded proposition or judgment.
    pub statement: Object,
    /// Encoded proof certificate.
    pub certificate: Object,
    /// Complete finite accepting trace.
    pub run: UniversalRun,
}

impl UniversalVerifier {
    /// Validates the program and reserved input-register contract.
    pub fn validate(&self) -> Result<(), UniversalError> {
        self.program.validate()?;
        if self.statement_register == self.certificate_register {
            Err(UniversalError::InputRegisterCollision)
        } else {
            Ok(())
        }
    }

    /// Checks initialization, every deterministic transition, and final acceptance.
    pub fn check(&self, certificate: &UniversalCertificate) -> Result<(), UniversalError> {
        self.validate()?;
        let mut expected_initial = BTreeMap::new();
        expected_initial.insert(
            self.statement_register.clone(),
            certificate.statement.clone(),
        );
        expected_initial.insert(
            self.certificate_register.clone(),
            certificate.certificate.clone(),
        );
        if certificate.run.initial_registers != expected_initial {
            return Err(UniversalError::WrongInitialState);
        }
        let final_state = self.program.check_run(&certificate.run)?;
        if matches!(&final_state.status, UniversalStatus::Accepted) {
            Ok(())
        } else {
            Err(UniversalError::NotAccepted)
        }
    }
}

/// Certified result of an arbitrary finite-trace computation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComputationCertificate {
    /// Exact program.
    pub program: UniversalProgram,
    /// Initial input registers.
    pub inputs: BTreeMap<Register, Object>,
    /// Claimed terminal register values.
    pub outputs: BTreeMap<Register, Object>,
    /// Complete finite execution trace.
    pub run: UniversalRun,
}

impl ComputationCertificate {
    /// Checks the exact run and all claimed terminal outputs.
    pub fn check(&self) -> Result<(), UniversalError> {
        if self.run.initial_registers != self.inputs {
            return Err(UniversalError::WrongInitialState);
        }
        let final_state = self.program.check_run(&self.run)?;
        match &final_state.status {
            UniversalStatus::Accepted => {}
            UniversalStatus::Running => return Err(UniversalError::NotTerminated),
            UniversalStatus::Rejected(_) => return Err(UniversalError::NotAccepted),
        }
        for (register, expected) in &self.outputs {
            if final_state.registers.get(register) != Some(expected) {
                return Err(UniversalError::WrongOutput(register.clone()));
            }
        }
        Ok(())
    }
}

/// Design-level universality witness for one compiled external checker.
///
/// The encoded program and the two total encoders are retained as exact data. The
/// finite acceptance trace is then checked by [`UniversalVerifier::check`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckerCompilation {
    /// Identity of the source proof system/checker.
    pub source_system: Symbol,
    /// Version of the source checker semantics.
    pub source_version: String,
    /// Compiled universal verifier.
    pub verifier: UniversalVerifier,
    /// Exact description of the statement encoder.
    pub statement_encoder: Object,
    /// Exact description of the certificate encoder.
    pub certificate_encoder: Object,
    /// Optional compiler correctness certificate in a surrounding formal theory.
    pub correctness_claim: Option<artist_kernel::Certificate>,
}

impl CheckerCompilation {
    /// Ensures the compiled universal program itself is structurally valid.
    pub fn validate(&self) -> Result<(), UniversalError> {
        self.verifier.validate()
    }
}

fn validate_jump(
    instruction_count: usize,
    instruction: usize,
    target: usize,
) -> Result<(), UniversalError> {
    if target < instruction_count {
        Ok(())
    } else {
        Err(UniversalError::InvalidJump {
            instruction,
            target,
        })
    }
}

fn required_register<'a>(
    state: &'a UniversalState,
    register: &Register,
) -> Result<&'a Object, UniversalError> {
    state
        .registers
        .get(register)
        .ok_or_else(|| UniversalError::MissingRegister(register.clone()))
}

/// Returns every register read by an instruction. Useful for frontend diagnostics.
#[must_use]
pub fn instruction_inputs(instruction: &UniversalInstruction) -> BTreeSet<Register> {
    match instruction {
        UniversalInstruction::Set { .. }
        | UniversalInstruction::Jump { .. }
        | UniversalInstruction::Accept
        | UniversalInstruction::Reject { .. }
        | UniversalInstruction::Pop { .. } => BTreeSet::new(),
        UniversalInstruction::Copy { source, .. }
        | UniversalInstruction::Project { source, .. }
        | UniversalInstruction::BranchMatch { source, .. }
        | UniversalInstruction::Push { source } => BTreeSet::from([source.clone()]),
        UniversalInstruction::Build { arguments, .. } => arguments.iter().cloned().collect(),
        UniversalInstruction::BranchEqual { left, right, .. } => {
            BTreeSet::from([left.clone(), right.clone()])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted_copy() -> (UniversalProgram, UniversalRun) {
        let program = UniversalProgram {
            id: Symbol::from("copy"),
            version: "1".into(),
            entry: 0,
            instructions: vec![
                UniversalInstruction::Copy {
                    target: Symbol::from("output"),
                    source: Symbol::from("input"),
                },
                UniversalInstruction::Accept,
            ],
        };
        let value = Object::atom("value");
        let initial_registers = BTreeMap::from([(Symbol::from("input"), value.clone())]);
        let output_registers = BTreeMap::from([
            (Symbol::from("input"), value.clone()),
            (Symbol::from("output"), value),
        ]);
        let run = UniversalRun {
            program: program.id.clone(),
            initial_registers: initial_registers.clone(),
            states: vec![
                UniversalState {
                    pc: 0,
                    registers: initial_registers,
                    stack: Vec::new(),
                    status: UniversalStatus::Running,
                },
                UniversalState {
                    pc: 1,
                    registers: output_registers.clone(),
                    stack: Vec::new(),
                    status: UniversalStatus::Running,
                },
                UniversalState {
                    pc: 1,
                    registers: output_registers,
                    stack: Vec::new(),
                    status: UniversalStatus::Accepted,
                },
            ],
        };
        (program, run)
    }

    #[test]
    fn complete_accepting_trace_is_checked() {
        let (program, run) = accepted_copy();
        assert!(matches!(
            &program.check_run(&run).unwrap().status,
            UniversalStatus::Accepted
        ));
    }

    #[test]
    fn computation_requires_acceptance_and_exact_outputs() {
        let (program, run) = accepted_copy();
        let value = Object::atom("value");
        let certificate = ComputationCertificate {
            program,
            inputs: BTreeMap::from([(Symbol::from("input"), value.clone())]),
            outputs: BTreeMap::from([(Symbol::from("output"), value)]),
            run,
        };
        assert_eq!(certificate.check(), Ok(()));
    }

    #[test]
    fn rejected_trace_does_not_certify_a_computation() {
        let reason = Object::atom("no");
        let program = UniversalProgram {
            id: Symbol::from("reject"),
            version: "1".into(),
            entry: 0,
            instructions: vec![UniversalInstruction::Reject {
                reason: reason.clone(),
            }],
        };
        let run = UniversalRun {
            program: program.id.clone(),
            initial_registers: BTreeMap::new(),
            states: vec![
                UniversalState {
                    pc: 0,
                    registers: BTreeMap::new(),
                    stack: Vec::new(),
                    status: UniversalStatus::Running,
                },
                UniversalState {
                    pc: 0,
                    registers: BTreeMap::new(),
                    stack: Vec::new(),
                    status: UniversalStatus::Rejected(reason),
                },
            ],
        };
        let certificate = ComputationCertificate {
            program,
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            run,
        };
        assert_eq!(certificate.check(), Err(UniversalError::NotAccepted));
    }
}
