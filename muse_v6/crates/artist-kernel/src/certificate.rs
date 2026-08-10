use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::{Context, DeclarationKind, Name, Term, Theory, TheoryId};

/// A finite proof object claiming `theory ; context ⊢ proposition`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Certificate {
    /// Exact theory content identity.
    pub theory: TheoryId,
    /// Exact local assumptions.
    pub context: Context,
    /// Claimed proposition.
    pub proposition: Term,
    /// Explicit proof term.
    pub proof: Term,
}

impl Certificate {
    /// Creates a certificate against the current content identity of a theory.
    #[must_use]
    pub fn new(theory: &Theory, context: Context, proposition: Term, proof: Term) -> Self {
        Self {
            theory: theory.id(),
            context,
            proposition,
            proof,
        }
    }

    /// Computes the exact declaration dependency closure. Local assumptions remain
    /// separately visible in the certificate context.
    #[must_use]
    pub fn dependencies(&self, theory: &Theory) -> DependencyReport {
        let mut pending = VecDeque::new();
        let mut seen = BTreeSet::new();
        for ty in &self.context.0 {
            pending.extend(ty.constants());
        }
        pending.extend(self.proposition.constants());
        pending.extend(self.proof.constants());

        let mut dependencies = Vec::new();
        while let Some(name) = pending.pop_front() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(declaration) = theory.declaration(&name) else {
                continue;
            };
            pending.extend(declaration.ty.constants());
            if let Some(body) = &declaration.body {
                pending.extend(body.constants());
            }
            dependencies.push(Dependency {
                name: declaration.name.clone(),
                kind: declaration.kind,
                source: declaration.provenance.source.clone(),
                external_ref: declaration.provenance.external_ref.clone(),
            });
        }
        dependencies.sort_by(|left, right| left.name.cmp(&right.name));
        DependencyReport {
            theory: theory.id(),
            assumptions: self.context.clone(),
            dependencies,
        }
    }
}

/// Disclosed dependency of an accepted certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    /// Declaration name.
    pub name: Name,
    /// Whether it was checked or trusted.
    pub kind: DeclarationKind,
    /// Declared source.
    pub source: String,
    /// Optional external pin.
    pub external_ref: Option<String>,
}

/// Complete dependency report for a judgment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyReport {
    /// Exact theory version.
    pub theory: TheoryId,
    /// Local assumptions.
    pub assumptions: Context,
    /// Transitive declaration dependencies.
    pub dependencies: Vec<Dependency>,
}
