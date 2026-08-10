use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{GraphError, GraphHash, ObjectGraph, ObjectId, Symbol};

/// Stable identity of one ontology version supplied by the harness.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OntologyId(pub String);

impl fmt::Display for OntologyId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable identity of an ontological type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OntologyTypeId(pub String);

impl From<&str> for OntologyTypeId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for OntologyTypeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable identity of an ontological noun, verb, relation, or function.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OntologySymbolId(pub String);

impl From<&str> for OntologySymbolId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl fmt::Display for OntologySymbolId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Theory-independent ontological type expression.
///
/// The harness uses these expressions to state exact noun and verb types. The
/// kernel later maps them mechanically into its fixed dependent type calculus.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OntologyTypeExpr {
    /// Predicative universe used when an object denotes a type itself.
    Universe { level: u32 },
    /// Universal exact type of quoted graph objects.
    QuotedObject,
    /// Named ontological type.
    Named { id: OntologyTypeId },
    /// Non-dependent function type. Dependent formal refinements remain available
    /// in the kernel theory after this exact ontology boundary.
    Function {
        domain: Box<OntologyTypeExpr>,
        codomain: Box<OntologyTypeExpr>,
    },
}

impl OntologyTypeExpr {
    /// Named type convenience constructor.
    #[must_use]
    pub fn named(id: impl Into<OntologyTypeId>) -> Self {
        Self::Named { id: id.into() }
    }

    /// Function type convenience constructor.
    #[must_use]
    pub fn function(domain: Self, codomain: Self) -> Self {
        Self::Function {
            domain: Box::new(domain),
            codomain: Box::new(codomain),
        }
    }

    /// Collects named type dependencies.
    #[must_use]
    pub fn named_types(&self) -> BTreeSet<OntologyTypeId> {
        let mut result = BTreeSet::new();
        self.collect_named_types(&mut result);
        result
    }

    fn collect_named_types(&self, output: &mut BTreeSet<OntologyTypeId>) {
        match self {
            Self::Universe { .. } | Self::QuotedObject => {}
            Self::Named { id } => {
                output.insert(id.clone());
            }
            Self::Function { domain, codomain } => {
                domain.collect_named_types(output);
                codomain.collect_named_types(output);
            }
        }
    }
}

/// Declared ontological type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyTypeDeclaration {
    /// Stable identity.
    pub id: OntologyTypeId,
    /// Universe containing the formal realization of this type.
    pub universe: u32,
    /// Optional human description with no logical force.
    pub description: Option<String>,
}

/// One named argument of a verb, relation, predicate, or function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologyParameter {
    /// Exact edge role used in the submitted graph.
    pub role: Symbol,
    /// Exact required ontological type.
    pub ty: OntologyTypeExpr,
}

/// Exact declaration of a noun, verb, relation, predicate, or function.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OntologySymbolDeclaration {
    /// Stable identity.
    pub id: OntologySymbolId,
    /// Ordered graph arguments. An empty list denotes a noun/value/type symbol.
    pub parameters: Vec<OntologyParameter>,
    /// Exact result type after every argument is supplied.
    pub result: OntologyTypeExpr,
    /// Optional human description with no logical force.
    pub description: Option<String>,
}

impl OntologySymbolDeclaration {
    /// Full curried type of this declaration.
    #[must_use]
    pub fn full_type(&self) -> OntologyTypeExpr {
        self.parameters
            .iter()
            .rev()
            .fold(self.result.clone(), |codomain, parameter| {
                OntologyTypeExpr::function(parameter.ty.clone(), codomain)
            })
    }
}

/// Complete exact ontology supplied by the harness.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ontology {
    /// Stable namespace.
    pub namespace: String,
    /// Version label. The content hash remains authoritative.
    pub version: String,
    /// The distinguished ontological type whose inhabitants are propositions.
    /// It maps to the kernel's proposition-as-type universe rather than to an
    /// ordinary first-class data type.
    pub proposition_type: OntologyTypeId,
    /// Declared types.
    pub types: BTreeMap<OntologyTypeId, OntologyTypeDeclaration>,
    /// Declared nouns, verbs, relations, predicates, and functions.
    pub symbols: BTreeMap<OntologySymbolId, OntologySymbolDeclaration>,
}

