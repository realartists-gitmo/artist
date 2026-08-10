use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Context;
use crate::machine::{Kernel, KernelError, KernelRequest, KernelResult};
use crate::term::{Name, Term, instantiate, write_string, write_terms};

/// Content identity of a complete versioned theory.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TheoryId(pub String);

impl fmt::Display for TheoryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Human- and machine-readable origin of a declaration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Authority, package, user, sensor, solver, or import that supplied it.
    pub source: String,
    /// Optional stable external reference.
    pub external_ref: Option<String>,
    /// Free-form note. It is disclosed but has no logical force.
    pub note: Option<String>,
}

impl Provenance {
    /// Creates minimal provenance.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            external_ref: None,
            note: None,
        }
    }
}

/// Whether a declaration body participates in definitional equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transparency {
    /// The body may be unfolded by the kernel.
    Transparent,
    /// The proof/body is checked but not unfolded.
    Opaque,
}

/// Trust status of a declaration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclarationKind {
    /// Kernel-checked transparent definition.
    Definition,
    /// Kernel-checked opaque theorem.
    Theorem,
    /// Explicitly trusted assumption.
    Axiom,
    /// Explicitly trusted result supplied by an external authority or computation.
    Oracle,
    /// Type former generated from a checked inductive-family declaration.
    InductiveType,
    /// Constructor generated from a checked inductive-family declaration.
    Constructor,
}

/// One declaration in an append-only theory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    /// Qualified name.
    pub name: Name,
    /// Declared type.
    pub ty: Term,
    /// Checked body for definitions and theorems.
    pub body: Option<Term>,
    /// Trust status.
    pub kind: DeclarationKind,
    /// Definitional transparency.
    pub transparency: Transparency,
    /// Exact origin.
    pub provenance: Provenance,
}

/// One constructor field. Recursive occurrences are represented by constructors
/// whose shape guarantees strict positivity by construction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum ConstructorField {
    /// An ordinary field whose type contains no occurrence of the family being defined.
    Plain { ty: Term },
    /// A direct recursive occurrence at the supplied family indices.
    Recursive { indices: Vec<Term> },
    /// A strictly-positive function field `(domain -> Family indices)`.
    RecursiveFunction { domain: Term, indices: Vec<Term> },
}

/// Checked constructor of an inductive family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InductiveConstructor {
    /// Globally unique constructor name.
    pub name: Name,
    /// Fields in dependency order. Each field is scoped over parameters and prior fields.
    pub fields: Vec<ConstructorField>,
    /// Result family indices, scoped over parameters and every constructor field.
    pub result_indices: Vec<Term>,
}

/// Native strictly-positive polynomial inductive family.
///
/// Parameter types are scoped over prior parameters. Index types are scoped over all
/// parameters and prior indices. Constructor fields are scoped over all parameters and
/// prior fields. Direct and function recursive fields are the only way the family may
/// occur, which makes negative occurrences unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InductiveDeclaration {
    /// Family/type-former name.
    pub name: Name,
    /// Parameter telescope.
    pub parameters: Vec<Term>,
    /// Index telescope.
    pub indices: Vec<Term>,
    /// Result universe `Type universe`.
    pub universe: u32,
    /// Constructors.
    pub constructors: Vec<InductiveConstructor>,
    /// Exact origin.
    pub provenance: Provenance,
}

impl InductiveDeclaration {
    /// Type of the family constant.
    #[must_use]
    pub fn type_former_type(&self) -> Term {
        Term::pi_many(
            self.parameters.iter().chain(&self.indices).cloned(),
            Term::universe(self.universe),
        )
    }

    /// Number of parameters.
    #[must_use]
    pub fn parameter_count(&self) -> usize {
        self.parameters.len()
    }

    /// Number of indices.
    #[must_use]
    pub fn index_count(&self) -> usize {
        self.indices.len()
    }

    /// Family application in a context containing `inner` binders after the parameters.
    #[must_use]
    pub fn family_application(&self, inner: usize, indices: Vec<Term>) -> Term {
        let parameters = parameter_variables(self.parameters.len(), inner);
        Term::apply_many(
            Term::constant(self.name.clone()),
            parameters.into_iter().chain(indices),
        )
    }

