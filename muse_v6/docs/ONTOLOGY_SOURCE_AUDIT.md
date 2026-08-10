# Ontology funnel source audit

Status: source audit completed 2026-08-06. This document pins the external material to be used when the missing Muse ontology funnel is authored. It does not itself add ontology semantics.

## Source-use policy

Muse remains UFO-grounded. External models are used in one of three ways:

- **ontology source**: definitions or distinctions may be adopted when their identity/dependence conditions fit Muse's UFO model;
- **semantic crosswalk**: Muse keeps its own semantics and records an explicit alignment to an external vocabulary;
- **interoperability evidence**: field names and event distinctions inform adapters only and are not treated as ontology authority.

No source below is imported wholesale.

## 1. Software engineering: SEON 1.0.5

Pinned source family: SEON, Software Engineering Ontology Network, NEMO/UFES, version 1.0.5, published 2017-09-25.

Primary locator: https://dev.nemo.inf.ufes.br/seon/SEON.html

SEON is the primary software-engineering ontology source because its architecture is itself grounded in UFO. Its foundational/core/domain funnel is compatible with Muse's required funnel: UFO grounds the Software Process Ontology (SPO), and SPO grounds domain ontologies.

Pinned component specifications:

- Software Ontology (SwO): https://dev.nemo.inf.ufes.br/seon/SwO.html
- Coding Process Ontology (CPO): https://dev.nemo.inf.ufes.br/seon/CPO.html
- Reference Software Requirements Ontology (RSRO): https://dev.nemo.inf.ufes.br/seon/RSRO.html
- Reference Ontology on Software Testing (ROoST): https://dev.nemo.inf.ufes.br/seon/ROoST.html
- Configuration Management Process Ontology (CMPO): https://dev.nemo.inf.ufes.br/seon/CMPO.html
- Runtime Requirements Ontology (RRO), especially Program Execution: https://dev.nemo.inf.ufes.br/seon/RRO.html

Adoption decisions:

- Reuse the distinction between a software product, program, code/source code, software-development activities, requirements, testing activities/artifacts, and configuration-management activities where the SEON concept's UFO grounding is compatible with Muse.
- Preserve SEON's distinction between an artifact/information item and an activity/event that creates, changes, uses, executes, or tests it.
- Treat a requirement as intentional/normative content only when the source supports that semantics. Do not collapse a requirement artifact/document into the requirement it describes.
- Reuse RRO's distinction between a program and a program execution as a key input to the later computing ontology.
- Do not copy SEON cardinalities or historical terminology mechanically. Each adopted constraint must be checked against Muse's broader intended domain and recorded as adopted, adapted, or rejected.

## 2. Provenance: W3C PROV-O Recommendation

Pinned source: W3C PROV-O, W3C Recommendation, 2013-04-30.

Versioned locator: https://www.w3.org/TR/2013/REC-prov-o-20130430/

Latest-document locator: https://www.w3.org/TR/prov-o/

Use: semantic crosswalk, not replacement of Muse provenance.

Candidate crosswalk:

| Muse provenance construct | PROV-O alignment | Decision |
|---|---|---|
| `SourceRecord` when denoting a source artifact/resource | `prov:Entity` | align when the record denotes an identifiable source entity |
| `AgentRecord` | `prov:Agent` | direct crosswalk |
| `ActivityRecord` | `prov:Activity` | direct crosswalk |
| `ActivityRecord.used_sources` | `prov:used` | direct relationship crosswalk |
| generated evidence/artifact -> activity | `prov:wasGeneratedBy` | use where generation is represented explicitly |
| attribution to an agent | `prov:wasAttributedTo` | direct relationship crosswalk |

Muse evidence status, confidence, assertion status, immutable package pins, and content hashes remain Muse semantics. PROV-O does not determine their truth conditions.

## 3. Software metadata: CodeMeta 3.1

Pinned source: CodeMeta 3.1 context and vocabulary.

Stable context: https://w3id.org/codemeta/3.1

Documentation: https://codemeta.github.io/terms/

Use: metadata interoperability and lexical alignment. CodeMeta is not used as a foundational ontology.

Candidate mappings include software source/application identity, repository location, software version, runtime platform, software requirements/dependencies, build instructions, issue tracker, release metadata, and source-code/application links. These are metadata relationships on Muse software entities; they do not determine Muse identity criteria for software systems, programs, source artifacts, or builds.

## 4. Software artifacts/builds/SBOM: SPDX 3.0.1

Pinned source: SPDX Specification 3.0.1.

Specification root: https://spdx.github.io/spdx-spec/v3.0.1/

Relevant pinned terms:

