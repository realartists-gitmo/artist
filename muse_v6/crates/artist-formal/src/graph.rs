use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A stable symbolic label used for properties, edge roles, and roots.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Symbol(pub String);

impl Symbol {
    /// Creates a symbol.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the symbol text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Symbol {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for Symbol {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Stable identity of an object within a graph.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectId(pub String);

impl ObjectId {
    /// Creates an object identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ObjectId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for ObjectId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A scalar payload. Numbers are stored lexically so representation never loses
/// precision or silently commits to IEEE-754 semantics.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Literal {
    /// UTF-8 text.
    Text(String),
    /// An arbitrary-precision signed integer in canonical decimal syntax.
    Integer(String),
    /// An exact decimal or rational lexical form interpreted by a theory.
    Number(String),
    /// A boolean scalar.
    Bool(bool),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

/// A reference to something whose complete representation lives outside this graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalRef {
    /// URI-like scheme identifying the external authority.
    pub scheme: Symbol,
    /// Authority-specific locator.
    pub locator: String,
    /// Optional content digest or version pin.
    pub integrity: Option<String>,
}

/// One open graph object.
///
/// `operator` points to another ordinary object rather than an enum variant. This
/// keeps predicates and operators first-class and permits unknown future operators.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectNode {
    /// Optional first-class operator or kind object.
    pub operator: Option<ObjectId>,
    /// Ordered role-labelled edges. Multiple targets preserve n-ary relations.
    pub edges: BTreeMap<Symbol, Vec<ObjectId>>,
    /// Theory-neutral scalar properties.
    pub properties: BTreeMap<Symbol, Literal>,
    /// Optional opaque external denotation.
    pub external: Option<ExternalRef>,
}

impl ObjectNode {
    /// Creates an empty object.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the first-class operator.
    #[must_use]
    pub fn with_operator(mut self, operator: impl Into<ObjectId>) -> Self {
        self.operator = Some(operator.into());
        self
    }

    /// Adds an outgoing edge.
    #[must_use]
    pub fn with_edge(mut self, role: impl Into<Symbol>, target: impl Into<ObjectId>) -> Self {
        self.edges
            .entry(role.into())
            .or_default()
            .push(target.into());
        self
    }

    /// Adds a scalar property.
    #[must_use]
    pub fn with_property(mut self, key: impl Into<Symbol>, value: Literal) -> Self {
        self.properties.insert(key.into(), value);
        self
    }
}

/// Stable BLAKE3 digest of a canonical graph encoding.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GraphHash(pub String);

impl fmt::Display for GraphHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A content-addressed root inside a graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRef {
    /// Canonical graph digest.
    pub graph: GraphHash,
    /// Root object within that graph.
    pub root: ObjectId,
}

/// An arbitrary finite graph. Cycles are explicitly permitted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectGraph {
    /// Named entry points.
    pub roots: BTreeMap<Symbol, ObjectId>,
    /// Objects keyed by stable identity.
    pub nodes: BTreeMap<ObjectId, ObjectNode>,
}

/// Structural graph errors. Unknown operators are not errors.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GraphError {
    /// A named root is missing.
    #[error("root {name} points to missing object {target}")]
    MissingRootTarget { name: Symbol, target: ObjectId },
    /// An operator edge points to a missing object.
    #[error("object {object} has missing operator {target}")]
    MissingOperatorTarget { object: ObjectId, target: ObjectId },
    /// A role edge points to a missing object.
    #[error("object {object} edge {role} points to missing object {target}")]
    MissingEdgeTarget {
        object: ObjectId,
        role: Symbol,
        target: ObjectId,
    },
    /// Integer literal does not use canonical signed decimal syntax.
    #[error("object {object} property {property} has non-canonical integer literal {value:?}")]
    InvalidIntegerLiteral {
        object: ObjectId,
        property: Symbol,
        value: String,
    },
    /// The requested root name does not exist.
    #[error("graph has no root named {0}")]
    UnknownRoot(Symbol),
}