/// Content identity of an ontology.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OntologyHash(pub String);

impl fmt::Display for OntologyHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Content identity of one complete exactly interpreted graph submission.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InterpretedGraphHash(pub String);

impl fmt::Display for InterpretedGraphHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Content-addressed root in one exact interpreted submission.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretedGraphRef {
    /// Complete graph, ontology, and interpretation-map identity.
    pub submission: InterpretedGraphHash,
    /// Structural graph identity retained for graph-level lookup and auditing.
    pub graph: GraphHash,
    /// Exact root object within the interpreted submission.
    pub root: ObjectId,
}

/// Ontology declaration failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum OntologyError {
    /// Distinguished proposition type is absent.
    #[error("ontology proposition type {0} is not declared")]
    MissingPropositionType(OntologyTypeId),
    /// The proposition type must realize `Type 0`, whose own type is `Type 1`.
    #[error("ontology proposition type must be declared in universe 1, found {0}")]
    InvalidPropositionUniverse(u32),
    /// A declared universe has no successor representable by the kernel format.
    #[error("ontology contains overflowing universe level {0}")]
    UniverseOverflow(u32),
    /// Map key and embedded type identity disagree.
    #[error("ontology type key {key} contains declaration {declared}")]
    TypeIdentityMismatch {
        key: OntologyTypeId,
        declared: OntologyTypeId,
    },
    /// Map key and embedded symbol identity disagree.
    #[error("ontology symbol key {key} contains declaration {declared}")]
    SymbolIdentityMismatch {
        key: OntologySymbolId,
        declared: OntologySymbolId,
    },
    /// A declaration references an absent type.
    #[error("ontology symbol {symbol} references unknown type {ty}")]
    UnknownType {
        symbol: OntologySymbolId,
        ty: OntologyTypeId,
    },
    /// Two parameters use the same graph role.
    #[error("ontology symbol {symbol} repeats parameter role {role}")]
    DuplicateParameterRole {
        symbol: OntologySymbolId,
        role: Symbol,
    },
}

impl Ontology {
    /// Validates identity consistency, type references, and operation role uniqueness.
    pub fn validate(&self) -> Result<(), OntologyError> {
        let proposition = self
            .types
            .get(&self.proposition_type)
            .ok_or_else(|| OntologyError::MissingPropositionType(self.proposition_type.clone()))?;
        if proposition.universe != 1 {
            return Err(OntologyError::InvalidPropositionUniverse(
                proposition.universe,
            ));
        }
        for (key, declaration) in &self.types {
            if key != &declaration.id {
                return Err(OntologyError::TypeIdentityMismatch {
                    key: key.clone(),
                    declared: declaration.id.clone(),
                });
            }
            if declaration.universe == u32::MAX {
                return Err(OntologyError::UniverseOverflow(declaration.universe));
            }
        }
        for (key, declaration) in &self.symbols {
            if key != &declaration.id {
                return Err(OntologyError::SymbolIdentityMismatch {
                    key: key.clone(),
                    declared: declaration.id.clone(),
                });
            }
            let mut roles = BTreeSet::new();
            for parameter in &declaration.parameters {
                if !roles.insert(parameter.role.clone()) {
                    return Err(OntologyError::DuplicateParameterRole {
                        symbol: declaration.id.clone(),
                        role: parameter.role.clone(),
                    });
                }
            }
            for ty in declaration.full_type().named_types() {
                if !self.types.contains_key(&ty) {
                    return Err(OntologyError::UnknownType {
                        symbol: declaration.id.clone(),
                        ty,
                    });
                }
            }
            if contains_overflowing_universe(&declaration.full_type()) {
                return Err(OntologyError::UniverseOverflow(u32::MAX));
            }
        }
        Ok(())
    }

