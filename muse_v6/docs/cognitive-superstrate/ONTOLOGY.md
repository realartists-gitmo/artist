# Ontology contract

## Ontological types

An ontological type states what an object is and how a symbol may be used. Examples include `Person`, `Vehicle`, `Event`, and function types.

A relation such as ownership is declared as:

```text
owns : Person -> Object -> Proposition
```

After application:

```text
owns alice car7 : Proposition
```

The distinguished `Proposition` ontology type maps mechanically to kernel `Type 0`. The elaborated proposition is therefore a DTT type, and a proof is a term inhabiting that type.

`QuotedObject` is a universal built-in ontology type for exact graph objects treated as data. It is not a user-declared domain type and cannot collide with ontology names.

## Exact object meanings

Every graph object has exactly one meaning:

- `Type(value)`: the object denotes an exact ontology type expression;
- `Symbol(id)`: the object denotes one exact declared noun, verb, relation, predicate, or function;
- `Application(arguments)`: the object applies its exact graph operator using the listed role order;
- `Quoted`: the object denotes its exact finite graph identity as `QuotedObject` data.

There is no generic “uninterpreted” state. An application may be fully represented and interpreted while still being ill-typed. Such failures are explicit diagnostics.

## Application typing

Application typing starts from the exact type of the operator and consumes one function domain per listed argument role. This permits:

- full application;
- partial application;
- higher-order application;
- predicates over quoted objects.

A direct ontology symbol must use its declared role order. Higher-order operators use the explicit role order supplied by the application object.

## Theory dependence

Ontology meanings and ontological types do not change between theories. Theories add facts, definitions, axioms, or formal refinements about those exact meanings.

Two theories may disagree about whether `owns alice car7` is provable while sharing the same exact ontology declaration and DTT embedding.

## Infinities

Finite ontology objects may denote infinite domains or constructions. Natural numbers, infinite streams, function spaces, ordinals, quantified claims, nontermination, and infinite behavior are represented intensionally through finite definitions, references, and certificates. The graph does not extensionally enumerate infinitely many objects.