    /// Generated type of one constructor.
    #[must_use]
    pub fn constructor_type(&self, constructor: &InductiveConstructor) -> Term {
        let mut field_types = Vec::with_capacity(constructor.fields.len());
        for (position, field) in constructor.fields.iter().enumerate() {
            field_types.push(self.field_type(field, position));
        }
        let result =
            self.family_application(constructor.fields.len(), constructor.result_indices.clone());
        Term::pi_many(self.parameters.iter().cloned().chain(field_types), result)
    }

    /// Generated type of a field in its constructor telescope.
    #[must_use]
    pub fn field_type(&self, field: &ConstructorField, previous_fields: usize) -> Term {
        match field {
            ConstructorField::Plain { ty } => ty.clone(),
            ConstructorField::Recursive { indices } => {
                self.family_application(previous_fields, indices.clone())
            }
            ConstructorField::RecursiveFunction { domain, indices } => Term::pi(
                domain.clone(),
                self.family_application(previous_fields + 1, indices.clone()),
            ),
        }
    }
}

/// A theory is an ordered, versioned declaration environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theory {
    /// Stable namespace.
    pub namespace: String,
    /// Package/version label. The content hash is still authoritative.
    pub version: String,
    /// Declarations in dependency order.
    pub declarations: Vec<Declaration>,
    /// Native inductive-family metadata required by elimination and iota reduction.
    #[serde(default)]
    pub inductives: Vec<InductiveDeclaration>,
}