    /// Canonical content identity independent of serialization formatting.
    #[must_use]
    pub fn canonical_hash(&self) -> OntologyHash {
        let mut bytes = Vec::new();
        put_str(&mut bytes, "artist.ontology/2");
        put_str(&mut bytes, &self.namespace);
        put_str(&mut bytes, &self.version);
        put_str(&mut bytes, &self.proposition_type.0);
        put_u64(&mut bytes, self.types.len() as u64);
        for (id, declaration) in &self.types {
            put_str(&mut bytes, &id.0);
            bytes.extend_from_slice(&declaration.universe.to_be_bytes());
            put_optional_str(&mut bytes, declaration.description.as_deref());
        }
        put_u64(&mut bytes, self.symbols.len() as u64);
        for (id, declaration) in &self.symbols {
            put_str(&mut bytes, &id.0);
            put_u64(&mut bytes, declaration.parameters.len() as u64);
            for parameter in &declaration.parameters {
                put_str(&mut bytes, parameter.role.as_str());
                put_type_expr(&mut bytes, &parameter.ty);
            }
            put_type_expr(&mut bytes, &declaration.result);
            put_optional_str(&mut bytes, declaration.description.as_deref());
        }
        OntologyHash(blake3::hash(&bytes).to_hex().to_string())
    }

    /// Stable externally visible identity combining namespace, version, and content.
    #[must_use]
    pub fn id(&self) -> OntologyId {
        OntologyId(format!(
            "{}@{}#{}",
            self.namespace,
            self.version,
            self.canonical_hash().0
        ))
    }
}

/// Exact meaning assigned to one graph object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "meaning", rename_all = "snake_case")]
pub enum ObjectMeaning {
    /// The object directly denotes an exact ontology type expression.
    Type { value: OntologyTypeExpr },
    /// The object directly denotes a declared ontology symbol.
    Symbol { id: OntologySymbolId },
    /// The object denotes application of its graph-level operator. `arguments`
    /// gives the exact role order used for curried application. This supports
    /// partial and higher-order applications without requiring the operator to be
    /// a direct ontology symbol.
    Application { arguments: Vec<Symbol> },
    /// The object is exact quoted graph structure. It is semantically available as
    /// syntax/data but is not recursively unfolded as a kernel term.
    Quoted,
}

/// Exact ontology annotation for one graph object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectInterpretation {
    /// Exact ontological type.
    pub ty: OntologyTypeExpr,
    /// Exact denotation mode.
    pub meaning: ObjectMeaning,
}

/// A graph whose every object has exact harness-supplied ontology information.
///
/// This is the normative write boundary. Every valid value is fully represented
/// and fully structurally interpreted. Formal proof elaboration is a later,
/// deterministic operation over this exact content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterpretedGraph {
    /// Complete finite graph, including cycles and quotation.
    pub graph: ObjectGraph,
    /// Exact ontology version.
    pub ontology: Ontology,
    /// Total object interpretation map.
    pub interpretations: BTreeMap<ObjectId, ObjectInterpretation>,
}

/// Failure of the total interpretation contract.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum InterpretationError {
    /// Graph structure is malformed.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// Ontology is malformed.
    #[error(transparent)]
    Ontology(#[from] OntologyError),
    /// A graph object has no exact ontology annotation.
    #[error("graph object {0} has no ontology interpretation")]
    MissingObjectInterpretation(ObjectId),
    /// An annotation exists for an object absent from the graph.
    #[error("ontology interpretation names absent graph object {0}")]
    UnexpectedObjectInterpretation(ObjectId),
    /// An object annotation references an absent ontological type.
    #[error("graph object {object} references unknown ontology type {ty}")]
    UnknownObjectType {
        object: ObjectId,
        ty: OntologyTypeId,
    },
    /// A type-denoting object references an absent ontological type.
    #[error("graph object {object} denotes a type expression containing unknown type {ty}")]
    UnknownDenotedType {
        object: ObjectId,
        ty: OntologyTypeId,
    },
    /// A type-denoting object's annotation is not the universe containing that type.
    #[error("graph object {object} has the wrong universe annotation for its denoted type")]
    TypeUniverseMismatch { object: ObjectId },
    /// A symbol meaning references an absent declaration.
    #[error("graph object {object} denotes unknown ontology symbol {symbol}")]
    UnknownSymbol {
        object: ObjectId,
        symbol: OntologySymbolId,
    },
    /// A directly denoted symbol's declared type differs from its object annotation.
    #[error("graph object {object} has type inconsistent with symbol {symbol}")]
    SymbolTypeMismatch {
        object: ObjectId,
        symbol: OntologySymbolId,
    },
    /// A quoted object's exact annotation must be the universal quoted-object type.
    #[error("quoted graph object {0} is not annotated as quoted-object data")]
    QuotedTypeMismatch(ObjectId),
}