impl ObjectGraph {
    /// Validates structural references. Cycles, unknown operators, and opaque objects
    /// are accepted.
    pub fn validate(&self) -> Result<(), GraphError> {
        for (name, target) in &self.roots {
            if !self.nodes.contains_key(target) {
                return Err(GraphError::MissingRootTarget {
                    name: name.clone(),
                    target: target.clone(),
                });
            }
        }
        for (source, node) in &self.nodes {
            if let Some(target) = &node.operator {
                if !self.nodes.contains_key(target) {
                    return Err(GraphError::MissingOperatorTarget {
                        object: source.clone(),
                        target: target.clone(),
                    });
                }
            }
            for (role, targets) in &node.edges {
                for target in targets {
                    if !self.nodes.contains_key(target) {
                        return Err(GraphError::MissingEdgeTarget {
                            object: source.clone(),
                            role: role.clone(),
                            target: target.clone(),
                        });
                    }
                }
            }
            for (property, literal) in &node.properties {
                if let Literal::Integer(value) = literal {
                    if !canonical_integer(value) {
                        return Err(GraphError::InvalidIntegerLiteral {
                            object: source.clone(),
                            property: property.clone(),
                            value: value.clone(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Resolves a named root.
    pub fn root(&self, name: impl Into<Symbol>) -> Result<&ObjectNode, GraphError> {
        let name = name.into();
        let id = self
            .roots
            .get(&name)
            .ok_or_else(|| GraphError::UnknownRoot(name.clone()))?;
        self.nodes
            .get(id)
            .ok_or_else(|| GraphError::MissingRootTarget {
                name,
                target: id.clone(),
            })
    }

    /// Returns the canonical content hash. Object identities are semantically
    /// significant and therefore included.
    #[must_use]
    pub fn canonical_hash(&self) -> GraphHash {
        let mut bytes = Vec::new();
        put_str(&mut bytes, "artist.graph/1");
        put_u64(&mut bytes, self.roots.len() as u64);
        for (name, id) in &self.roots {
            put_str(&mut bytes, name.as_str());
            put_str(&mut bytes, id.as_str());
        }
        put_u64(&mut bytes, self.nodes.len() as u64);
        for (id, node) in &self.nodes {
            put_str(&mut bytes, id.as_str());
            match &node.operator {
                Some(operator) => {
                    bytes.push(1);
                    put_str(&mut bytes, operator.as_str());
                }
                None => bytes.push(0),
            }
            put_u64(&mut bytes, node.edges.len() as u64);
            for (role, targets) in &node.edges {
                put_str(&mut bytes, role.as_str());
                put_u64(&mut bytes, targets.len() as u64);
                for target in targets {
                    put_str(&mut bytes, target.as_str());
                }
            }
            put_u64(&mut bytes, node.properties.len() as u64);
            for (key, value) in &node.properties {
                put_str(&mut bytes, key.as_str());
                put_literal(&mut bytes, value);
            }
            match &node.external {
                Some(reference) => {
                    bytes.push(1);
                    put_str(&mut bytes, reference.scheme.as_str());
                    put_str(&mut bytes, &reference.locator);
                    match &reference.integrity {
                        Some(integrity) => {
                            bytes.push(1);
                            put_str(&mut bytes, integrity);
                        }
                        None => bytes.push(0),
                    }
                }
                None => bytes.push(0),
            }
        }
        GraphHash(blake3::hash(&bytes).to_hex().to_string())
    }

    /// Produces a content-addressed reference to a named root.
    pub fn reference(&self, root: impl Into<Symbol>) -> Result<GraphRef, GraphError> {
        self.validate()?;
        let root_name = root.into();
        let root = self
            .roots
            .get(&root_name)
            .cloned()
            .ok_or_else(|| GraphError::UnknownRoot(root_name.clone()))?;
        Ok(GraphRef {
            graph: self.canonical_hash(),
            root,
        })
    }

    /// Returns all objects reachable from the named root. This operation terminates
    /// on cyclic graphs by tracking visited identities.
    pub fn reachable(&self, root: impl Into<Symbol>) -> Result<BTreeSet<ObjectId>, GraphError> {
        self.validate()?;
        let root_name = root.into();
        let start = self
            .roots
            .get(&root_name)
            .cloned()
            .ok_or(GraphError::UnknownRoot(root_name))?;
        let mut seen = BTreeSet::new();
        let mut pending = vec![start];
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let node = &self.nodes[&id];
            if let Some(operator) = &node.operator {
                pending.push(operator.clone());
            }
            for targets in node.edges.values() {
                pending.extend(targets.iter().cloned());
            }
        }
        Ok(seen)
    }
}

/// Convenience builder that allocates graph-local deterministic identities.
#[derive(Clone, Debug, Default)]
pub struct GraphBuilder {
    graph: ObjectGraph,
    next_id: u64,
}

impl GraphBuilder {
    /// Creates an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a node with an explicit identity.
    pub fn insert(&mut self, id: impl Into<ObjectId>, node: ObjectNode) -> ObjectId {
        let id = id.into();
        self.graph.nodes.insert(id.clone(), node);
        id
    }

    /// Allocates and inserts a node.
    pub fn alloc(&mut self, node: ObjectNode) -> ObjectId {
        loop {
            let id = ObjectId(format!("n{next}", next = self.next_id));
            self.next_id += 1;
            if !self.graph.nodes.contains_key(&id) {
                self.graph.nodes.insert(id.clone(), node);
                return id;
            }
        }
    }

    /// Defines or replaces a named root.
    pub fn root(&mut self, name: impl Into<Symbol>, id: impl Into<ObjectId>) {
        self.graph.roots.insert(name.into(), id.into());
    }

    /// Mutably accesses an existing object.
    #[must_use]
    pub fn node_mut(&mut self, id: &ObjectId) -> Option<&mut ObjectNode> {
        self.graph.nodes.get_mut(id)
    }

    /// Completes the graph after structural validation.
    pub fn finish(self) -> Result<ObjectGraph, GraphError> {
        self.graph.validate()?;
        Ok(self.graph)
    }
}

fn canonical_integer(value: &str) -> bool {
    if value == "0" {
        return true;
    }
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn put_str(output: &mut Vec<u8>, value: &str) {
    put_u64(output, value.len() as u64);
    output.extend_from_slice(value.as_bytes());
}

fn put_literal(output: &mut Vec<u8>, literal: &Literal) {
    match literal {
        Literal::Text(value) => {
            output.push(0);
            put_str(output, value);
        }
        Literal::Integer(value) => {
            output.push(1);
            put_str(output, value);
        }
        Literal::Number(value) => {
            output.push(2);
            put_str(output, value);
        }
        Literal::Bool(value) => {
            output.push(3);
            output.push(u8::from(*value));
        }
        Literal::Bytes(value) => {
            output.push(4);
            put_u64(output, value.len() as u64);
            output.extend_from_slice(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cyclic_self_reference_is_valid_and_hashable() {
        let mut builder = GraphBuilder::new();
        let liar = builder.insert("liar", ObjectNode::new());
        let truth = builder.insert(
            "truth",
            ObjectNode::new().with_property("symbol", Literal::Text("truth".into())),
        );
        *builder.node_mut(&liar).unwrap() = ObjectNode::new()
            .with_operator(truth)
            .with_edge("quoted", liar.clone());
        builder.root("main", liar);
        let graph = builder.finish().unwrap();
        assert_eq!(graph.reachable("main").unwrap().len(), 2);
        assert_eq!(graph.canonical_hash(), graph.clone().canonical_hash());
    }

    #[test]
    fn dangling_references_are_rejected() {
        let mut builder = GraphBuilder::new();
        let node = builder.alloc(ObjectNode::new().with_edge("x", "missing"));
        builder.root("main", node);
        assert!(matches!(
            builder.finish(),
            Err(GraphError::MissingEdgeTarget { .. })
        ));
    }

    #[test]
    fn unknown_operator_is_not_semantically_rejected() {
        let mut builder = GraphBuilder::new();
        let unknown = builder.insert(
            "future-operator",
            ObjectNode::new().with_property("name", Literal::Text("future".into())),
        );
        let expression = builder.alloc(ObjectNode::new().with_operator(unknown));
        builder.root("main", expression);
        assert!(builder.finish().is_ok());
    }

    #[test]
    fn integer_literals_use_one_exact_encoding() {
        for valid in ["0", "1", "-1", "123456789"] {
            assert!(canonical_integer(valid));
        }
        for invalid in ["", "00", "01", "-0", "+1", "1.0", "x"] {
            assert!(!canonical_integer(invalid));
        }
    }

    #[test]
    fn malformed_deserialized_roots_return_errors_instead_of_panicking() {
        let graph = ObjectGraph {
            roots: BTreeMap::from([(Symbol::from("main"), ObjectId::from("missing"))]),
            nodes: BTreeMap::new(),
        };
        assert!(matches!(
            graph.root("main"),
            Err(GraphError::MissingRootTarget { .. })
        ));
        assert!(matches!(
            graph.reference("main"),
            Err(GraphError::MissingRootTarget { .. })
        ));
        assert!(matches!(
            graph.reachable("main"),
            Err(GraphError::MissingRootTarget { .. })
        ));
    }
}