impl Theory {
    /// Creates an empty theory.
    #[must_use]
    pub fn empty(namespace: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            version: version.into(),
            declarations: Vec::new(),
            inductives: Vec::new(),
        }
    }

    /// Computes the canonical content identity.
    #[must_use]
    pub fn id(&self) -> TheoryId {
        let mut bytes = Vec::new();
        write_string(&mut bytes, "artist.theory/1");
        write_string(&mut bytes, &self.namespace);
        write_string(&mut bytes, &self.version);
        bytes.extend_from_slice(&(self.declarations.len() as u64).to_be_bytes());
        for declaration in &self.declarations {
            write_string(&mut bytes, declaration.name.as_str());
            declaration.ty.write_canonical(&mut bytes);
            match &declaration.body {
                Some(body) => {
                    bytes.push(1);
                    body.write_canonical(&mut bytes);
                }
                None => bytes.push(0),
            }
            bytes.push(match declaration.kind {
                DeclarationKind::Definition => 0,
                DeclarationKind::Theorem => 1,
                DeclarationKind::Axiom => 2,
                DeclarationKind::Oracle => 3,
                DeclarationKind::InductiveType => 4,
                DeclarationKind::Constructor => 5,
            });
            bytes.push(match declaration.transparency {
                Transparency::Transparent => 0,
                Transparency::Opaque => 1,
            });
            write_provenance(&mut bytes, &declaration.provenance);
        }
        bytes.extend_from_slice(&(self.inductives.len() as u64).to_be_bytes());
        for inductive in &self.inductives {
            write_string(&mut bytes, inductive.name.as_str());
            write_terms(&mut bytes, &inductive.parameters);
            write_terms(&mut bytes, &inductive.indices);
            bytes.extend_from_slice(&inductive.universe.to_be_bytes());
            bytes.extend_from_slice(&(inductive.constructors.len() as u64).to_be_bytes());
            for constructor in &inductive.constructors {
                write_string(&mut bytes, constructor.name.as_str());
                bytes.extend_from_slice(&(constructor.fields.len() as u64).to_be_bytes());
                for field in &constructor.fields {
                    match field {
                        ConstructorField::Plain { ty } => {
                            bytes.push(0);
                            ty.write_canonical(&mut bytes);
                        }
                        ConstructorField::Recursive { indices } => {
                            bytes.push(1);
                            write_terms(&mut bytes, indices);
                        }
                        ConstructorField::RecursiveFunction { domain, indices } => {
                            bytes.push(2);
                            domain.write_canonical(&mut bytes);
                            write_terms(&mut bytes, indices);
                        }
                    }
                }
                write_terms(&mut bytes, &constructor.result_indices);
            }
            write_provenance(&mut bytes, &inductive.provenance);
        }
        TheoryId(blake3::hash(&bytes).to_hex().to_string())
    }

    /// Finds a declaration.
    #[must_use]
    pub fn declaration(&self, name: &Name) -> Option<&Declaration> {
        self.declarations.iter().find(|entry| &entry.name == name)
    }

    /// Finds an inductive family.
    #[must_use]
    pub fn inductive(&self, name: &Name) -> Option<&InductiveDeclaration> {
        self.inductives.iter().find(|entry| &entry.name == name)
    }

    /// Finds a constructor and its owning family/index.
    #[must_use]
    pub fn constructor(
        &self,
        name: &Name,
    ) -> Option<(&InductiveDeclaration, usize, &InductiveConstructor)> {
        self.inductives.iter().find_map(|inductive| {
            inductive
                .constructors
                .iter()
                .enumerate()
                .find(|(_, constructor)| &constructor.name == name)
                .map(|(index, constructor)| (inductive, index, constructor))
        })
    }

    /// Reconstructs this snapshot through the checked append-only builder.
    ///
    /// This is required for directly constructed and deserialized theories: public
    /// data representation must not bypass type checking, positivity, declaration
    /// ordering, or the transparency discipline.
    pub fn validate(&self) -> Result<(), TheoryError> {
        let mut builder = TheoryBuilder::new(self.namespace.clone(), self.version.clone());
        let mut used_inductives = BTreeSet::new();
        let mut position = 0usize;
        while position < self.declarations.len() {
            let declaration = &self.declarations[position];
            match declaration.kind {
                DeclarationKind::Axiom => {
                    require_declaration_shape(declaration, false, Transparency::Opaque)?;
                    builder.axiom(
                        declaration.name.clone(),
                        declaration.ty.clone(),
                        declaration.provenance.clone(),
                    )?;
                    position += 1;
                }
                DeclarationKind::Oracle => {
                    require_declaration_shape(declaration, false, Transparency::Opaque)?;
                    builder.oracle(
                        declaration.name.clone(),
                        declaration.ty.clone(),
                        declaration.provenance.clone(),
                    )?;
                    position += 1;
                }
                DeclarationKind::Definition => {
                    require_declaration_shape(declaration, true, Transparency::Transparent)?;
                    builder.define(
                        declaration.name.clone(),
                        declaration.ty.clone(),
                        declaration.body.clone().ok_or_else(|| {
                            TheoryError::InvalidDeclarationShape {
                                name: declaration.name.clone(),
                                message: "checked declaration has no body".to_owned(),
                            }
                        })?,
                        declaration.provenance.clone(),
                    )?;
                    position += 1;
                }
                DeclarationKind::Theorem => {
                    require_declaration_shape(declaration, true, Transparency::Opaque)?;
                    builder.theorem(
                        declaration.name.clone(),
                        declaration.ty.clone(),
                        declaration.body.clone().ok_or_else(|| {
                            TheoryError::InvalidDeclarationShape {
                                name: declaration.name.clone(),
                                message: "checked declaration has no body".to_owned(),
                            }
                        })?,
                        declaration.provenance.clone(),
                    )?;
                    position += 1;
                }
                DeclarationKind::InductiveType => {
                    let inductive = self.inductive(&declaration.name).ok_or_else(|| {
                        TheoryError::MissingInductiveMetadata(declaration.name.clone())
                    })?;
                    if !used_inductives.insert(inductive.name.clone()) {
                        return Err(TheoryError::Duplicate(inductive.name.clone()));
                    }
                    let before = builder.snapshot().declarations.len();
                    builder.inductive(inductive.clone())?;
                    let snapshot = builder.snapshot();
                    let generated = &snapshot.declarations[before..];
                    let end = position.saturating_add(generated.len());
                    if self.declarations.get(position..end) != Some(generated) {
                        return Err(TheoryError::SnapshotMismatch);
                    }
                    position = end;
                }
                DeclarationKind::Constructor => {
                    return Err(TheoryError::UnexpectedConstructor(declaration.name.clone()));
                }
            }
        }
        let rebuilt = builder.finish();
        if used_inductives.len() != self.inductives.len() || &rebuilt != self {
            return Err(TheoryError::SnapshotMismatch);
        }
        Ok(())
    }

    /// Returns whether this theory is exact append-only growth of `parent`.
    /// Existing declarations and inductive metadata must remain byte-for-byte equal
    /// and in the same order. The namespace is stable; the human version may change.
    #[must_use]
    pub fn is_extension_of(&self, parent: &Self) -> bool {
        self.namespace == parent.namespace
            && self.declarations.starts_with(&parent.declarations)
            && self.inductives.starts_with(&parent.inductives)
    }

    /// Creates a name-indexed view.
    #[must_use]
    pub fn index(&self) -> BTreeMap<Name, &Declaration> {
        self.declarations
            .iter()
            .map(|declaration| (declaration.name.clone(), declaration))
            .collect()
    }

    /// Returns the transparent body of a definition.
    #[must_use]
    pub fn unfold(&self, name: &Name) -> Option<&Term> {
        let declaration = self.declaration(name)?;
        (declaration.transparency == Transparency::Transparent)
            .then_some(declaration.body.as_ref())
            .flatten()
    }
}