/// Theory-neutral diagnostic for graph application typing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "diagnostic", rename_all = "snake_case")]
pub enum OntologicalTypingDiagnostic {
    /// Application has no graph operator.
    MissingOperator { object: ObjectId },
    /// Application supplies no argument roles.
    EmptyApplication { object: ObjectId },
    /// Application repeats an argument role.
    DuplicateArgumentRole { object: ObjectId, role: Symbol },
    /// A direct ontology symbol is applied using a role order inconsistent with
    /// its declaration.
    SymbolRoleMismatch {
        object: ObjectId,
        symbol: OntologySymbolId,
        position: usize,
        expected: Symbol,
        actual: Symbol,
    },
    /// Another argument was supplied after the operator's function type was exhausted.
    OperatorIsNotFunction { object: ObjectId, role: Symbol },
    /// Required role is absent or has more than one target.
    ArgumentArity {
        object: ObjectId,
        role: Symbol,
        actual: usize,
    },
    /// Graph contains a role not declared by the operation.
    UnexpectedRole { object: ObjectId, role: Symbol },
    /// Argument has the wrong exact ontological type.
    ArgumentType {
        object: ObjectId,
        role: Symbol,
        expected: OntologyTypeExpr,
        actual: OntologyTypeExpr,
    },
    /// Annotated application result differs from the operation result.
    ResultType {
        object: ObjectId,
        expected: OntologyTypeExpr,
        actual: OntologyTypeExpr,
    },
}

impl InterpretedGraph {
    /// Validates that interpretation is total and every direct symbol is exact.
    /// Ill-typed applications remain representable and are reported separately by
    /// [`Self::typing_diagnostics`].
    pub fn validate(&self) -> Result<(), InterpretationError> {
        self.graph.validate()?;
        self.ontology.validate()?;
        for object in self.graph.nodes.keys() {
            let interpretation = self
                .interpretations
                .get(object)
                .ok_or_else(|| InterpretationError::MissingObjectInterpretation(object.clone()))?;
            for ty in interpretation.ty.named_types() {
                if !self.ontology.types.contains_key(&ty) {
                    return Err(InterpretationError::UnknownObjectType {
                        object: object.clone(),
                        ty,
                    });
                }
            }
            if contains_overflowing_universe(&interpretation.ty) {
                return Err(InterpretationError::Ontology(
                    OntologyError::UniverseOverflow(u32::MAX),
                ));
            }
            match &interpretation.meaning {
                ObjectMeaning::Type { value } => {
                    for ty in value.named_types() {
                        if !self.ontology.types.contains_key(&ty) {
                            return Err(InterpretationError::UnknownDenotedType {
                                object: object.clone(),
                                ty,
                            });
                        }
                    }
                    if contains_overflowing_universe(value) {
                        return Err(InterpretationError::Ontology(
                            OntologyError::UniverseOverflow(u32::MAX),
                        ));
                    }
                    let expected = OntologyTypeExpr::Universe {
                        level: type_sort_level(value, &self.ontology),
                    };
                    if interpretation.ty != expected {
                        return Err(InterpretationError::TypeUniverseMismatch {
                            object: object.clone(),
                        });
                    }
                }
                ObjectMeaning::Symbol { id } => {
                    let declaration = self.ontology.symbols.get(id).ok_or_else(|| {
                        InterpretationError::UnknownSymbol {
                            object: object.clone(),
                            symbol: id.clone(),
                        }
                    })?;
                    if declaration.full_type() != interpretation.ty {
                        return Err(InterpretationError::SymbolTypeMismatch {
                            object: object.clone(),
                            symbol: id.clone(),
                        });
                    }
                }
                ObjectMeaning::Application { .. } => {}
                ObjectMeaning::Quoted => {
                    if interpretation.ty != OntologyTypeExpr::QuotedObject {
                        return Err(InterpretationError::QuotedTypeMismatch(object.clone()));
                    }
                }
            }
        }
        for object in self.interpretations.keys() {
            if !self.graph.nodes.contains_key(object) {
                return Err(InterpretationError::UnexpectedObjectInterpretation(
                    object.clone(),
                ));
            }
        }
        Ok(())
    }

