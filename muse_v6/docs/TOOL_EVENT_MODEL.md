# Protocol-neutral tool event model

`muse-tooling` is the harness-independent normalization boundary for structured tool records.

## Identity

Native IDs are never assumed globally unique. Every external identity declares one scope:

- `Source`: stable across runs/records from one source.
- `Run`: stable only inside a source run; requires `source.run`.
- `Record`: stable only inside one record.

Identity scope is part of identity. Equal raw strings in different scopes are distinct.

## Invocation, result, and effect are different claims

A tool invocation records that a principal invoked a tool with arguments.

A tool result records what the protocol returned and the status the protocol reported.

An effect records a requested or independently observed state transition/action.

`success` is not effect evidence. A result may report success while the only justified filesystem claim is that a write was requested. An observed write requires independent structured evidence.

## Artifact identity and state

A path literal is a `comp:Path`, not proof of a `comp:File` or `comp:Directory`.

Artifact identity is stable independently from state. Digests, before/after snapshots, and other state observations are represented as state descriptors/occurrences attached to an artifact reference. A content change does not create a new artifact identity merely because its digest changed.

## Generic extension rule

Known effect families cover filesystem, patch, process, build, compilation, test, network, and model invocation. Future harness-specific semantics use ontology-backed extension concepts without changing the shared occurrence language.

Concrete harness adapters may specialize structured records only when the native source deterministically supplies the specialization. Unknown tools remain valid generic tool invocations.