/// Errors while constructing a checked theory.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum TheoryError {
    /// A declaration name was reused.
    #[error("duplicate declaration {0}")]
    Duplicate(Name),
    /// A declared type is not itself a type.
    #[error("declaration {name} has an invalid type: {source}")]
    InvalidType { name: Name, source: KernelError },
    /// A body does not inhabit its declared type.
    #[error("declaration {name} has an invalid body: {source}")]
    InvalidBody { name: Name, source: KernelError },
    /// An internal kernel request produced the wrong result shape.
    #[error("kernel returned an unexpected result while adding {0}")]
    UnexpectedKernelResult(Name),
    /// A field violates the declared strict-positivity shape.
    #[error("inductive family {family}, constructor {constructor}, field {field}: {message}")]
    InvalidInductiveField {
        family: Name,
        constructor: Name,
        field: usize,
        message: String,
    },
    /// Constructor result has the wrong number or types of indices.
    #[error(
        "inductive family {family}, constructor {constructor}: invalid result indices: {message}"
    )]
    InvalidConstructorResult {
        family: Name,
        constructor: Name,
        message: String,
    },
    /// A binder exceeds the family universe.
    #[error("inductive family {family}: binder lives in Type{found}, above Type{declared}")]
    LargeInductiveBinder {
        family: Name,
        found: u32,
        declared: u32,
    },
    /// A serialized declaration's body/transparency does not match its kind.
    #[error("declaration {name} has an invalid serialized shape: {message}")]
    InvalidDeclarationShape { name: Name, message: String },
    /// An inductive type declaration has no matching metadata.
    #[error("inductive declaration {0} has no metadata")]
    MissingInductiveMetadata(Name),
    /// A constructor appeared outside the generated block of its family.
    #[error("constructor declaration {0} is not owned by the preceding inductive family")]
    UnexpectedConstructor(Name),
    /// Rebuilding a serialized snapshot changed its checked content.
    #[error("theory snapshot does not match checked append-only reconstruction")]
    SnapshotMismatch,
}

/// Append-only checked theory construction.
#[derive(Clone, Debug)]
pub struct TheoryBuilder {
    theory: Theory,
}