    /// Produces a content-addressed reference to a named root under this exact
    /// ontology and total interpretation map.
    pub fn reference(
        &self,
        root: impl Into<Symbol>,
    ) -> Result<InterpretedGraphRef, InterpretationError> {
        self.validate()?;
        let reference = self.graph.reference(root)?;
        Ok(InterpretedGraphRef {
            submission: self.canonical_hash(),
            graph: reference.graph,
            root: reference.root,
        })
    }

    /// Returns exact graph identity.
    #[must_use]
    pub fn graph_hash(&self) -> GraphHash {
        self.graph.canonical_hash()
    }

    /// Returns the content identity of the graph, ontology, and total interpretation map.
    #[must_use]
    pub fn canonical_hash(&self) -> InterpretedGraphHash {
        let mut bytes = Vec::new();
        put_str(&mut bytes, "artist.interpreted-graph/3");
        put_str(&mut bytes, &self.graph.canonical_hash().0);
        put_str(&mut bytes, &self.ontology.canonical_hash().0);
        put_u64(&mut bytes, self.interpretations.len() as u64);
        for (object, interpretation) in &self.interpretations {
            put_str(&mut bytes, object.as_str());
            put_type_expr(&mut bytes, &interpretation.ty);
            match &interpretation.meaning {
                ObjectMeaning::Type { value } => {
                    bytes.push(0);
                    put_type_expr(&mut bytes, value);
                }
                ObjectMeaning::Symbol { id } => {
                    bytes.push(1);
                    put_str(&mut bytes, &id.0);
                }
                ObjectMeaning::Application { arguments } => {
                    bytes.push(2);
                    put_u64(&mut bytes, arguments.len() as u64);
                    for argument in arguments {
                        put_str(&mut bytes, argument.as_str());
                    }
                }
                ObjectMeaning::Quoted => bytes.push(3),
            }
        }
        InterpretedGraphHash(blake3::hash(&bytes).to_hex().to_string())
    }