- SoftwareArtifact: https://spdx.github.io/spdx-spec/v3.0.1/model/Software/Classes/SoftwareArtifact/
- Package: https://spdx.github.io/spdx-spec/v3.0.1/model/Software/Classes/Package/
- File: https://spdx.github.io/spdx-spec/v3.0.1/model/Software/Classes/File/
- Build profile: https://spdx.github.io/spdx-spec/v3.0.1/model/Build/Build/
- Build class: https://spdx.github.io/spdx-spec/v3.0.1/model/Build/Classes/Build/

Use: ontology-source input where compatible, plus external metadata crosswalk.

Adoption decisions:

- SPDX's software-artifact/package/file distinctions are useful interoperability categories but do not by themselves settle UFO identity/sortality.
- SPDX Build explicitly represents software-build information and inputs/outputs. Muse will distinguish the build occurrence/event from a record that describes that build.
- SPDX `hasInput`, `hasOutput`, `usesTool`, configuration, and content identifiers are useful crosswalk targets for later build provenance.
- A package is not synonymous with a software product or program. Muse will model those separately and crosswalk only when an external SPDX record actually denotes the same artifact.

## 5. Agent messages: FIPA ACL SC00061G

Pinned source: FIPA ACL Message Structure Specification, SC00061G, Standard, status date 2002-12-03.

Repository: https://www.fipa.org/repository/aclspecs.html

Specification locator: http://www.fipa.org/specs/fipa00061/

Use: ontology-source input for message/conversation structure and interoperability terminology, not authority for implementation-level software-agent intentional states.

Useful distinctions include communicative act/performative, sender, receiver, content, content language, ontology reference, protocol, conversation identity, reply correlation, and reply deadline. Muse may represent these as properties/roles of messages and communicative occurrences. Software-agent internal state must not be equated with human belief/desire/intention merely because FIPA uses speech-act/BDI semantics.

## 6. RPC/tool invocation: JSON-RPC 2.0

Pinned source: JSON-RPC 2.0 Specification.

Locator: https://www.jsonrpc.org/specification

Use: interoperability evidence for generic request/result/error distinctions.

Useful distinctions include request identity, method, parameters, successful `result`, structured `error`, notifications without a response, and request/response ID correlation. Muse's tool invocation model will be protocol-neutral; JSON-RPC terms are crosswalk targets only.

## 7. Current telemetry: OpenTelemetry Semantic Conventions

Pinned core semantic-conventions release for this audit: v1.43.0, release commit `89aae43`, released 2026-07-03.

Release locator: https://github.com/open-telemetry/semantic-conventions/releases/tag/v1.43.0

RPC conventions: https://opentelemetry.io/docs/specs/semconv/rpc/

Use: interoperability evidence only. RPC conventions distinguish client/server call lifecycle, method/system identity, error type, status, retries, and timing. Their current status is release-candidate rather than ontology authority.

Generative-AI conventions moved out of the core repository in v1.42.0. The dedicated repository is therefore the pinned source family for later agent/tool telemetry work:

- repository: https://github.com/open-telemetry/semantic-conventions-genai
- schema URL advertised by the repository at audit time: https://opentelemetry.io/schemas/gen-ai/1.42.0
- agent-span documentation: https://github.com/open-telemetry/semantic-conventions-genai/blob/main/docs/gen-ai/gen-ai-agent-spans.md

The GenAI conventions are useful evidence for `invoke_agent`, `execute_tool`, tool-call arguments/results, provider/model identity, and trace correlation. They must not define Muse's ontology or collapse telemetry spans into semantic occurrences.

## 8. Consequences for Muse package design

The first new ontology package should be a software-engineering package grounded primarily in the existing Muse UFO packages and source-aligned to SEON. SPDX and CodeMeta mappings should be explicit interoperability annotations/evidence rather than superclass imports.

The next computing package should specialize execution/filesystem/command concepts, reusing the SEON program-vs-execution distinction and using SPDX/JSON-RPC/OpenTelemetry only where their distinctions improve interoperability.

The AI/software-agent package should ground agency/message concepts in UFO-C where justified, use FIPA for message structure, and treat OpenTelemetry GenAI conventions as telemetry crosswalk evidence.

No Artist-specific ontology should be authored until the current Artist repository is inspected through the authoritative GitHub connection, as required by the handoff.

## Verification policy

Ontology/source work is part of the pre-training implementation critical path. The complete Rust/release verification suite is intentionally deferred until the semantic schema, deterministic tool formalizer, formal lowering, label specification, and training corpus contract are ready to freeze. Static, deterministic-generation, sealing, manifest, and other toolchain-independent checks run continuously during implementation.

The final Rust gate immediately before the training-contract freeze must still pass in full; deferring that gate does not waive it.
