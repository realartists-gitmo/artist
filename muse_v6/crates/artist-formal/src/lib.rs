//! Universal, theory-neutral representation for Artist cognition.
//!
//! [`ObjectGraph`] stores arbitrary finite or cyclic object graphs. [`InterpretedGraph`]
//! is the normative harness write boundary: every graph object carries an exact
//! ontology type and denotation supplied before it enters the superstrate. Formal
//! elaboration and proof checking are later operations; structural interpretation
//! is total.

#![forbid(unsafe_code)]

mod graph;
mod ontology;
mod query;

pub use graph::{
    ExternalRef, GraphBuilder, GraphError, GraphHash, GraphRef, Literal, ObjectGraph, ObjectId,
    ObjectNode, Symbol,
};
pub use ontology::{
    InterpretationError, InterpretedGraph, InterpretedGraphHash, InterpretedGraphRef,
    ObjectInterpretation, ObjectMeaning, OntologicalTypingDiagnostic, Ontology, OntologyError,
    OntologyHash, OntologyId, OntologyParameter, OntologySymbolDeclaration, OntologySymbolId,
    OntologyTypeDeclaration, OntologyTypeExpr, OntologyTypeId,
};
pub use query::{HoleId, OpenQuery, QueryAnswer, QueryError, QueryHole};