    /// Reports application-level type errors without discarding exact represented content.
    #[must_use]
    pub fn typing_diagnostics(&self) -> Vec<OntologicalTypingDiagnostic> {
        let mut diagnostics = Vec::new();
        for (object, node) in &self.graph.nodes {
            let Some(interpretation) = self.interpretations.get(object) else {
                continue;
            };
            let ObjectMeaning::Application { arguments } = &interpretation.meaning else {
                continue;
            };
            let Some(operator_id) = &node.operator else {
                diagnostics.push(OntologicalTypingDiagnostic::MissingOperator {
                    object: object.clone(),
                });
                continue;
            };
            if arguments.is_empty() {
                diagnostics.push(OntologicalTypingDiagnostic::EmptyApplication {
                    object: object.clone(),
                });
            }
            let mut seen_roles = BTreeSet::new();
            for role in arguments {
                if !seen_roles.insert(role.clone()) {
                    diagnostics.push(OntologicalTypingDiagnostic::DuplicateArgumentRole {
                        object: object.clone(),
                        role: role.clone(),
                    });
                }
            }
            let expected_roles: BTreeSet<_> = arguments.iter().cloned().collect();
            for role in node.edges.keys() {
                if !expected_roles.contains(role) {
                    diagnostics.push(OntologicalTypingDiagnostic::UnexpectedRole {
                        object: object.clone(),
                        role: role.clone(),
                    });
                }
            }
            let Some(operator_interpretation) = self.interpretations.get(operator_id) else {
                continue;
            };
            if let ObjectMeaning::Symbol { id: symbol_id } = &operator_interpretation.meaning {
                if let Some(symbol) = self.ontology.symbols.get(symbol_id) {
                    for (position, role) in arguments.iter().enumerate() {
                        if let Some(parameter) = symbol.parameters.get(position) {
                            if &parameter.role != role {
                                diagnostics.push(OntologicalTypingDiagnostic::SymbolRoleMismatch {
                                    object: object.clone(),
                                    symbol: symbol_id.clone(),
                                    position,
                                    expected: parameter.role.clone(),
                                    actual: role.clone(),
                                });
                            }
                        }
                    }
                }
            }
            let mut current_type = operator_interpretation.ty.clone();
            for role in arguments {
                let targets = node.edges.get(role).map_or(&[][..], Vec::as_slice);
                if targets.len() != 1 {
                    diagnostics.push(OntologicalTypingDiagnostic::ArgumentArity {
                        object: object.clone(),
                        role: role.clone(),
                        actual: targets.len(),
                    });
                    continue;
                }
                let (domain, codomain) =
                    if let OntologyTypeExpr::Function { domain, codomain } = &current_type {
                        ((**domain).clone(), (**codomain).clone())
                    } else {
                        diagnostics.push(OntologicalTypingDiagnostic::OperatorIsNotFunction {
                            object: object.clone(),
                            role: role.clone(),
                        });
                        break;
                    };
                if let Some(actual) = self.interpretations.get(&targets[0]) {
                    if actual.ty != domain {
                        diagnostics.push(OntologicalTypingDiagnostic::ArgumentType {
                            object: object.clone(),
                            role: role.clone(),
                            expected: domain,
                            actual: actual.ty.clone(),
                        });
                    }
                }
                current_type = codomain;
            }
            if interpretation.ty != current_type {
                diagnostics.push(OntologicalTypingDiagnostic::ResultType {
                    object: object.clone(),
                    expected: current_type,
                    actual: interpretation.ty.clone(),
                });
            }
        }
        diagnostics
    }
}

fn type_sort_level(ty: &OntologyTypeExpr, ontology: &Ontology) -> u32 {
    match ty {
        OntologyTypeExpr::Universe { level } => level + 1,
        OntologyTypeExpr::QuotedObject => 0,
        OntologyTypeExpr::Named { id } => ontology.types[id].universe,
        OntologyTypeExpr::Function { domain, codomain } => {
            type_sort_level(domain, ontology).max(type_sort_level(codomain, ontology))
        }
    }
}

fn contains_overflowing_universe(ty: &OntologyTypeExpr) -> bool {
    match ty {
        OntologyTypeExpr::Universe { level } => *level == u32::MAX,
        OntologyTypeExpr::QuotedObject | OntologyTypeExpr::Named { .. } => false,
        OntologyTypeExpr::Function { domain, codomain } => {
            contains_overflowing_universe(domain) || contains_overflowing_universe(codomain)
        }
    }
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_str(output: &mut Vec<u8>, value: &str) {
    put_u64(output, value.len() as u64);
    output.extend_from_slice(value.as_bytes());
}

fn put_optional_str(output: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => {
            output.push(1);
            put_str(output, value);
        }
        None => output.push(0),
    }
}