impl TheoryBuilder {
    /// Starts an empty theory.
    #[must_use]
    pub fn new(namespace: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            theory: Theory::empty(namespace, version),
        }
    }

    /// Continues from an existing checked theory.
    pub fn from_theory(theory: Theory) -> Result<Self, TheoryError> {
        theory.validate()?;
        Ok(Self { theory })
    }

    /// Returns the current snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Theory {
        self.theory.clone()
    }

    /// Adds an explicit axiom.
    pub fn axiom(
        &mut self,
        name: impl Into<Name>,
        ty: Term,
        provenance: Provenance,
    ) -> Result<(), TheoryError> {
        self.add_assumption(name.into(), ty, provenance, DeclarationKind::Axiom)
    }

    /// Adds an explicit external-oracle assumption.
    pub fn oracle(
        &mut self,
        name: impl Into<Name>,
        ty: Term,
        provenance: Provenance,
    ) -> Result<(), TheoryError> {
        self.add_assumption(name.into(), ty, provenance, DeclarationKind::Oracle)
    }

    /// Adds a transparent checked definition.
    pub fn define(
        &mut self,
        name: impl Into<Name>,
        ty: Term,
        body: Term,
        provenance: Provenance,
    ) -> Result<(), TheoryError> {
        self.add_checked(
            name.into(),
            ty,
            body,
            provenance,
            DeclarationKind::Definition,
            Transparency::Transparent,
        )
    }

    /// Adds an opaque checked theorem.
    pub fn theorem(
        &mut self,
        name: impl Into<Name>,
        proposition: Term,
        proof: Term,
        provenance: Provenance,
    ) -> Result<(), TheoryError> {
        self.add_checked(
            name.into(),
            proposition,
            proof,
            provenance,
            DeclarationKind::Theorem,
            Transparency::Opaque,
        )
    }

    /// Adds a native strictly-positive inductive family and generated constants.
    pub fn inductive(&mut self, declaration: InductiveDeclaration) -> Result<(), TheoryError> {
        self.ensure_fresh(&declaration.name)?;
        let mut names = BTreeSet::new();
        for constructor in &declaration.constructors {
            self.ensure_fresh(&constructor.name)?;
            if !names.insert(constructor.name.clone()) || constructor.name == declaration.name {
                return Err(TheoryError::Duplicate(constructor.name.clone()));
            }
        }
        // Install only the family signature while validating constructor fields and
        // result indices. Indexed recursive fields may occur in the validation
        // context, so the family must already be known, but no constructor is
        // exposed until every declaration has checked successfully.
        self.theory.declarations.push(Declaration {
            name: declaration.name.clone(),
            ty: declaration.type_former_type(),
            body: None,
            kind: DeclarationKind::InductiveType,
            transparency: Transparency::Opaque,
            provenance: declaration.provenance.clone(),
        });

        let validation = (|| {
            self.validate_inductive(&declaration)?;
            declaration
                .constructors
                .iter()
                .map(|constructor| {
                    let ty = declaration.constructor_type(constructor);
                    self.validate_type(&constructor.name, &ty)?;
                    Ok((constructor, ty))
                })
                .collect::<Result<Vec<_>, TheoryError>>()
        })();

        let constructor_types = match validation {
            Ok(types) => types,
            Err(error) => {
                self.theory.declarations.pop();
                return Err(error);
            }
        };

        for (constructor, ty) in constructor_types {
            self.theory.declarations.push(Declaration {
                name: constructor.name.clone(),
                ty,
                body: None,
                kind: DeclarationKind::Constructor,
                transparency: Transparency::Opaque,
                provenance: declaration.provenance.clone(),
            });
        }
        self.theory.inductives.push(declaration);
        Ok(())
    }

    /// Finishes construction.
    #[must_use]
    pub fn finish(self) -> Theory {
        self.theory
    }

    fn ensure_fresh(&self, name: &Name) -> Result<(), TheoryError> {
        if self.theory.declaration(name).is_some() || self.theory.inductive(name).is_some() {
            Err(TheoryError::Duplicate(name.clone()))
        } else {
            Ok(())
        }
    }

    fn validate_type(&self, name: &Name, ty: &Term) -> Result<(), TheoryError> {
        match Kernel::run_to_completion_assuming_valid_theory(
            self.theory.clone(),
            KernelRequest::EnsureUniverse {
                context: Context::new(),
                term: ty.clone(),
            },
        ) {
            Ok(KernelResult::Universe(_)) => Ok(()),
            Ok(_) => Err(TheoryError::UnexpectedKernelResult(name.clone())),
            Err(source) => Err(TheoryError::InvalidType {
                name: name.clone(),
                source,
            }),
        }
    }

    fn ensure_binder_level(
        &self,
        family: &Name,
        context: &Context,
        term: &Term,
        declared: u32,
    ) -> Result<(), TheoryError> {
        match Kernel::run_to_completion_assuming_valid_theory(
            self.theory.clone(),
            KernelRequest::EnsureUniverse {
                context: context.clone(),
                term: term.clone(),
            },
        ) {
            Ok(KernelResult::Universe(found)) if found <= declared.saturating_add(1) => Ok(()),
            Ok(KernelResult::Universe(found)) => Err(TheoryError::LargeInductiveBinder {
                family: family.clone(),
                found,
                declared,
            }),
            Ok(_) => Err(TheoryError::UnexpectedKernelResult(family.clone())),
            Err(source) => Err(TheoryError::InvalidType {
                name: family.clone(),
                source,
            }),
        }
    }

    fn validate_inductive(&self, declaration: &InductiveDeclaration) -> Result<(), TheoryError> {
        let mut parameter_context = Context::new();
        for parameter in &declaration.parameters {
            self.ensure_binder_level(
                &declaration.name,
                &parameter_context,
                parameter,
                declaration.universe,
            )?;
            parameter_context = parameter_context.extend(parameter.clone());
        }

        let mut index_context = parameter_context.clone();
        for index in &declaration.indices {
            self.ensure_binder_level(
                &declaration.name,
                &index_context,
                index,
                declaration.universe,
            )?;
            index_context = index_context.extend(index.clone());
        }

        for constructor in &declaration.constructors {
            let mut context = parameter_context.clone();
            for (field_index, field) in constructor.fields.iter().enumerate() {
                let field_type = declaration.field_type(field, field_index);
                match field {
                    ConstructorField::Plain { ty } => {
                        if ty.constants().contains(&declaration.name) {
                            return Err(TheoryError::InvalidInductiveField {
                                family: declaration.name.clone(),
                                constructor: constructor.name.clone(),
                                field: field_index,
                                message:
                                    "recursive occurrences must use a positive recursive field"
                                        .to_owned(),
                            });
                        }
                        self.ensure_binder_level(
                            &declaration.name,
                            &context,
                            ty,
                            declaration.universe,
                        )?;
                    }
                    ConstructorField::Recursive { indices } => {
                        self.validate_indices(
                            declaration,
                            &context,
                            field_index,
                            constructor,
                            indices,
                        )?;
                    }
                    ConstructorField::RecursiveFunction { domain, indices } => {
                        if domain.constants().contains(&declaration.name) {
                            return Err(TheoryError::InvalidInductiveField {
                                family: declaration.name.clone(),
                                constructor: constructor.name.clone(),
                                field: field_index,
                                message: "recursive family occurs in a function domain".to_owned(),
                            });
                        }
                        self.ensure_binder_level(
                            &declaration.name,
                            &context,
                            domain,
                            declaration.universe,
                        )?;
                        self.validate_indices(
                            declaration,
                            &context.extend(domain.clone()),
                            field_index,
                            constructor,
                            indices,
                        )?;
                    }
                }
                context = context.extend(field_type);
            }
            self.validate_indices(
                declaration,
                &context,
                constructor.fields.len(),
                constructor,
                &constructor.result_indices,
            )
            .map_err(|error| match error {
                TheoryError::InvalidInductiveField { message, .. } => {
                    TheoryError::InvalidConstructorResult {
                        family: declaration.name.clone(),
                        constructor: constructor.name.clone(),
                        message,
                    }
                }
                other => other,
            })?;
        }
        Ok(())
    }

    fn validate_indices(
        &self,
        declaration: &InductiveDeclaration,
        context: &Context,
        field: usize,
        constructor: &InductiveConstructor,
        indices: &[Term],
    ) -> Result<(), TheoryError> {
        if indices.len() != declaration.indices.len() {
            return Err(TheoryError::InvalidInductiveField {
                family: declaration.name.clone(),
                constructor: constructor.name.clone(),
                field,
                message: format!(
                    "expected {expected} family indices, found {actual}",
                    expected = declaration.indices.len(),
                    actual = indices.len(),
                ),
            });
        }
        let parameter_values = parameter_variables(
            declaration.parameters.len(),
            context.len().saturating_sub(declaration.parameters.len()),
        );
        let mut witnesses = parameter_values;
        for (index_term, supplied) in declaration.indices.iter().zip(indices) {
            let expected = instantiate(index_term, &witnesses);
            match Kernel::run_to_completion_assuming_valid_theory(
                self.theory.clone(),
                KernelRequest::Check {
                    context: context.clone(),
                    term: supplied.clone(),
                    expected,
                },
            ) {
                Ok(KernelResult::Checked) => witnesses.push(supplied.clone()),
                Ok(_) => {
                    return Err(TheoryError::UnexpectedKernelResult(
                        declaration.name.clone(),
                    ));
                }
                Err(source) => {
                    return Err(TheoryError::InvalidInductiveField {
                        family: declaration.name.clone(),
                        constructor: constructor.name.clone(),
                        field,
                        message: source.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    fn add_assumption(
        &mut self,
        name: Name,
        ty: Term,
        provenance: Provenance,
        kind: DeclarationKind,
    ) -> Result<(), TheoryError> {
        self.ensure_fresh(&name)?;
        self.validate_type(&name, &ty)?;
        self.theory.declarations.push(Declaration {
            name,
            ty,
            body: None,
            kind,
            transparency: Transparency::Opaque,
            provenance,
        });
        Ok(())
    }

    fn add_checked(
        &mut self,
        name: Name,
        ty: Term,
        body: Term,
        provenance: Provenance,
        kind: DeclarationKind,
        transparency: Transparency,
    ) -> Result<(), TheoryError> {
        self.ensure_fresh(&name)?;
        self.validate_type(&name, &ty)?;
        match Kernel::run_to_completion_assuming_valid_theory(
            self.theory.clone(),
            KernelRequest::Check {
                context: Context::new(),
                term: body.clone(),
                expected: ty.clone(),
            },
        ) {
            Ok(KernelResult::Checked) => {}
            Ok(_) => return Err(TheoryError::UnexpectedKernelResult(name)),
            Err(source) => return Err(TheoryError::InvalidBody { name, source }),
        }
        self.theory.declarations.push(Declaration {
            name,
            ty,
            body: Some(body),
            kind,
            transparency,
            provenance,
        });
        Ok(())
    }
}

fn require_declaration_shape(
    declaration: &Declaration,
    body_required: bool,
    transparency: Transparency,
) -> Result<(), TheoryError> {
    if declaration.body.is_some() != body_required {
        return Err(TheoryError::InvalidDeclarationShape {
            name: declaration.name.clone(),
            message: if body_required {
                "checked declaration has no body".to_owned()
            } else {
                "assumption unexpectedly has a body".to_owned()
            },
        });
    }
    if declaration.transparency != transparency {
        return Err(TheoryError::InvalidDeclarationShape {
            name: declaration.name.clone(),
            message: "transparency does not match declaration kind".to_owned(),
        });
    }
    Ok(())
}

/// Variables referring to parameters in a context with `inner` newer binders.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn parameter_variables(count: usize, inner: usize) -> Vec<Term> {
    (0..count)
        .map(|position| Term::var((inner + count - position - 1) as u32))
        .collect()
}

fn write_provenance(output: &mut Vec<u8>, provenance: &Provenance) {
    write_string(output, &provenance.source);
    write_optional(output, provenance.external_ref.as_deref());
    write_optional(output, provenance.note.as_deref());
}

fn write_optional(output: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            output.push(1);
            write_string(output, value);
        }
        None => output.push(0),
    }
}
