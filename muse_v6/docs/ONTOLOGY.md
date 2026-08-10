# Ontology packages and executable semantics

An ontology package contains concepts, relations, and explicit axioms. Concepts support
parentage, disjointness, genus/differentia records, rigidity, sortality, identity profile,
abstractness, deprecation, and evidence. Relations support domain/range, super-relations,
reciprocal inverses, characteristics, cardinality, and evidence.

Supported axiom forms are:

- concept and relation subsumption;
- disjointness and equivalence;
- complete partitions;
- domain and range constraints;
- existential and universal restrictions;
- qualified cardinality restrictions;
- binary Horn-style rules with explicit variables.

The index computes transitive type closure, descendants, propagated disjointness, common guaranteed
ancestors, and conservative classifications. The reasoner computes relation inheritance, inverses,
domain/range typing, symmetry/reflexivity, transitivity, equivalence and rules, then checks the
executable constraints.

The bundled profile corrects the diagram errors previously identified:

- trigger/result situation are relations/roles rather than permanent `PreSituation` and
  `PostSituation` subtypes;
- MLT order is orthogonal to host-language and DTT type levels;
- UFO-AB is a temporal/modal bridge package rather than a duplicate taxonomy;
- UFO-S and UFO-L are grounded extension packages rather than peer roots of the foundational
  individual/type taxonomy;
- gUFO and OntoUML are downstream representations and are not used as the full authority.