fn put_type_expr(output: &mut Vec<u8>, ty: &OntologyTypeExpr) {
    match ty {
        OntologyTypeExpr::Universe { level } => {
            output.push(0);
            output.extend_from_slice(&level.to_be_bytes());
        }
        OntologyTypeExpr::Named { id } => {
            output.push(1);
            put_str(output, &id.0);
        }
        OntologyTypeExpr::Function { domain, codomain } => {
            output.push(2);
            put_type_expr(output, domain);
            put_type_expr(output, codomain);
        }
        OntologyTypeExpr::QuotedObject => output.push(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GraphBuilder, ObjectNode};

    fn ontology() -> Ontology {
        let proposition = OntologyTypeId::from("Proposition");
        let person = OntologyTypeId::from("Person");
        let alice = OntologySymbolId::from("alice");
        let bob = OntologySymbolId::from("bob");
        Ontology {
            namespace: "test.people".into(),
            version: "1".into(),
            proposition_type: proposition.clone(),
            types: BTreeMap::from([
                (
                    proposition.clone(),
                    OntologyTypeDeclaration {
                        id: proposition,
                        universe: 1,
                        description: None,
                    },
                ),
                (
                    person.clone(),
                    OntologyTypeDeclaration {
                        id: person.clone(),
                        universe: 0,
                        description: None,
                    },
                ),
            ]),
            symbols: BTreeMap::from([
                (
                    alice.clone(),
                    OntologySymbolDeclaration {
                        id: alice,
                        parameters: Vec::new(),
                        result: OntologyTypeExpr::named(person.clone()),
                        description: None,
                    },
                ),
                (
                    bob.clone(),
                    OntologySymbolDeclaration {
                        id: bob,
                        parameters: Vec::new(),
                        result: OntologyTypeExpr::named(person),
                        description: None,
                    },
                ),
            ]),
        }
    }

    #[test]
    fn complete_submission_hash_binds_interpretations() {
        let mut graph = GraphBuilder::new();
        let root = graph.alloc(ObjectNode::new());
        graph.root("main", root.clone());
        let graph = graph.finish().unwrap();
        let ontology = ontology();
        let person = OntologyTypeExpr::named("Person");

        let alice = InterpretedGraph {
            graph: graph.clone(),
            ontology: ontology.clone(),
            interpretations: BTreeMap::from([(
                root.clone(),
                ObjectInterpretation {
                    ty: person.clone(),
                    meaning: ObjectMeaning::Symbol {
                        id: OntologySymbolId::from("alice"),
                    },
                },
            )]),
        };
        let bob = InterpretedGraph {
            graph,
            ontology,
            interpretations: BTreeMap::from([(
                root,
                ObjectInterpretation {
                    ty: person,
                    meaning: ObjectMeaning::Symbol {
                        id: OntologySymbolId::from("bob"),
                    },
                },
            )]),
        };

        alice.validate().unwrap();
        bob.validate().unwrap();
        assert_eq!(alice.graph_hash(), bob.graph_hash());
        assert_eq!(
            alice.ontology.canonical_hash(),
            bob.ontology.canonical_hash()
        );
        assert_ne!(alice.canonical_hash(), bob.canonical_hash());
    }

    #[test]
    fn quoted_meaning_requires_the_universal_quoted_object_type() {
        let mut graph = GraphBuilder::new();
        let root = graph.alloc(ObjectNode::new());
        graph.root("main", root.clone());
        let mut submission = InterpretedGraph {
            graph: graph.finish().unwrap(),
            ontology: ontology(),
            interpretations: BTreeMap::from([(
                root.clone(),
                ObjectInterpretation {
                    ty: OntologyTypeExpr::QuotedObject,
                    meaning: ObjectMeaning::Quoted,
                },
            )]),
        };
        submission.validate().unwrap();
        submission.interpretations.get_mut(&root).unwrap().ty = OntologyTypeExpr::named("Person");
        assert_eq!(
            submission.validate(),
            Err(InterpretationError::QuotedTypeMismatch(root))
        );
    }

    #[test]
    fn proposition_type_is_exactly_type_zero() {
        let mut invalid = ontology();
        invalid
            .types
            .get_mut(&invalid.proposition_type)
            .unwrap()
            .universe = 0;
        assert_eq!(
            invalid.validate(),
            Err(OntologyError::InvalidPropositionUniverse(0))
        );
    }
}
