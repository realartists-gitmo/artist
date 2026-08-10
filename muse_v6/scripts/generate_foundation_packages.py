#!/usr/bin/env python3
"""Generate sealed software/computing/agent/coding-harness Muse ontology packages."""
from __future__ import annotations

from copy import deepcopy
from pathlib import Path
import argparse
import json
import os
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = ROOT / "packages" / "foundation"
VERSION = "1.0.0"
ZERO = {"algorithm": "sha256", "value": "0" * 64}


def package_ref(path: Path):
    data = json.loads(path.read_text())
    return deepcopy(data["payload"]["document"]["package"]["header"]["package"])


def exact(ref):
    return {
        "id": ref["id"],
        "version": ref["version"],
        "digest": deepcopy(ref["digest"]),
        "optional": False,
    }


def profile(entity_mode="individual", rigidity=None, sortality=None, identity=None, abstractness=None, **meta):
    return {
        "entity_mode": entity_mode,
        "rigidity": rigidity,
        "sortality": sortality,
        "identity": identity,
        "abstractness": abstractness,
        "metaproperties": dict(sorted(meta.items())),
    }


def header(pid, kind, title, description, imports=(), evidence=()):
    return {
        "package": {"id": pid, "version": VERSION, "digest": deepcopy(ZERO)},
        "kind": kind,
        "title": title,
        "description": description,
        "license": "MIT OR Apache-2.0",
        "imports": list(imports),
        "evidence": sorted(evidence),
    }


def concept(cid, label, text, parents, evidence, *, prof=None, alt=(), notes=(), disjoint=()):
    return {
        "id": cid,
        "preferred_labels": {"en": label},
        "alternate_labels": {"en": sorted(set(alt))} if alt else {},
        "definition": {
            "text": text,
            "genus": parents[0] if parents else None,
            "differentia": [],
            "necessary_conditions": [],
            "sufficient_conditions": [],
            "scope": None,
            "notes": list(notes),
        },
        "parents": sorted(set(parents)),
        "disjoint_with": sorted(set(disjoint)),
        "profile": prof or profile(),
        "deprecated": False,
        "evidence": sorted(set(evidence)),
    }


def relation(rid, label, text, domains, ranges, evidence, *, supers=(), inverse=None, chars=(), minimum=None, maximum=None):
    return {
        "id": rid,
        "preferred_labels": {"en": label},
        "definition": text,
        "domains": sorted(set(domains)),
        "ranges": sorted(set(ranges)),
        "super_relations": sorted(set(supers)),
        "inverse": inverse,
        "characteristics": sorted(set(chars)),
        "cardinality": None if minimum is None and maximum is None else {"minimum": minimum, "maximum": maximum},
        "deprecated": False,
        "evidence": sorted(set(evidence)),
    }


def ontology(pid, title, description, imports, evidence, concepts, relations, axioms=()):
    return {
        "header": header(pid, "ontology", title, description, imports, evidence),
        "concepts": dict(sorted(concepts.items())),
        "relations": dict(sorted(relations.items())),
        "axioms": list(axioms),
    }


def source(sid, kind, title, creators, locator, citation, issued=None, notes=()):
    return {
        "id": sid,
        "kind": kind,
        "title": title,
        "creators": list(creators),
        "locator": locator,
        "citation": citation,
        "issued": issued,
        "notes": list(notes),
    }


def evidence(eid, sid, status, note, locator=None):
    return {
        "id": eid,
        "status": status,
        "source": sid,
        "generated_by": None,
        "attributed_to": [],
        "confidence": 10000,
        "excerpt": None,
        "locator": locator,
        "notes": [note],
    }


def seal(out: Path, kind: str, package: dict, filename: str):
    document = {"kind": kind, "package": package}
    out.mkdir(parents=True, exist_ok=True)
    # Muse's typed Rust serialization is the authority for the package and
    # envelope digests. The generator supplies source data but never guesses
    # serde's canonical field order.
    raw = out / f".{filename}.raw.json"
    output = out / filename
    raw.write_text(json.dumps(document, ensure_ascii=False) + "\n")
    seal_bin = Path(os.environ.get("MUSE_SEAL_BIN", ROOT / "target" / "debug" / "muse"))
    if not seal_bin.is_file():
        raise RuntimeError(
            f"Muse sealing binary is unavailable at {seal_bin}; build muse-cli or set MUSE_SEAL_BIN"
        )
    subprocess.run([str(seal_bin), "seal-package", str(raw), str(output)], check=True)
    raw.unlink()
    sealed = json.loads(output.read_text())["payload"]["document"]["package"]
    return deepcopy(sealed["header"]["package"])


def provenance_package():
    sources = {
        "source:seon-swo": source(
            "source:seon-swo", "documentation", "Software Ontology (SwO)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/SwO.html",
            "SEON Software Ontology (SwO), SEON 1.0.5.", "2017-09-25",
            ["UFO-grounded source for software product, program, code, source code, and specification distinctions."],
        ),
        "source:seon-cpo": source(
            "source:seon-cpo", "documentation", "Coding Process Ontology (CPO)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/CPO.html",
            "SEON Coding Process Ontology (CPO), SEON 1.0.5.", "2017-09-25",
            ["UFO-grounded source for coding activities, code development, documentation, and review."],
        ),
        "source:seon-rsro": source(
            "source:seon-rsro", "documentation", "Reference Software Requirements Ontology (RSRO)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/RSRO.html",
            "SEON Reference Software Requirements Ontology (RSRO), SEON 1.0.5.", "2017-09-25",
        ),
        "source:seon-roost": source(
            "source:seon-roost", "documentation", "Reference Ontology on Software Testing (ROoST)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/ROoST.html",
            "SEON Reference Ontology on Software Testing (ROoST), SEON 1.0.5.", "2017-09-25",
        ),
        "source:seon-cmpo": source(
            "source:seon-cmpo", "documentation", "Configuration Management Process Ontology (CMPO)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/CMPO.html",
            "SEON Configuration Management Process Ontology (CMPO), SEON 1.0.5.", "2017-09-25",
        ),
        "source:seon-rro": source(
            "source:seon-rro", "documentation", "Runtime Requirements Ontology (RRO)",
            ["NEMO/UFES Software Engineering Ontology Network"],
            "https://dev.nemo.inf.ufes.br/seon/RRO.html",
            "SEON Runtime Requirements Ontology (RRO), SEON 1.0.5.", "2017-09-25",
            ["Source for preserving the program/program-execution distinction."],
        ),
        "source:spdx-3.0.1": source(
            "source:spdx-3.0.1", "standard", "SPDX Specification 3.0.1",
            ["Linux Foundation SPDX Project"],
            "https://spdx.github.io/spdx-spec/v3.0.1/",
            "SPDX Specification 3.0.1.", "2025",
            ["Used for software-artifact/build interoperability evidence; not imported as foundational identity criteria."],
        ),
        "source:codemeta-3.1": source(
            "source:codemeta-3.1", "documentation", "CodeMeta 3.1",
            ["CodeMeta Project"], "https://w3id.org/codemeta/3.1",
            "CodeMeta 3.1 vocabulary and context.", None,
            ["Metadata interoperability only."],
        ),
        "source:prov-o": source(
            "source:prov-o", "standard", "PROV-O: The PROV Ontology",
            ["World Wide Web Consortium"],
            "https://www.w3.org/TR/2013/REC-prov-o-20130430/",
            "W3C Recommendation, PROV-O, 30 April 2013.", "2013-04-30",
            ["Crosswalk source for entities, activities, agents, use, generation, and attribution."],
        ),
        "source:fipa-acl": source(
            "source:fipa-acl", "standard", "FIPA ACL Message Structure Specification",
            ["Foundation for Intelligent Physical Agents"],
            "http://www.fipa.org/specs/fipa00061/",
            "FIPA ACL Message Structure Specification, SC00061G.", "2002-12-03",
            ["Used for message/conversation structure; does not identify software implementation state with human mental states."],
        ),
        "source:jsonrpc-2": source(
            "source:jsonrpc-2", "standard", "JSON-RPC 2.0 Specification",
            ["JSON-RPC Working Group"], "https://www.jsonrpc.org/specification",
            "JSON-RPC 2.0 Specification.", "2010-03-26",
            ["Protocol-neutral request/result/error interoperability evidence."],
        ),
        "source:otel-semconv-1.43": source(
            "source:otel-semconv-1.43", "documentation", "OpenTelemetry Semantic Conventions 1.43.0",
            ["OpenTelemetry Authors"],
            "https://github.com/open-telemetry/semantic-conventions/releases/tag/v1.43.0",
            "OpenTelemetry Semantic Conventions v1.43.0.", "2026-07-03",
            ["Telemetry interoperability evidence only."],
        ),
        "source:otel-genai": source(
            "source:otel-genai", "documentation", "OpenTelemetry GenAI Semantic Conventions",
            ["OpenTelemetry Authors"],
            "https://github.com/open-telemetry/semantic-conventions-genai",
            "OpenTelemetry semantic-conventions-genai repository; schema family 1.42.0 at audit time.", "2026",
            ["Evidence for agent/model/tool telemetry vocabulary; not ontology authority."],
        ),
        "source:artist-gortnite-event-schema": source(
            "source:artist-gortnite-event-schema", "source_code", "Artist Gortnite session event schema",
            ["Artist project"],
            "https://github.com/realartists-gitmo/artist/blob/656383b4906a796727b09a249ef5da7e60f51b81/crates/artist-session/src/event.rs",
            "Artist Gortnite event.rs at commit 656383b4906a796727b09a249ef5da7e60f51b81; blob 903add9c1a33947423ed82820718edffe74ca35a.",
            "2026-08-07",
            ["Authoritative implementation source for the frozen Artist session envelope and event payload kinds."],
        ),
        "source:artist-gortnite-tool-registry": source(
            "source:artist-gortnite-tool-registry", "source_code", "Artist Gortnite exhaustive built-in tool registry",
            ["Artist project"],
            "https://github.com/realartists-gitmo/artist/blob/656383b4906a796727b09a249ef5da7e60f51b81/crates/artist-agent/src/tool_set.rs",
            "Artist Gortnite tool_set.rs at commit 656383b4906a796727b09a249ef5da7e60f51b81.",
            "2026-08-07",
            ["Authoritative implementation source for Artist built-in tool availability and names."],
        ),
        "source:artist-gortnite-tool-capture": source(
            "source:artist-gortnite-tool-capture", "source_code", "Artist Gortnite structured tool-result capture",
            ["Artist project"],
            "https://github.com/realartists-gitmo/artist/blob/656383b4906a796727b09a249ef5da7e60f51b81/crates/artist-agent/src/capture.rs",
            "Artist Gortnite capture.rs at commit 656383b4906a796727b09a249ef5da7e60f51b81; blob aba5be88db5f305fa24d0334268fcaab12b5a22e.",
            "2026-08-07",
            ["Authoritative implementation source for internal tool-call correlation, structured outcomes, and duration capture."],
        ),
        "source:muse-design": source(
            "source:muse-design", "expert_decision", "Muse training critical-path semantic design",
            ["Muse project"], "docs/TRAINING_CRITICAL_PATH.md",
            "Muse project-authored semantic design decisions for the shared formalizer and training contract.", "2026-08-06",
            ["Used only where no external source should determine Muse-specific identity or safety distinctions."],
        ),
    }
    ev = {
        "evidence:seon-swo": evidence("evidence:seon-swo", "source:seon-swo", "canonical", "Adopted/adapted UFO-grounded software distinctions from SwO."),
        "evidence:seon-cpo": evidence("evidence:seon-cpo", "source:seon-cpo", "canonical", "Adopted/adapted coding-activity distinctions from CPO."),
        "evidence:seon-rsro": evidence("evidence:seon-rsro", "source:seon-rsro", "canonical", "Requirements ontology source alignment."),
        "evidence:seon-roost": evidence("evidence:seon-roost", "source:seon-roost", "canonical", "Software-testing ontology source alignment."),
        "evidence:seon-cmpo": evidence("evidence:seon-cmpo", "source:seon-cmpo", "canonical", "Configuration-management ontology source alignment."),
        "evidence:seon-rro": evidence("evidence:seon-rro", "source:seon-rro", "canonical", "Runtime/program-execution ontology source alignment."),
        "evidence:spdx": evidence("evidence:spdx", "source:spdx-3.0.1", "adopted_from_standard", "SPDX artifact/build interoperability mapping evidence."),
        "evidence:codemeta": evidence("evidence:codemeta", "source:codemeta-3.1", "canonical", "CodeMeta metadata interoperability mapping evidence."),
        "evidence:prov-o": evidence("evidence:prov-o", "source:prov-o", "adopted_from_standard", "PROV-O provenance crosswalk evidence."),
        "evidence:fipa": evidence("evidence:fipa", "source:fipa-acl", "adopted_from_standard", "FIPA message/conversation structural evidence."),
        "evidence:jsonrpc": evidence("evidence:jsonrpc", "source:jsonrpc-2", "adopted_from_standard", "JSON-RPC request/result/error interoperability evidence."),
        "evidence:otel": evidence("evidence:otel", "source:otel-semconv-1.43", "canonical", "OpenTelemetry runtime/RPC telemetry interoperability evidence."),
        "evidence:otel-genai": evidence("evidence:otel-genai", "source:otel-genai", "canonical", "OpenTelemetry GenAI agent/tool telemetry interoperability evidence."),
        "evidence:artist-gortnite-events": evidence("evidence:artist-gortnite-events", "source:artist-gortnite-event-schema", "canonical", "Exact Artist Gortnite session/event implementation semantics."),
        "evidence:artist-gortnite-tools": evidence("evidence:artist-gortnite-tools", "source:artist-gortnite-tool-registry", "canonical", "Exact Artist Gortnite built-in tool registry semantics."),
        "evidence:artist-gortnite-capture": evidence("evidence:artist-gortnite-capture", "source:artist-gortnite-tool-capture", "canonical", "Exact Artist Gortnite tool-result correlation/outcome capture semantics."),
        "evidence:muse-design": evidence("evidence:muse-design", "source:muse-design", "expert_asserted", "Muse-authored identity and safety distinctions."),
    }
    return {
        "header": header(
            "muse.foundation.provenance", "provenance", "Muse software/agent foundation provenance",
            "Pinned source and design evidence for software, computing, software-agent, and coding-harness ontology packages.",
        ),
        "sources": dict(sorted(sources.items())),
        "agents": {},
        "activities": {},
        "evidence": dict(sorted(ev.items())),
        "assertions": {},
    }


def software_package(prov_ref, ufo_a, ufo_b, ufo_c):
    E_SWO = ["evidence:seon-swo"]
    E_CPO = ["evidence:seon-cpo"]
    E_REQ = ["evidence:seon-rsro"]
    E_TEST = ["evidence:seon-roost"]
    E_CM = ["evidence:seon-cmpo"]
    E_SP = ["evidence:spdx", "evidence:muse-design"]
    c = {}
    def add(cid, label, text, parents, ev=E_SWO, **kw): c[cid] = concept(cid, label, text, parents, ev, **kw)
    add("se:Artifact", "artifact", "An object intentionally made or maintained to serve a purpose in software work.", ["ufo:Object"], E_SP)
    add("se:InformationArtifact", "information artifact", "An artifact whose relevant identity is tied to information content rather than merely to one physical or digital bearer.", ["se:Artifact"], E_SP)
    add("se:ArtifactBearer", "artifact bearer", "A concrete artifact that bears or realizes an information artifact while remaining distinct from its information content.", ["se:Artifact"], E_SP)
    add("se:DigitalArtifact", "digital artifact", "An artifact bearer whose relevant realization is stored or transmitted digitally.", ["se:ArtifactBearer"], E_SP)
    add("se:SoftwareArtifact", "software artifact", "An artifact produced, used, or maintained as part of software engineering or software execution.", ["se:Artifact"], E_SWO)
    add("se:SoftwareInformationArtifact", "software information artifact", "A software artifact whose relevant identity is informational content.", ["se:SoftwareArtifact", "se:InformationArtifact"], E_SWO)
    add("se:SoftwareProduct", "software product", "A software artifact delivered as a named usable product and potentially constituted by programs and auxiliary artifacts.", ["se:SoftwareArtifact"], E_SWO)
    add("se:Program", "program", "A software information artifact intended to produce a specified result through execution; it is constituted by code but is not identical to that code.", ["se:SoftwareInformationArtifact"], E_SWO)
    add("se:Code", "code", "A software information artifact representing computer instructions and data definitions.", ["se:SoftwareInformationArtifact"], E_SWO)
    add("se:SourceCode", "source code", "Code expressed in a programming language in a form suitable as input to an assembler, compiler, interpreter, or other translator/executor.", ["se:Code"], E_SWO, disjoint=["se:MachineCode"])
    add("se:MachineCode", "machine code", "Code expressed in a machine-recognizable form produced for direct processing by a computer system.", ["se:Code"], E_SWO, disjoint=["se:SourceCode"])
    add("se:SourceCodeArtifact", "source-code artifact", "A digital artifact bearer that realizes or stores source-code information content.", ["se:DigitalArtifact", "se:SoftwareArtifact"], E_SP)
    add("se:SoftwareSpecification", "software specification", "An information artifact specifying required or intended properties of software.", ["se:InformationArtifact"], E_SWO)
    add("se:ProgramSpecification", "program specification", "A software specification describing a program's intended purpose, structure, or functions sufficiently to constrain its identity and implementation.", ["se:SoftwareSpecification"], E_SWO)
    add("se:Requirement", "software requirement", "A proposition expressing a required property, behavior, constraint, or quality of software or its development.", ["ufo:Proposition"], E_REQ, prof=profile(abstractness="abstract"))
    add("se:RequirementSpecification", "requirements specification", "An information artifact that records, organizes, or communicates software requirements.", ["se:InformationArtifact"], E_REQ)
    add("se:Configuration", "software configuration", "An information artifact specifying selected values, components, or dependencies that constrain software construction or execution.", ["se:InformationArtifact"], E_CM)
    add("se:DependencySpecification", "dependency specification", "An information artifact specifying a software dependency and any version, feature, platform, or source constraints attached to it.", ["se:InformationArtifact"], ["evidence:codemeta", "evidence:spdx"])
    add("se:BuildArtifact", "build artifact", "A software artifact produced as an output of a software build or compilation activity.", ["se:SoftwareArtifact"], ["evidence:spdx", "evidence:seon-swo"])
    add("se:TestArtifact", "test artifact", "An artifact used or produced in software testing, including test cases, fixtures, reports, and generated test outputs.", ["se:Artifact"], E_TEST)
    add("se:Diagnostic", "software diagnostic", "An information artifact reporting a detected software condition, defect, warning, error, or analysis result.", ["se:InformationArtifact"], ["evidence:seon-roost", "evidence:muse-design"])
    add("se:DefectSituation", "defect situation", "A situation in which software violates or fails to satisfy a relevant expected property or requirement.", ["ufo:Situation"], ["evidence:seon-roost", "evidence:muse-design"])
    add("se:SoftwareDevelopmentActivity", "software-development activity", "An intentional action performed as part of developing or maintaining software.", ["ufo:Action"], E_CPO)
    add("se:CodingActivity", "coding activity", "A software-development activity that creates, changes, documents, or reviews software code.", ["se:SoftwareDevelopmentActivity"], E_CPO)
    add("se:BuildActivity", "build activity", "A software-development activity that transforms software inputs into build artifacts using build tools and configuration.", ["se:SoftwareDevelopmentActivity"], ["evidence:spdx", "evidence:muse-design"])
    add("se:CompilationActivity", "compilation activity", "A build activity that translates source or intermediate code into another executable or intermediate code form.", ["se:BuildActivity"], E_SWO)
    add("se:TestingActivity", "testing activity", "A software-development activity that evaluates software against test conditions or expected behavior and produces test observations or artifacts.", ["se:SoftwareDevelopmentActivity"], E_TEST)
    add("se:ConfigurationManagementActivity", "configuration-management activity", "A software-development activity that identifies, controls, records, or changes software configuration items and versions.", ["se:SoftwareDevelopmentActivity"], E_CM)

    r = {}
    def rel(rid, label, text, dom, rng, ev=E_SP, **kw): r[rid] = relation(rid, label, text, dom, rng, ev, **kw)
    rel("se:bearsInformation", "bears information", "Relates an artifact bearer to information content it realizes or stores without identifying the bearer with that content.", ["se:ArtifactBearer"], ["se:InformationArtifact"], E_SP)
    rel("se:constitutedByCode", "constituted by code", "Relates a program to code that constitutes an implementation of that program while preserving distinct identities.", ["se:Program"], ["se:Code"], E_SWO)
    rel("se:specifiedBy", "specified by", "Relates software to a specification that constrains its intended identity or behavior.", ["se:SoftwareArtifact"], ["se:SoftwareSpecification"], E_SWO)
    rel("se:recordsRequirement", "records requirement", "Relates a requirements specification to a requirement proposition it records.", ["se:RequirementSpecification"], ["se:Requirement"], E_REQ)
    rel("se:encodesSourceCode", "encodes source code", "Relates a source-code artifact bearer to the source-code information content it realizes.", ["se:SourceCodeArtifact"], ["se:SourceCode"], E_SP)
    rel("se:dependsOn", "depends on", "Relates a software artifact to another software artifact required for its construction, execution, or intended behavior.", ["se:SoftwareArtifact"], ["se:SoftwareArtifact"], ["evidence:codemeta", "evidence:spdx"])
    rel("se:softwareConfiguredBy", "software configured by", "Relates a software artifact to configuration information that constrains its construction or execution.", ["se:SoftwareArtifact"], ["se:Configuration"], E_CM)
    rel("se:activityConfiguredBy", "activity configured by", "Relates a software-development activity to configuration information that constrains how the activity is performed.", ["se:SoftwareDevelopmentActivity"], ["se:Configuration"], E_CM)
    rel("se:usesArtifact", "uses artifact", "Relates a software-development activity to an artifact it consumes, consults, or operates upon.", ["se:SoftwareDevelopmentActivity"], ["se:Artifact"], E_CPO)
    rel("se:createsArtifact", "creates artifact", "Relates a software-development activity to an artifact whose relevant identity begins as a result of that activity.", ["se:SoftwareDevelopmentActivity"], ["se:Artifact"], E_CPO)
    rel("se:modifiesArtifact", "modifies artifact", "Relates a software-development activity to a pre-existing artifact whose state/content it changes.", ["se:SoftwareDevelopmentActivity"], ["se:Artifact"], E_CPO)
    rel("se:testsArtifact", "tests artifact", "Relates a testing activity to the software artifact under test.", ["se:TestingActivity"], ["se:SoftwareArtifact"], E_TEST)
    rel("se:reportsDefect", "reports defect", "Relates a diagnostic to a defect situation it reports or describes.", ["se:Diagnostic"], ["se:DefectSituation"], E_TEST)
    rel("se:producedBy", "produced by", "Relates an artifact to the software-development activity that produced the represented artifact occurrence/version.", ["se:Artifact"], ["se:SoftwareDevelopmentActivity"], ["evidence:prov-o", "evidence:muse-design"])
    return ontology(
        "muse.software", "Muse software engineering ontology",
        "UFO-grounded software artifacts, specifications, requirements, and development activities for Muse.",
        [exact(prov_ref), exact(ufo_a), exact(ufo_b), exact(ufo_c)],
        sorted(set(x for v in c.values() for x in v["evidence"]) | set(x for v in r.values() for x in v["evidence"])),
        c, r,
    )


def computing_package(prov_ref, software_ref, ufo_a, ufo_b):
    E = ["evidence:seon-rro", "evidence:muse-design"]
    c = {}
    def add(cid, label, text, parents, ev=E, **kw): c[cid] = concept(cid, label, text, parents, ev, **kw)
    add("comp:ComputerSystem", "computer system", "A physical or virtual system capable of executing programs and maintaining computational state.", ["ufo:Object"])
    add("comp:ExecutionEnvironment", "execution environment", "A situation/configured environment providing the runtime conditions under which software is executed.", ["ufo:Situation"])
    add("comp:OperatingSystem", "operating system", "A software system that manages computer resources and provides execution services to programs.", ["se:SoftwareProduct"])
    add("comp:ProcessExecution", "process execution", "A temporally extended occurrence of executing a program or command under a computer execution environment.", ["ufo:Event"])
    add("comp:ProgramInvocation", "program invocation", "A process-execution occurrence that requests execution of a particular program with supplied invocation parameters.", ["comp:ProcessExecution"])
    add("comp:Command", "command", "An information artifact specifying an operation to be invoked, including command text or structured method identity distinct from its execution.", ["se:InformationArtifact"])
    add("comp:CommandInvocation", "command invocation", "A process-execution occurrence initiated from a command specification.", ["comp:ProcessExecution"])
    add("comp:DataValue", "data value", "An abstract scalar or structured data value used as exact computational input, output, status, or metadata content.", ["ufo:AbstractIndividual"])
    add("comp:StringValue", "string value", "A data value whose exact computational representation is a character string.", ["comp:DataValue"])
    add("comp:NumericValue", "numeric value", "A data value with mathematical numeric denotation; transport spelling is not part of its semantic identity.", ["comp:DataValue"])
    add("comp:IntegerValue", "integer value", "A numeric value denoting an integer.", ["comp:NumericValue"])
    add("comp:DecimalValue", "decimal value", "A numeric value denoting a finite decimal/rational decimal quantity.", ["comp:NumericValue"])
    add("comp:RatioValue", "ratio value", "A numeric value represented as an unevaluated ratio of a numerator numeric value to a nonzero denominator numeric value.", ["comp:NumericValue"])
    add("comp:PercentageValue", "percentage value", "A ratio value explicitly presented as a percentage.", ["comp:RatioValue"])
    add("comp:ApproximateNumericValue", "approximate numeric value", "A numeric value explicitly presented as approximate rather than exact.", ["comp:NumericValue"])
    add("comp:NumericInterval", "numeric interval", "A numeric value denoting a source-specified interval with explicit lower and upper numeric bounds.", ["comp:NumericValue"])
    add("comp:PluralScaleValue", "plural scale value", "A vague numeric scale expression such as thousands or millions that denotes an order/scale without asserting one exact cardinality.", ["comp:NumericValue"])
    add("comp:ArithmeticExpressionValue", "arithmetic expression value", "A numeric value represented by an unevaluated arithmetic expression whose operands and operation are source explicit.", ["comp:NumericValue"])
    add("comp:AdditionExpressionValue", "addition expression value", "An arithmetic expression value formed by source-explicit addition.", ["comp:ArithmeticExpressionValue"])
    add("comp:SubtractionExpressionValue", "subtraction expression value", "An arithmetic expression value formed by source-explicit subtraction.", ["comp:ArithmeticExpressionValue"])
    add("comp:MultiplicationExpressionValue", "multiplication expression value", "An arithmetic expression value formed by source-explicit multiplication.", ["comp:ArithmeticExpressionValue"])
    add("comp:DivisionExpressionValue", "division expression value", "An arithmetic expression value formed by source-explicit division.", ["comp:ArithmeticExpressionValue"])
    add("comp:MeasurementValue", "measurement value", "A quale carrying an explicit numeric magnitude and unit of measure.", ["ufo:Quale"])
    add("comp:UnitOfMeasure", "unit of measure", "An abstract unit used to interpret an explicitly measured magnitude.", ["ufo:AbstractIndividual"])
    add("comp:BooleanValue", "boolean value", "A data value representing one of the two boolean truth values.", ["comp:DataValue"])
    add("comp:NullValue", "null value", "A distinguished data value representing an explicit null/no-value token in a structured protocol.", ["comp:DataValue"])
    add("comp:StructuredDataValue", "structured data value", "A data value representing an ordered array or key/value object preserved in canonical structured form.", ["comp:DataValue"])
    add("comp:Argument", "invocation argument", "An information artifact/value supplied as an input parameter to a program, command, or tool invocation.", ["se:InformationArtifact"])
    add("comp:EnvironmentBinding", "environment binding", "An information artifact binding an environment variable/name to a value for an execution context.", ["se:InformationArtifact"])
    add("comp:FilesystemObject", "filesystem object", "A digitally represented object addressable through a filesystem namespace.", ["se:DigitalArtifact"])
    add("comp:File", "file", "A filesystem object that stores a sequence of content bytes or equivalent file content.", ["comp:FilesystemObject"], disjoint=["comp:Directory"])
    add("comp:Directory", "directory", "A filesystem object that organizes named filesystem entries.", ["comp:FilesystemObject"], disjoint=["comp:File"])
    add("comp:Path", "filesystem path", "An information artifact denoting or attempting to denote a filesystem location through a path expression.", ["se:InformationArtifact"])
    add("comp:ArtifactState", "artifact state", "A situation representing source-established state/content metadata of an artifact at a specified point, interval, or event-relative role; it is distinct from the artifact's enduring identity.", ["ufo:Situation"])
    add("comp:FileState", "file state", "An artifact state representing the content/metadata state of a file at a specified point or interval.", ["comp:ArtifactState"])
    add("comp:FilesystemOperation", "filesystem operation", "An action that reads, creates, changes, relocates, copies, or deletes filesystem state.", ["ufo:Action"])
    add("comp:FileRead", "file read", "A filesystem operation that observes file content or metadata without itself entailing a write.", ["comp:FilesystemOperation"])
    add("comp:FileWrite", "file write", "A filesystem operation that requests or performs a change to file content or metadata.", ["comp:FilesystemOperation"])
    add("comp:FileCreate", "file creation", "A filesystem operation that creates a new file identity or filesystem entry.", ["comp:FilesystemOperation"])
    add("comp:FileDelete", "file deletion", "A filesystem operation that removes a filesystem entry or makes the referenced object no longer present at that path.", ["comp:FilesystemOperation"])
    add("comp:FileMove", "file move", "A filesystem operation that changes the filesystem location/name of a file or directory while preserving the moved object's intended identity where supported.", ["comp:FilesystemOperation"])
    add("comp:FileCopy", "file copy", "A filesystem operation that creates a distinct filesystem object whose initial content is derived from another object.", ["comp:FilesystemOperation"])
    add("comp:StreamArtifact", "execution stream artifact", "An information artifact emitted to or consumed from a process stream.", ["se:InformationArtifact"])
    add("comp:StandardInput", "standard input", "A stream artifact supplied through an invocation's standard-input channel.", ["comp:StreamArtifact"])
    add("comp:StandardOutput", "standard output", "A stream artifact emitted through an invocation's standard-output channel.", ["comp:StreamArtifact"])
    add("comp:StandardError", "standard error", "A stream artifact emitted through an invocation's standard-error channel.", ["comp:StreamArtifact"])
    add("comp:ExitStatus", "exit status", "An information artifact/value reported when a process execution terminates, distinct from independent verification of requested external effects.", ["se:InformationArtifact"])
    add("comp:ExecutionOutcome", "execution outcome", "A situation/status attributed to an execution occurrence by an observed or reported result.", ["ufo:Situation"])
    add("comp:SuccessOutcome", "success outcome", "An execution outcome reported as successful with respect to the execution protocol's success criterion.", ["comp:ExecutionOutcome"], disjoint=["comp:FailureOutcome"])
    add("comp:FailureOutcome", "failure outcome", "An execution outcome reported as failed with respect to the execution protocol's success criterion.", ["comp:ExecutionOutcome"], disjoint=["comp:SuccessOutcome"])
    add("comp:PartialOutcome", "partial outcome", "An execution outcome reporting that only part of the requested or attempted operation completed.", ["comp:ExecutionOutcome"])
    add("comp:SkippedOutcome", "skipped outcome", "An execution outcome reporting that an operation was not attempted because the source protocol skipped it.", ["comp:ExecutionOutcome"])
    add("comp:DeniedOutcome", "denied outcome", "An execution outcome reporting that execution/effect was denied by policy, permissions, or an explicit gate.", ["comp:ExecutionOutcome"])
    add("comp:CancelledOutcome", "cancelled outcome", "An execution outcome reporting that an operation was cancelled before normal completion.", ["comp:ExecutionOutcome"])
    add("comp:InterruptedOutcome", "interrupted outcome", "An execution outcome reporting that an operation began but was interrupted before normal completion.", ["comp:ExecutionOutcome"])
    add("comp:UnknownOutcome", "unknown outcome", "An execution outcome token whose protocol-level status is preserved without assigning a stronger success/failure interpretation.", ["comp:ExecutionOutcome"])
    add("comp:ResourceUsage", "resource usage record", "An information artifact recording resource consumption associated with an execution occurrence.", ["se:InformationArtifact"], ["evidence:otel", "evidence:muse-design"])
    add("comp:DiagnosticEmission", "diagnostic emission", "An event in which an executing program or tool emits a diagnostic information artifact.", ["ufo:Event"])

    r = {}
    def rel(rid, label, text, dom, rng, ev=E, **kw): r[rid] = relation(rid, label, text, dom, rng, ev, **kw)
    rel("comp:runsOn", "runs on", "Relates a process execution to the computer system on which it executes.", ["comp:ProcessExecution"], ["comp:ComputerSystem"])
    rel("comp:underEnvironment", "under environment", "Relates a process execution to its execution environment.", ["comp:ProcessExecution"], ["comp:ExecutionEnvironment"])
    rel("comp:invokesProgram", "invokes program", "Relates a program invocation to the program whose execution it requests.", ["comp:ProgramInvocation"], ["se:Program"])
    rel("comp:invokesCommand", "invokes command", "Relates a command invocation to the command information artifact that specified it.", ["comp:CommandInvocation"], ["comp:Command"])
    rel("comp:hasArgument", "has argument", "Relates an invocation occurrence to an explicitly supplied invocation argument.", ["comp:ProcessExecution"], ["comp:Argument"])
    rel("comp:argumentName", "argument name", "Relates an invocation-argument artifact to its source-level parameter name; the object is represented as a literal value in occurrence documents.", ["comp:Argument"], ["comp:StringValue"])
    rel("comp:argumentValue", "argument value", "Relates an invocation-argument artifact to its losslessly preserved source-level value; the object is represented as a literal value in occurrence documents.", ["comp:Argument"], ["comp:DataValue"])
    rel("comp:ratioNumerator", "ratio numerator", "Relates a ratio value to its source-grounded numerator.", ["comp:RatioValue"], ["comp:NumericValue"])
    rel("comp:ratioDenominator", "ratio denominator", "Relates a ratio value to its source-grounded nonzero denominator.", ["comp:RatioValue"], ["comp:NumericValue"])
    rel("comp:approximateNumericValue", "approximate numeric value", "Relates an explicit approximation value to the exact numeric value around which the source expresses approximation.", ["comp:ApproximateNumericValue"], ["comp:NumericValue"])
    rel("comp:intervalLowerBound", "interval lower bound", "Relates a numeric interval to its lower bound.", ["comp:NumericInterval"], ["comp:NumericValue"])
    rel("comp:intervalUpperBound", "interval upper bound", "Relates a numeric interval to its upper bound.", ["comp:NumericInterval"], ["comp:NumericValue"])
    rel("comp:pluralScaleBase", "plural scale base", "Relates a vague plural-scale value to its source-grounded numeric scale base.", ["comp:PluralScaleValue"], ["comp:NumericValue"])
    rel("comp:arithmeticLeftOperand", "arithmetic left operand", "Relates an arithmetic expression value to its left operand.", ["comp:ArithmeticExpressionValue"], ["comp:NumericValue"])
    rel("comp:arithmeticRightOperand", "arithmetic right operand", "Relates an arithmetic expression value to its right operand.", ["comp:ArithmeticExpressionValue"], ["comp:NumericValue"])
    rel("comp:measurementMagnitude", "measurement magnitude", "Relates a measurement quale to its explicit numeric magnitude.", ["comp:MeasurementValue"], ["comp:NumericValue"])
    rel("comp:measurementUnit", "measurement unit", "Relates a measurement quale to its explicit unit of measure.", ["comp:MeasurementValue"], ["comp:UnitOfMeasure"])
    rel("comp:environmentHasBinding", "environment has binding", "Relates an execution environment to one of its explicit environment bindings.", ["comp:ExecutionEnvironment"], ["comp:EnvironmentBinding"])
    rel("comp:invocationHasEnvironmentBinding", "invocation has environment binding", "Relates a process execution to an environment binding explicitly supplied for that invocation.", ["comp:ProcessExecution"], ["comp:EnvironmentBinding"])
    rel("comp:workingDirectory", "has working directory", "Relates a process execution to the directory used as its working directory.", ["comp:ProcessExecution"], ["comp:Directory"])
    rel("comp:pathDenotes", "path denotes", "Relates a filesystem path expression to the filesystem object it denotes in a particular resolved context.", ["comp:Path"], ["comp:FilesystemObject"])
    rel("comp:hasFileState", "has file state", "Relates a file to a represented state/version of that file.", ["comp:File"], ["comp:FileState"])
    rel("comp:stateOfArtifact", "state of artifact", "Relates an artifact-state situation to the artifact whose state it represents.", ["comp:ArtifactState"], ["se:Artifact"])
    rel("comp:stateDigest", "state digest", "Relates an artifact-state situation to an exact source-reported content/state digest represented as a string data value.", ["comp:ArtifactState"], ["comp:StringValue"])
    rel("comp:beforeArtifactState", "before artifact state", "Relates an event to an artifact state explicitly identified as preceding that event/effect.", ["ufo:Event"], ["comp:ArtifactState"])
    rel("comp:afterArtifactState", "after artifact state", "Relates an event to an artifact state explicitly identified as following/resulting from that event/effect.", ["ufo:Event"], ["comp:ArtifactState"])
    rel("comp:inputArtifactState", "input artifact state", "Relates an event to an artifact state explicitly used as an input state.", ["ufo:Event"], ["comp:ArtifactState"])
    rel("comp:outputArtifactState", "output artifact state", "Relates an event to an artifact state explicitly identified as an output state.", ["ufo:Event"], ["comp:ArtifactState"])
    rel("comp:observedArtifactState", "observed artifact state", "Relates an event to an artifact state observed in association with that event when no narrower before/after/input/output role is established.", ["ufo:Event"], ["comp:ArtifactState"])
    rel("comp:readsObject", "reads object", "Relates a filesystem read operation to the filesystem object it reads when object identity is established.", ["comp:FileRead"], ["comp:FilesystemObject"])
    rel("comp:readPath", "reads path", "Relates a filesystem read operation to the path expression it attempts/resolves for the read without asserting filesystem-object existence merely from the path string.", ["comp:FileRead"], ["comp:Path"])
    rel("comp:writesObject", "writes object", "Relates a filesystem write operation to the filesystem object whose state it attempts or performs a change upon when object identity is established.", ["comp:FileWrite"], ["comp:FilesystemObject"])
    rel("comp:writePath", "writes path", "Relates a filesystem write operation to the path expression it targets without asserting that a filesystem object already exists at that path.", ["comp:FileWrite"], ["comp:Path"])
    rel("comp:createsObject", "creates object", "Relates a creation operation to the filesystem object created when creation is observed/established.", ["comp:FileCreate"], ["comp:FilesystemObject"])
    rel("comp:createPath", "creates at path", "Relates a file-creation operation to its requested/resolved destination path without by itself asserting successful object creation.", ["comp:FileCreate"], ["comp:Path"])
    rel("comp:deletesObject", "deletes object", "Relates a deletion operation to a filesystem object whose identity is established as its target.", ["comp:FileDelete"], ["comp:FilesystemObject"])
    rel("comp:deletePath", "deletes path", "Relates a deletion operation to the path expression it targets without asserting that an object exists at the path.", ["comp:FileDelete"], ["comp:Path"])
    rel("comp:moveSource", "move source object", "Relates a move operation to its source filesystem object when source-object identity is established.", ["comp:FileMove"], ["comp:FilesystemObject"])
    rel("comp:moveSourcePath", "move source path", "Relates a move operation to the path expression supplied/resolved as its source.", ["comp:FileMove"], ["comp:Path"])
    rel("comp:moveDestinationPath", "move destination path", "Relates a move operation to the path expression supplied or resolved as its destination.", ["comp:FileMove"], ["comp:Path"])
    rel("comp:moveDestinationObject", "move destination object", "Relates a move operation to the filesystem object established or observed at its destination.", ["comp:FileMove"], ["comp:FilesystemObject"])
    rel("comp:copySource", "copy source object", "Relates a copy operation to the filesystem object whose content/state is used as source when source-object identity is established.", ["comp:FileCopy"], ["comp:FilesystemObject"])
    rel("comp:copySourcePath", "copy source path", "Relates a copy operation to the path expression supplied/resolved as its source.", ["comp:FileCopy"], ["comp:Path"])
    rel("comp:copyDestinationPath", "copy destination path", "Relates a copy operation to the path expression supplied or resolved as its destination.", ["comp:FileCopy"], ["comp:Path"])
    rel("comp:copyDestinationObject", "copy destination object", "Relates a copy operation to the distinct filesystem object established or observed as the copy destination.", ["comp:FileCopy"], ["comp:FilesystemObject"])
    rel("comp:hasStandardInput", "has standard input", "Relates a process execution to standard-input content supplied to it.", ["comp:ProcessExecution"], ["comp:StandardInput"])
    rel("comp:producesStandardOutput", "produces standard output", "Relates a process execution to standard-output content it emits.", ["comp:ProcessExecution"], ["comp:StandardOutput"])
    rel("comp:producesStandardError", "produces standard error", "Relates a process execution to standard-error content it emits.", ["comp:ProcessExecution"], ["comp:StandardError"])
    rel("comp:hasExitStatus", "has exit status", "Relates a completed process execution to its reported exit status.", ["comp:ProcessExecution"], ["comp:ExitStatus"])
    rel("comp:exitCodeValue", "exit-code value", "Relates an exit-status artifact to the exact source-reported numeric exit code represented as a literal.", ["comp:ExitStatus"], ["comp:IntegerValue"])
    rel("comp:streamContent", "stream content", "Relates an execution-stream artifact to exact captured stream content represented as a literal.", ["comp:StreamArtifact"], ["comp:StringValue"])
    rel("comp:hasReportedOutcome", "has reported outcome", "Relates an execution occurrence to an outcome status explicitly reported by its source/protocol.", ["comp:ProcessExecution"], ["comp:ExecutionOutcome"])
    rel("comp:emitsDiagnostic", "emits diagnostic", "Relates a diagnostic-emission event to the diagnostic information artifact emitted.", ["comp:DiagnosticEmission"], ["se:Diagnostic"])
    rel("comp:resourceUsageOf", "resource usage of", "Relates a resource-usage record to the execution occurrence whose consumption it reports.", ["comp:ResourceUsage"], ["comp:ProcessExecution"], ["evidence:otel"])
    rel("comp:durationMillis", "duration milliseconds", "Relates a process execution to a source-reported elapsed duration in milliseconds represented as an integer data value.", ["comp:ProcessExecution"], ["comp:IntegerValue"], ["evidence:otel"])
    return ontology(
        "muse.computing", "Muse computing ontology",
        "Execution, command, process, filesystem, stream, status, and resource concepts for deterministic tool and runtime formalization.",
        [exact(prov_ref), exact(software_ref), exact(ufo_a), exact(ufo_b)],
        sorted(set(x for v in c.values() for x in v["evidence"]) | set(x for v in r.values() for x in v["evidence"])),
        c, r,
    )


def semantic_package(prov_ref, computing_ref, ufo_a, ufo_c):
    E = ["evidence:muse-design"]
    c = {}
    def add(cid, label, text, parents, **kw): c[cid] = concept(cid, label, text, parents, E, **kw)
    add("sem:SemanticContent", "semantic content", "An abstract content object presented by discourse without requiring that the content itself be truth-valued.", ["ufo:AbstractIndividual"])
    add("sem:InterrogativeContent", "interrogative content", "Semantic content whose discourse force requests resolution of an open or closed question rather than asserting a proposition.", ["sem:SemanticContent"])
    add("sem:PolarQuestionContent", "polar-question content", "Interrogative content asking whether a proposition holds.", ["sem:InterrogativeContent"])
    add("sem:WhQuestionContent", "wh-question content", "Interrogative content binding one or more source-grounded answer variables.", ["sem:InterrogativeContent"])
    add("sem:AlternativeQuestionContent", "alternative-question content", "Interrogative content explicitly offering proposition/content alternatives.", ["sem:InterrogativeContent"])
    add("sem:TagQuestionContent", "tag-question content", "Interrogative content expressed by a source-explicit tag question construction.", ["sem:InterrogativeContent"])
    add("sem:DirectiveContent", "directive content", "Semantic content presented as a directive toward an addressee, distinct from asserting that its proposition already holds.", ["sem:SemanticContent"])
    add("sem:CommandContent", "command content", "Directive content presented as a command.", ["sem:DirectiveContent"])
    add("sem:RequestContent", "request content", "Directive content presented as a request.", ["sem:DirectiveContent"])
    add("sem:SuggestionContent", "suggestion content", "Directive content presented as a suggestion or recommendation.", ["sem:DirectiveContent"])
    add("sem:CommissiveContent", "commissive content", "Semantic content by which the presenter commits to a proposition/action rather than asserting its current truth.", ["sem:SemanticContent"])
    add("sem:PromiseContent", "promise content", "Commissive content presented as a promise.", ["sem:CommissiveContent"])
    add("sem:QuotationContent", "quotation content", "Semantic content that mentions or quotes exact source material without asserting the quoted material.", ["sem:SemanticContent"])
    add("sem:SemanticOperator", "semantic operator", "An abstract source-grounded semantic operator whose lexical identity is preserved when the distinction is not a closed logical connective.", ["ufo:AbstractIndividual"])
    add("sem:PropositionalOperator", "propositional operator", "A semantic operator whose scope is a proposition and whose subtype determines the operator force.", ["sem:SemanticOperator"])
    add("sem:ModalOperator", "modal operator", "A propositional operator contributing modal force without conflating epistemic, deontic, and dynamic modality.", ["sem:PropositionalOperator"])
    add("sem:EpistemicModalOperator", "epistemic modal operator", "A modal operator concerning support, likelihood, or necessity relative to information or belief.", ["sem:ModalOperator"])
    add("sem:EpistemicPossibilityOperator", "epistemic possibility operator", "Epistemic possibility, as in a source-grounded maybe/might reading.", ["sem:EpistemicModalOperator"])
    add("sem:EpistemicProbabilityOperator", "epistemic probability operator", "Epistemic probability or likelihood, as in a source-grounded probably/likely reading.", ["sem:EpistemicModalOperator"])
    add("sem:EpistemicNecessityOperator", "epistemic necessity operator", "Epistemic necessity, as in a source-grounded must/cannot reading supported by information or inference.", ["sem:EpistemicModalOperator"])
    add("sem:EpistemicExpectationOperator", "epistemic expectation operator", "Expectedness weaker than necessity, including source-grounded predictive should readings.", ["sem:EpistemicModalOperator"])
    add("sem:DeonticModalOperator", "deontic modal operator", "A modal operator concerning permission, obligation, prohibition, or normative advisability.", ["sem:ModalOperator"])
    add("sem:PermissionOperator", "permission operator", "Deontic permission.", ["sem:DeonticModalOperator"])
    add("sem:ObligationOperator", "obligation operator", "Deontic obligation or requirement.", ["sem:DeonticModalOperator"])
    add("sem:ProhibitionOperator", "prohibition operator", "Deontic prohibition.", ["sem:DeonticModalOperator"])
    add("sem:AdvisabilityOperator", "advisability operator", "Normative recommendation or advisability, including source-grounded advisory should readings.", ["sem:DeonticModalOperator"])
    add("sem:DynamicModalOperator", "dynamic modal operator", "A modal operator grounded in properties or circumstances of a participant rather than information or norms.", ["sem:ModalOperator"])
    add("sem:CapabilityOperator", "capability operator", "Dynamic modality expressing a participant's capability to realize the scoped proposition.", ["sem:DynamicModalOperator"])
    add("sem:EpistemicQualificationOperator", "epistemic qualification operator", "An epistemic operator explicitly limiting an assertion to an information state, as in 'as far as we know'.", ["sem:EpistemicModalOperator"])
    add("sem:EpistemicCertaintyOperator", "epistemic certainty operator", "An epistemic operator expressing source-grounded certainty or confidence without collapsing it to logical necessity.", ["sem:EpistemicModalOperator"])
    add("sem:GenericOperator", "generic operator", "A propositional operator contributing generic or habitual force, distinct from universal quantification.", ["sem:PropositionalOperator"])
    add("sem:PerfectAspectOperator", "perfect aspect operator", "A propositional operator contributing source-explicit perfect aspect.", ["sem:PropositionalOperator"])
    add("sem:ProgressiveAspectOperator", "progressive aspect operator", "A propositional operator contributing source-explicit progressive aspect.", ["sem:PropositionalOperator"])
    add("sem:TemporalAspectOperator", "temporal aspect operator", "A source-grounded propositional operator contributing temporal/aspectual meaning beyond tense.", ["sem:PropositionalOperator"])
    add("sem:AlreadyOperator", "already operator", "Temporal/aspectual operator contributed by a source-explicit 'already' reading without inventing an unexpressed presupposition formula.", ["sem:TemporalAspectOperator"])
    add("sem:StillOperator", "still operator", "Temporal/aspectual continuation operator contributed by source-explicit 'still'.", ["sem:TemporalAspectOperator"])
    add("sem:AgainOperator", "again operator", "Repetition/restoration operator contributed by source-explicit 'again'.", ["sem:TemporalAspectOperator"])
    add("sem:OptimizationOperator", "optimization operator", "A propositional operator requiring source-explicit maximization or minimization relative to an expressed target, distinct from ordinary comparison or modality.", ["sem:PropositionalOperator"])
    add("sem:MaximizationOperator", "maximization operator", "Optimization force such as source-explicit 'as many as possible' or 'as high ... as possible'.", ["sem:OptimizationOperator"])
    add("sem:MinimizationOperator", "minimization operator", "Optimization force such as source-explicit 'as few as possible' or 'as low ... as possible'.", ["sem:OptimizationOperator"])
    add("sem:GeneralizedQuantifier", "generalized quantifier", "A source-grounded quantifier operator whose force is not one of the closed exact logical/cardinality operators.", ["sem:SemanticOperator"])
    add("sem:MostQuantifier", "most quantifier", "Generalized quantifier expressing most of the source-grounded domain.", ["sem:GeneralizedQuantifier"])
    add("sem:ManyQuantifier", "many quantifier", "Context-sensitive generalized quantifier expressing many domain members.", ["sem:GeneralizedQuantifier"])
    add("sem:FewQuantifier", "few quantifier", "Context-sensitive generalized quantifier expressing few domain members.", ["sem:GeneralizedQuantifier"])
    add("sem:SeveralQuantifier", "several quantifier", "Context-sensitive generalized quantifier expressing several domain members.", ["sem:GeneralizedQuantifier"])
    add("sem:FocusOperator", "focus operator", "A source-grounded operator affecting focus alternatives without being reduced to cardinality.", ["sem:SemanticOperator"])
    add("sem:ExclusiveFocusOperator", "exclusive focus operator", "Focus operator expressing exclusion, including source-grounded only/merely/just readings.", ["sem:FocusOperator"])
    add("sem:AdditiveFocusOperator", "additive focus operator", "Focus operator adding the focused alternative, including source-grounded also/too readings.", ["sem:FocusOperator"])
    add("sem:ScalarFocusOperator", "scalar focus operator", "Scalar focus operator such as source-grounded even.", ["sem:FocusOperator"])
    r = {}
    def rel(rid, label, text, dom, rng, **kw): r[rid] = relation(rid, label, text, dom, rng, E, **kw)
    rel("sem:contentProposition", "content proposition", "Relates semantic content to a proposition constituting its propositional body without asserting that proposition.", ["sem:SemanticContent"], ["ufo:Proposition"])
    rel("sem:contentPresenter", "content presenter", "Relates semantic content to a source-established presenter/sender without asserting the content proposition.", ["sem:SemanticContent"], ["ufo:Entity"])
    rel("sem:contentAddressee", "content addressee", "Relates semantic content to a source-established addressee/recipient without asserting the content proposition.", ["sem:SemanticContent"], ["ufo:Entity"])
    rel("sem:alternativeProposition", "alternative proposition", "Relates alternative-question content to one explicitly offered proposition alternative.", ["sem:AlternativeQuestionContent"], ["ufo:Proposition"])
    rel("sem:questionAnswerVariable", "question answer variable", "Relates interrogative content to a source-grounded typed variable whose value resolves part of the question. The variable carries its ontology domain independently.", ["sem:InterrogativeContent"], ["ufo:Entity"])
    rel("sem:quotedText", "quoted text", "Relates quotation/mention content to the exact source text value it quotes or mentions without asserting that text as a proposition.", ["sem:QuotationContent"], ["comp:StringValue"])
    rel("sem:operatorBearer", "operator bearer", "Relates a scoped operator to an explicitly expressed bearer/subject when the operator's force is participant-relative, such as capability or obligation.", ["sem:PropositionalOperator"], ["ufo:Entity"])
    rel("sem:optimizationTarget", "optimization target", "Relates an optimization operator to the source-grounded entity, variable, quality, or value dimension being maximized or minimized.", ["sem:OptimizationOperator"], ["ufo:Entity"])
    rel("sem:epistemicSource", "epistemic source", "Relates an epistemic operator to an explicitly expressed agent or source whose information state supplies the modal basis.", ["sem:EpistemicModalOperator"], ["ufo:Entity"])
    rel("sem:explains", "explains", "Relates a proposition that supplies a source-explicit explanation or reason to the proposition it explains, without collapsing explanation into event causation.", ["ufo:Proposition"], ["ufo:Proposition"])
    rel("sem:agentParticipant", "agent participant", "Relates an event or situation to a participant source-groundedly interpreted as agent/initiator.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:patientParticipant", "patient participant", "Relates an event or situation to a participant source-groundedly interpreted as affected patient.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:themeParticipant", "theme participant", "Relates an event or situation to a participant source-groundedly interpreted as theme.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:experiencerParticipant", "experiencer participant", "Relates an event or situation to a participant source-groundedly interpreted as experiencer.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:stimulusParticipant", "stimulus participant", "Relates an event or situation to a participant source-groundedly interpreted as stimulus.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:recipientParticipant", "recipient participant", "Relates an event or situation to a participant source-groundedly interpreted as recipient.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:beneficiaryParticipant", "beneficiary participant", "Relates an event or situation to a participant source-groundedly interpreted as beneficiary.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:instrumentParticipant", "instrument participant", "Relates an event or situation to a participant source-groundedly interpreted as instrument.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:locationParticipant", "location participant", "Relates an event or situation to an explicitly expressed location participant.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:sourceParticipant", "source participant", "Relates an event or situation to an explicitly expressed source/origin participant.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:goalParticipant", "goal participant", "Relates an event or situation to an explicitly expressed goal/destination participant.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:contentParticipant", "content participant", "Relates an event or situation to explicitly expressed semantic or informational content when no narrower ontology relation applies.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:surfaceSubjectParticipant", "surface-subject participant", "Lowest-precision fallback preserving which participant is realized as the source clause subject when no semantic role is justified.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    rel("sem:surfaceObjectParticipant", "surface-object participant", "Lowest-precision fallback preserving which participant is realized as the source clause object when no narrower semantic role is justified.", ["ufo:Event", "ufo:Situation"], ["ufo:Entity"])
    return ontology(
        "muse.semantic", "Muse semantic-content ontology",
        "Ontology concepts for non-truth-valued discourse content and open semantic operators used by the learned semantic target.",
        [exact(prov_ref), exact(computing_ref), exact(ufo_a), exact(ufo_c)],
        E, c, r,
    )


def agent_package(prov_ref, software_ref, computing_ref, semantic_ref, ufo_b, ufo_c):
    E = ["evidence:fipa", "evidence:muse-design"]
    c = {}
    def add(cid, label, text, parents, ev=E, **kw): c[cid] = concept(cid, label, text, parents, ev, **kw)
    add("agent:ArtificialAgent", "artificial agent", "An agent whose capacity for action is realized through an engineered artificial system.", ["ufo:Agent"])
    add("agent:SoftwareAgent", "software agent", "An artificial agent whose operational realization is software executing in a computing environment.", ["agent:ArtificialAgent"])
    add("agent:ModelBackedAgent", "model-backed agent", "A software agent whose behavior generation materially depends on invocation of one or more learned model artifacts.", ["agent:SoftwareAgent"], ["evidence:muse-design", "evidence:otel-genai"])
    add("agent:ModelArtifact", "model artifact", "A software artifact encoding learned parameters and associated model structure/configuration used for inference.", ["se:SoftwareArtifact"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:LanguageModel", "language model", "A model artifact whose inference interface maps linguistic or tokenized context to distributions or generated linguistic/token content.", ["agent:ModelArtifact"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:ModelInvocation", "model invocation", "An event in which a model artifact is invoked with an input/context to produce model output or an error.", ["ufo:Event"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:AgentRun", "agent run", "A temporally bounded event/interaction episode in which a software agent performs work under a run identity.", ["ufo:Event"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:Task", "agent task", "A plan/goal description specifying work requested or adopted for an agent run.", ["ufo:PlanDescription"], ["evidence:muse-design"])
    add("agent:Subtask", "agent subtask", "A task whose intended execution is subordinate to another task.", ["agent:Task"], ["evidence:muse-design"])
    add("agent:TaskState", "task state", "A source-attributed situation describing the lifecycle/status state of an agent task without implying that the task's planned work has been completed.", ["ufo:Situation"], ["evidence:muse-design"])
    add("agent:Message", "agent message", "An information artifact containing communicative content with source-specified sender/addressee semantics; existence of the artifact does not by itself establish successful delivery.", ["se:InformationArtifact"], E)
    add("agent:MessageDelivery", "message delivery", "A communicative act in which a message artifact is delivered or otherwise communicated to an addressee; a requested send operation does not establish that this act occurred.", ["ufo:CommunicativeAct"], ["evidence:fipa", "evidence:muse-design"])
    add("agent:QuestionPrompt", "question prompt", "An information artifact specifying interrogative content to be presented to an addressee, including structured choice prompts used by agent tools.", ["se:InformationArtifact"], ["evidence:muse-design"])
    add("agent:ChoiceOption", "choice option", "An information artifact representing one explicitly offered candidate answer or action for a question prompt; its presence does not assert that the candidate is true or selected.", ["se:InformationArtifact"], ["evidence:muse-design"])
    add("agent:ConversationalTurn", "conversational turn", "A communicative event contributing one participant's message/content to a conversation sequence.", ["ufo:Event"], E)
    add("agent:Context", "agent context", "An information artifact representing information supplied or retained as context for an agent or model invocation.", ["se:InformationArtifact"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:ContextWindow", "context window", "A bounded context artifact supplied to a specific model invocation, including its ordered model-visible content.", ["agent:Context"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:Tool", "tool", "A software artifact exposed to an agent through a callable operation interface.", ["se:SoftwareArtifact"], ["evidence:otel-genai", "evidence:jsonrpc"])
    add("agent:ToolSpecification", "tool specification", "An information artifact specifying a tool's callable identity, arguments, result/error contract, and relevant semantics.", ["se:SoftwareSpecification"], ["evidence:otel-genai", "evidence:jsonrpc"])
    add("agent:ToolInvocation", "tool invocation", "An event in which an agent/harness requests execution of a tool operation with structured arguments.", ["ufo:Event"], ["evidence:otel-genai", "evidence:jsonrpc"])
    add("agent:ToolInvocationUpdate", "tool invocation update", "A source-visible information artifact reporting lifecycle state or revised metadata for an already identified tool invocation; observing this artifact does not constitute another invocation event.", ["se:InformationArtifact"], ["evidence:muse-design", "evidence:otel-genai", "evidence:jsonrpc"])
    add("agent:ToolResult", "tool result", "An information artifact returned or recorded as the result/error payload of a tool invocation.", ["se:InformationArtifact"], ["evidence:otel-genai", "evidence:jsonrpc"])
    add("agent:Delegation", "delegation", "An action by which an agent or harness assigns a task/subtask to another agent or execution scope.", ["ufo:Action"], ["evidence:fipa", "evidence:muse-design"])
    add("agent:Observation", "agent observation", "An information artifact supplied to an agent as an observation of tool, environment, user, or system state.", ["se:InformationArtifact"], ["evidence:otel-genai", "evidence:muse-design"])
    add("agent:GeneratedContent", "generated content", "An information artifact generated by a model-backed agent or model invocation.", ["se:InformationArtifact"], ["evidence:otel-genai"])
    add("agent:RepresentationalState", "agent representational state", "A computational state that explicitly represents propositional or task-relevant content for a software agent; it is not thereby identified with a human mental moment.", ["ufo:Situation"], ["evidence:muse-design"])
    add("agent:HypothesisState", "agent hypothesis state", "A representational state in which propositional content is treated as a hypothesis/candidate rather than as an unqualified assertion.", ["agent:RepresentationalState"], ["evidence:muse-design"])
    add("agent:BeliefLikeState", "agent belief-like state", "A representational state recording content treated by the software agent as currently accepted or relied upon, without asserting phenomenological equivalence to human belief.", ["agent:RepresentationalState"], ["evidence:muse-design"])

    r = {}
    def rel(rid, label, text, dom, rng, ev=E, **kw): r[rid] = relation(rid, label, text, dom, rng, ev, **kw)
    rel("agent:usesModel", "uses model", "Relates a model-backed agent to a model artifact it invokes as part of its operation.", ["agent:ModelBackedAgent"], ["agent:ModelArtifact"], ["evidence:otel-genai"])
    rel("agent:invokesModel", "invokes model", "Relates a model invocation event to the model artifact invoked.", ["agent:ModelInvocation"], ["agent:ModelArtifact"], ["evidence:otel-genai"])
    rel("agent:runOf", "run of", "Relates an agent run to the software agent performing work in that run.", ["agent:AgentRun"], ["agent:SoftwareAgent"])
    rel("agent:hasTask", "has task", "Relates an agent run to a task it is attempting or performing.", ["agent:AgentRun"], ["agent:Task"])
    rel("agent:subtaskOf", "subtask of", "Relates a subtask to the task under which it is scoped.", ["agent:Subtask"], ["agent:Task"], ["evidence:muse-design"])
    rel("agent:taskStateOf", "task state of", "Relates a source-attributed task-state situation to the task whose lifecycle/status it describes.", ["agent:TaskState"], ["agent:Task"], ["evidence:muse-design"])
    rel("agent:taskStateValue", "task state value", "Relates a task-state situation to its exact source-reported status token represented as a string value.", ["agent:TaskState"], ["comp:StringValue"], ["evidence:muse-design"])
    rel("agent:turnCarriesMessage", "turn carries message", "Relates a conversational turn to the message/content artifact contributed by that turn.", ["agent:ConversationalTurn"], ["agent:Message"])
    rel("agent:messageSender", "message sender", "Relates a message to the agent or other participant identified as its sender.", ["agent:Message"], ["ufo:Agent"], ["evidence:fipa"])
    rel("agent:messageReceiver", "message receiver", "Relates a message to an identified receiver/addressee.", ["agent:Message"], ["ufo:Agent"], ["evidence:fipa"])
    rel("agent:messageExpressesProposition", "message expresses proposition", "Relates a message to propositional content it expresses; this does not establish that the message was successfully delivered.", ["agent:Message"], ["ufo:Proposition"], ["evidence:fipa", "evidence:muse-design"])
    rel("agent:deliversMessage", "delivers message", "Relates a message-delivery communicative act to the message artifact delivered or communicated by that act.", ["agent:MessageDelivery"], ["agent:Message"], ["evidence:fipa", "evidence:muse-design"])
    rel("agent:messageCarriesArtifact", "message carries artifact", "Relates a message to an information artifact included or referenced as communicated content.", ["agent:Message"], ["se:InformationArtifact"], ["evidence:fipa", "evidence:muse-design"])
    rel("agent:messageSummary", "message summary", "Relates a message to an explicitly supplied short summary string without treating the summary as a separate asserted message.", ["agent:Message"], ["comp:StringValue"], ["evidence:muse-design"])
    rel("agent:questionPromptHeader", "question prompt header", "Relates a structured question prompt to its explicitly supplied short header/title string.", ["agent:QuestionPrompt"], ["comp:StringValue"], ["evidence:muse-design"])
    rel("agent:questionPromptExpressesContent", "question prompt expresses content", "Relates a structured question prompt to interrogative semantic content without asserting any embedded proposition as true.", ["agent:QuestionPrompt"], ["sem:InterrogativeContent"], ["evidence:muse-design"])
    rel("agent:questionPromptHasOption", "question prompt has option", "Relates a structured question prompt to one explicitly offered choice option.", ["agent:QuestionPrompt"], ["agent:ChoiceOption"], ["evidence:muse-design"])
    rel("agent:questionPromptMultiSelect", "question prompt multi-select", "Relates a structured question prompt to its source-specified multi-select flag.", ["agent:QuestionPrompt"], ["comp:BooleanValue"], ["evidence:muse-design"])
    rel("agent:choiceOptionLabel", "choice option label", "Relates a choice option to its exact source-visible short label.", ["agent:ChoiceOption"], ["comp:StringValue"], ["evidence:muse-design"])
    rel("agent:choiceOptionDescription", "choice option description", "Relates a choice option to its exact source-visible descriptive text.", ["agent:ChoiceOption"], ["comp:StringValue"], ["evidence:muse-design"])
    rel("agent:choiceOptionExpressesContent", "choice option expresses content", "Relates a choice option to semantic content expressed by that option; the relation does not assert or select that content.", ["agent:ChoiceOption"], ["sem:SemanticContent"], ["evidence:muse-design"])
    rel("agent:modelInvocationUsesContext", "model invocation uses context", "Relates a model invocation to context supplied to that invocation.", ["agent:ModelInvocation"], ["agent:Context"], ["evidence:otel-genai"])
    rel("agent:runUsesContext", "run uses context", "Relates an agent run to context retained or supplied within that run.", ["agent:AgentRun"], ["agent:Context"], ["evidence:otel-genai"])
    rel("agent:toolSpecifiedBy", "tool specified by", "Relates a tool to its callable specification.", ["agent:Tool"], ["agent:ToolSpecification"], ["evidence:jsonrpc", "evidence:otel-genai"])
    rel("agent:invokesTool", "invokes tool", "Relates a tool invocation event to the tool being invoked.", ["agent:ToolInvocation"], ["agent:Tool"], ["evidence:otel-genai"])
    rel("agent:toolInvocationPrincipal", "tool invocation principal", "Relates a tool invocation to the agent or harness object that initiated the invocation according to the structured source record.", ["agent:ToolInvocation"], ["ufo:Object"], ["evidence:otel-genai", "evidence:muse-design"])
    rel("agent:toolInvocationInRun", "tool invocation in run", "Relates a tool invocation to the agent run within which it occurred.", ["agent:ToolInvocation"], ["agent:AgentRun"], ["evidence:otel-genai"])
    rel("agent:toolInvocationHasArgument", "tool invocation has argument", "Relates a tool invocation to an explicitly supplied argument artifact.", ["agent:ToolInvocation"], ["comp:Argument"], ["evidence:jsonrpc", "evidence:otel-genai"])
    rel("agent:toolInvocationRetryOf", "tool invocation retry of", "Relates a tool invocation to a prior tool invocation that the source explicitly identifies as the attempt being retried.", ["agent:ToolInvocation"], ["agent:ToolInvocation"], ["evidence:muse-design"])
    rel("agent:toolInvocationParent", "tool invocation parent", "Relates a nested tool invocation to the enclosing/parent tool invocation explicitly identified by the source.", ["agent:ToolInvocation"], ["agent:ToolInvocation"], ["evidence:muse-design"])
    rel("agent:toolInvocationUpdateFor", "tool invocation update for", "Relates a lifecycle/update artifact to the tool invocation whose state or metadata it reports; this relation may be added only when exact source identity establishes the join.", ["agent:ToolInvocationUpdate"], ["agent:ToolInvocation"], ["evidence:muse-design", "evidence:otel-genai", "evidence:jsonrpc"])
    rel("agent:toolInvocationUpdateStatus", "tool invocation update status", "Relates a lifecycle/update artifact to its exact source-reported status token represented as a string value.", ["agent:ToolInvocationUpdate"], ["comp:StringValue"], ["evidence:muse-design", "evidence:otel-genai"])
    rel("agent:toolInvocationUpdateCarriesResult", "tool invocation update carries result", "Relates a lifecycle/update artifact to a tool-result artifact explicitly carried in that update; this does not independently verify external effects described by the result.", ["agent:ToolInvocationUpdate"], ["agent:ToolResult"], ["evidence:muse-design", "evidence:otel-genai"])
    rel("agent:toolInvocationResult", "tool invocation result", "Relates a tool invocation to its returned/recorded result artifact without asserting independent external effects.", ["agent:ToolInvocation"], ["agent:ToolResult"], ["evidence:jsonrpc", "evidence:muse-design"])
    rel("agent:toolResultOfInvocation", "tool result of invocation", "Relates a tool-result artifact to the exact tool invocation whose returned/result record it is, only when source identity establishes the join.", ["agent:ToolResult"], ["agent:ToolInvocation"], ["evidence:jsonrpc", "evidence:muse-design"])
    rel("agent:toolInvocationRequestedEffect", "tool invocation requested effect", "Relates a tool invocation to propositional effect content requested by the invocation. The proposition is intensional content and is not independently asserted as an observed world change.", ["agent:ToolInvocation"], ["ufo:Proposition"], ["evidence:muse-design"])
    rel("agent:toolInvocationObservedEffect", "tool invocation observed effect", "Relates a tool invocation to an effect event independently established by structured source evidence; result success alone is insufficient evidence for this relation.", ["agent:ToolInvocation"], ["ufo:Event"], ["evidence:muse-design"])
    rel("agent:toolResultPayload", "tool result payload", "Relates a tool-result artifact to its exact source-visible payload represented as a literal; this records returned content and does not independently establish external effects.", ["agent:ToolResult"], ["comp:DataValue"], ["evidence:jsonrpc", "evidence:muse-design"])
    rel("agent:toolResultOutcome", "tool result outcome", "Relates a tool-result artifact to the execution outcome that the structured result explicitly reports for the tool operation. A failure outcome classifies the operation/result status; it does not negate or invalidate other payload observations that completed before the failure.", ["agent:ToolResult"], ["comp:ExecutionOutcome"], ["evidence:jsonrpc", "evidence:otel-genai", "evidence:muse-design"])
    rel("agent:toolResultDiagnostic", "tool result diagnostic", "Relates a tool-result artifact to a diagnostic information artifact explicitly returned or recorded with it.", ["agent:ToolResult"], ["se:Diagnostic"], ["evidence:muse-design"])
    rel("agent:toolInvocationDurationMillis", "tool invocation duration milliseconds", "Relates a tool invocation to a source-reported elapsed duration in milliseconds represented as an integer data value.", ["agent:ToolInvocation"], ["comp:IntegerValue"], ["evidence:otel-genai"])
    rel("agent:delegatesTask", "delegates task", "Relates a delegation action to the task/subtask delegated.", ["agent:Delegation"], ["agent:Task"])
    rel("agent:delegatedTo", "delegated to", "Relates a delegation action to the receiving software agent.", ["agent:Delegation"], ["agent:SoftwareAgent"])
    rel("agent:observationForAgent", "observation for agent", "Relates an observation artifact to the software agent to which it was supplied.", ["agent:Observation"], ["agent:SoftwareAgent"])
    rel("agent:observationForRun", "observation for run", "Relates an observation artifact to the agent run in which it was supplied.", ["agent:Observation"], ["agent:AgentRun"])
    rel("agent:generatedByInvocation", "generated by invocation", "Relates generated content to the model invocation that generated it.", ["agent:GeneratedContent"], ["agent:ModelInvocation"], ["evidence:otel-genai"])
    rel("agent:representsProposition", "represents proposition", "Relates an agent representational state to propositional content it computationally represents.", ["agent:RepresentationalState"], ["ufo:Proposition"], ["evidence:muse-design"])
    return ontology(
        "muse.agent", "Muse AI and software-agent ontology",
        "Framework-neutral software-agent, model, message, context, tool, delegation, and representational-state concepts.",
        [exact(prov_ref), exact(software_ref), exact(computing_ref), exact(semantic_ref), exact(ufo_b), exact(ufo_c)],
        sorted(set(x for v in c.values() for x in v["evidence"]) | set(x for v in r.values() for x in v["evidence"])),
        c, r,
    )


def harness_package(prov_ref, software_ref, computing_ref, agent_ref, ufo_b):
    E = ["evidence:muse-design", "evidence:otel-genai"]
    c = {}
    def add(cid, label, text, parents, ev=E, **kw): c[cid] = concept(cid, label, text, parents, ev, **kw)
    add("harness:CodingHarness", "coding harness", "A software system that orchestrates coding-agent runs, model turns, tools, workspaces, and run/event records.", ["se:SoftwareProduct"])
    add("harness:CodingAgent", "coding agent", "A software agent operating in a coding harness to perform software-engineering tasks.", ["agent:SoftwareAgent"])
    add("harness:HarnessSession", "coding-harness session", "A temporally bounded harness context that groups one or more coding runs, conversation lineages, tool activity, and its event-log identity without identifying the session with any single run.", ["ufo:Situation"])
    add("harness:ConversationLineage", "conversation lineage", "An information artifact/identity that designates one ordered conversational branch through a coding-harness session and can persist across multiple model runs.", ["se:InformationArtifact"])
    add("harness:CodingRun", "coding run", "An agent run scoped to a coding-harness execution episode and its run identity.", ["agent:AgentRun"])
    add("harness:CodingTask", "coding task", "An agent task whose subject matter is software engineering or modification of a software workspace.", ["agent:Task"])
    add("harness:Workspace", "coding workspace", "An execution environment containing the filesystem/repository state made available for a coding run.", ["comp:ExecutionEnvironment"])
    add("harness:Repository", "software repository", "A software artifact collection managed under a repository identity, potentially with version-control history.", ["se:SoftwareArtifact"])
    add("harness:SourceFile", "source file", "A file artifact that bears source-code information content.", ["comp:File", "se:SourceCodeArtifact"])
    add("harness:EventLog", "coding event log", "An information artifact containing ordered structured records for coding-harness occurrences and content.", ["se:InformationArtifact"])
    add("harness:EventLogRecord", "coding event-log record", "A structured information artifact recording one harness occurrence, state update, message, tool invocation/result, or other logged datum.", ["se:InformationArtifact"])
    add("harness:RunRecord", "run record", "An event-log record identifying creation, status, usage, or completion information for a coding run.", ["harness:EventLogRecord"])
    add("harness:TurnRecord", "turn record", "An event-log record representing a user/agent conversational turn or model turn.", ["harness:EventLogRecord"])
    add("harness:ToolCallRecord", "tool-call record", "A structured event-log/message record that identifies a tool invocation and its arguments, distinct from the invocation occurrence itself.", ["harness:EventLogRecord"])
    add("harness:ToolResultRecord", "tool-result record", "A structured event-log record that records result/error content for a tool invocation, distinct from independently observed world effects.", ["harness:EventLogRecord"])
    add("harness:ShellInvocation", "shell invocation", "A command invocation executed through a shell/process tool in a coding harness.", ["comp:CommandInvocation"])
    add("harness:PatchArtifact", "patch artifact", "An information artifact specifying a set of intended textual/source changes.", ["se:InformationArtifact"])
    add("harness:PatchApplication", "patch application", "A filesystem/software-development action that attempts to apply a patch artifact to workspace artifacts.", ["se:CodingActivity", "comp:FilesystemOperation"])
    add("harness:BuildExecution", "build execution", "A concrete build activity/process execution performed within a coding harness.", ["se:BuildActivity", "comp:ProcessExecution"])
    add("harness:CompilationExecution", "compilation execution", "A concrete compilation activity/process execution performed within a coding harness.", ["se:CompilationActivity", "harness:BuildExecution"])
    add("harness:TestExecution", "test execution", "A concrete testing activity/process execution performed within a coding harness.", ["se:TestingActivity", "comp:ProcessExecution"])
    add("harness:ToolFailure", "tool failure", "A failure outcome reported for a coding-harness tool invocation.", ["comp:FailureOutcome"])
    add("harness:CommandFailure", "command failure", "A failure outcome reported for a shell/command invocation.", ["harness:ToolFailure"])
    add("harness:Retry", "retry", "An action that repeats or re-attempts a prior invocation/run step after failure, interruption, or another retry condition.", ["ufo:Action"])
    add("harness:Recovery", "recovery", "An action intended to restore progress or usable state after a failed/partial coding-harness occurrence.", ["ufo:Action"])
    add("harness:GeneratedArtifact", "generated coding artifact", "A software artifact created as an output of coding-agent/tool activity.", ["se:SoftwareArtifact"])
    add("harness:ExternalServiceInvocation", "external service invocation", "A tool invocation directed to a service outside the local harness process/workspace boundary.", ["agent:ToolInvocation"])
    # Reusable tool-family invocation classes. These classify what structured
    # operation was requested; they do not assert that any requested effect
    # actually occurred.
    add("harness:ShellToolInvocation", "shell tool invocation", "A tool invocation whose structured operation is shell/process execution or session management.", ["agent:ToolInvocation"])
    add("harness:FilesystemToolInvocation", "filesystem tool invocation", "A tool invocation whose structured operation addresses filesystem paths or filesystem information.", ["agent:ToolInvocation"])
    add("harness:PatchToolInvocation", "patch tool invocation", "A tool invocation whose structured operation applies or constructs a patch.", ["agent:ToolInvocation"])
    add("harness:BuildToolInvocation", "build tool invocation", "A tool invocation explicitly structured as a software build operation.", ["agent:ToolInvocation"])
    add("harness:CompilationToolInvocation", "compilation tool invocation", "A tool invocation explicitly structured as compilation.", ["harness:BuildToolInvocation"])
    add("harness:TestToolInvocation", "test tool invocation", "A tool invocation explicitly structured as test execution.", ["agent:ToolInvocation"])
    add("harness:NetworkToolInvocation", "network tool invocation", "A tool invocation explicitly structured as network or remote-service interaction.", ["agent:ToolInvocation"])
    add("harness:ModelToolInvocation", "model tool invocation", "A tool invocation explicitly structured as a model/inference operation.", ["agent:ToolInvocation"])
    add("harness:CodeInspectionToolInvocation", "code inspection tool invocation", "A tool invocation that inspects source-code structure, symbols, dependencies, calls, or related indexed code information.", ["agent:ToolInvocation"])
    add("harness:CodeTransformationToolInvocation", "code transformation tool invocation", "A tool invocation that performs a structured source-code transformation.", ["agent:ToolInvocation"])
    add("harness:ResourceLookupToolInvocation", "resource lookup tool invocation", "A tool invocation that retrieves harness resources such as skills or other registered informational resources.", ["agent:ToolInvocation"])
    add("harness:TaskManagementToolInvocation", "task-management tool invocation", "A tool invocation that reads or changes harness-owned task/todo state.", ["agent:ToolInvocation"])
    add("harness:MemoryToolInvocation", "memory tool invocation", "A tool invocation that reads or changes durable agent memory through a structured harness interface.", ["agent:ToolInvocation"])
    add("harness:ComputerToolInvocation", "computer interaction tool invocation", "A tool invocation that observes or acts on a computer surface through a structured interface.", ["agent:ToolInvocation"])
    add("harness:ApplicationToolInvocation", "application interaction tool invocation", "A tool invocation that creates, opens, or manipulates a harness-managed application surface.", ["agent:ToolInvocation"])
    add("harness:AgentCoordinationToolInvocation", "agent coordination tool invocation", "A tool invocation used to delegate, message, poll, abort, list, or hand off agent work.", ["agent:ToolInvocation"])
    add("harness:UserInteractionToolInvocation", "user interaction tool invocation", "A tool invocation that explicitly requests or records interaction with the attached human user.", ["agent:ToolInvocation"])

    r = {}
    def rel(rid, label, text, dom, rng, ev=E, **kw): r[rid] = relation(rid, label, text, dom, rng, ev, **kw)
    rel("harness:sessionInHarness", "session in harness", "Relates a harness session to the coding harness orchestrating that session.", ["harness:HarnessSession"], ["harness:CodingHarness"])
    rel("harness:sessionHasRun", "session has run", "Relates a harness session to a coding run grouped within that session.", ["harness:HarnessSession"], ["harness:CodingRun"])
    rel("harness:sessionHasLineage", "session has lineage", "Relates a harness session to a conversational lineage identified within that session.", ["harness:HarnessSession"], ["harness:ConversationLineage"])
    rel("harness:runInLineage", "run in lineage", "Relates a coding run to the conversation lineage under which that run contributes model/tool activity.", ["harness:CodingRun"], ["harness:ConversationLineage"])
    rel("harness:runInHarness", "run in harness", "Relates a coding run to the coding harness orchestrating it.", ["harness:CodingRun"], ["harness:CodingHarness"])
    rel("harness:runWorkspace", "run workspace", "Relates a coding run to the workspace against which it operates.", ["harness:CodingRun"], ["harness:Workspace"])
    rel("harness:workspaceRepository", "workspace repository", "Relates a coding workspace to a repository represented within it.", ["harness:Workspace"], ["harness:Repository"])
    rel("harness:logRecords", "log records", "Relates a coding event log to a record contained in that log.", ["harness:EventLog"], ["harness:EventLogRecord"])
    rel("harness:recordBelongsToRun", "record belongs to run", "Relates a structured harness record to the coding run identity whose occurrence/content it records.", ["harness:EventLogRecord"], ["harness:CodingRun"])
    rel("harness:eventSequence", "event sequence", "Relates a coding-harness event to the exact source-log sequence number represented as an integer data value.", ["ufo:Event"], ["comp:IntegerValue"])
    rel("harness:toolCallRecordsInvocation", "tool-call record records invocation", "Relates a tool-call record to the tool-invocation occurrence it denotes/records.", ["harness:ToolCallRecord"], ["agent:ToolInvocation"])
    rel("harness:toolResultRecordsInvocation", "tool-result record records invocation", "Relates a tool-result record to the tool invocation whose result/error it reports.", ["harness:ToolResultRecord"], ["agent:ToolInvocation"])
    rel("harness:toolResultCarriesResult", "tool-result record carries result", "Relates a tool-result record to the tool-result information artifact it carries.", ["harness:ToolResultRecord"], ["agent:ToolResult"])
    rel("harness:occursInWorkspace", "occurs in workspace", "Relates a coding-harness event to the workspace that provides the local execution/filesystem context for that event.", ["ufo:Event"], ["harness:Workspace"])
    rel("harness:patchUsesArtifact", "patch uses artifact", "Relates a patch application to the patch artifact it attempts to apply.", ["harness:PatchApplication"], ["harness:PatchArtifact"])
    rel("harness:generatedByRun", "generated by run", "Relates a generated coding artifact to the coding run during which it was generated.", ["harness:GeneratedArtifact"], ["harness:CodingRun"])
    rel("harness:retryOf", "retry of", "Relates a retry action to the prior occurrence/invocation/run step being re-attempted.", ["harness:Retry"], ["ufo:Event"])
    rel("harness:recoversFromOutcome", "recovers from outcome", "Relates a recovery action to a reported execution outcome that motivated recovery.", ["harness:Recovery"], ["comp:ExecutionOutcome"])
    rel("harness:recoversFromEvent", "recovers from event", "Relates a recovery action to an event whose failure, interruption, or partial completion motivated recovery.", ["harness:Recovery"], ["ufo:Event"])
    return ontology(
        "muse.coding_harness", "Muse generic coding-harness ontology",
        "Reusable coding-harness run/workspace/repository/event-log/tool/build/test/retry concepts independent of Artist.",
        [exact(prov_ref), exact(software_ref), exact(computing_ref), exact(agent_ref), exact(ufo_b)],
        sorted(set(x for v in c.values() for x in v["evidence"]) | set(x for v in r.values() for x in v["evidence"])),
        c, r,
    )


def artist_package(prov_ref, harness_ref, agent_ref, computing_ref, ufo_b):
    E = ["evidence:artist-gortnite-events"]
    c = {}
    def add(cid, label, text, parents, ev=E, **kw): c[cid] = concept(cid, label, text, parents, ev, **kw)
    add("artist:ArtistHarness", "Artist coding harness", "The concrete Artist coding-harness software system defined by the pinned Gortnite implementation.", ["harness:CodingHarness"], ["evidence:artist-gortnite-events", "evidence:artist-gortnite-tools"])
    add("artist:ArtistSession", "Artist session", "An Artist session identified by the frozen session envelope's session field and grouping runs, lineages, and one ordered event log.", ["harness:HarnessSession"])
    add("artist:ArtistLineage", "Artist lineage", "An Artist conversation lineage identified by the frozen session envelope's lineage field; it is distinct from an individual model run.", ["harness:ConversationLineage"])
    add("artist:ArtistSessionEventRecord", "Artist session event record", "One versioned ordered Artist session-log record carrying seq, timestamp, session, optional run, lineage, kind, and payload fields.", ["harness:EventLogRecord"])
    add("artist:ArtistModelTurnRecord", "Artist model-turn record", "An Artist model.turn event record whose content blocks can contain model prose, reasoning material, tool-call blocks, images, or opaque content.", ["harness:TurnRecord", "artist:ArtistSessionEventRecord"])
    add("artist:ArtistToolResultRecord", "Artist tool-result record", "An Artist tool.result event record keyed by internal_call_id and carrying tool identity, arguments, presentation result, structured outcome, and optional timing.", ["harness:ToolResultRecord", "artist:ArtistSessionEventRecord"], ["evidence:artist-gortnite-events", "evidence:artist-gortnite-capture"])
    add("artist:ArtistChangeRecord", "Artist change record", "An Artist change.recorded event record that independently records a path mutation with before/after digests and optional diff evidence associated with a tool-call identity.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistComputerStage", "Artist computer stage", "A concrete Artist computer-tool stage identity opened/closed by computer.stage_opened and computer.stage_closed records.", ["ufo:Situation"])
    add("artist:ArtistComputerActionRecord", "Artist computer action record", "An Artist computer.acted record containing the model-requested target label separately from any resolved target name and action result.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistRuleFiringRecord", "Artist rule-firing record", "An Artist rule.fired record documenting a concrete Artist rule-engine firing without treating rule output as unqualified world truth.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistHandoffRecord", "Artist handoff record", "An Artist handoff.performed record documenting an Artist agent-profile handoff occurrence.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistTaskRecord", "Artist task record", "An Artist task.started, task.updated, or task.finished record for a durable task identity scoped by session and lineage.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistTask", "Artist task", "A durable Artist task identity scoped by session and lineage and referenced by task lifecycle records.", ["harness:CodingTask"])
    add("artist:ArtistComputerStep", "Artist computer action step", "One ordered step recorded inside an Artist computer.acted record, preserving requested label separately from resolved target name.", ["se:InformationArtifact"])
    add("artist:ArtistToolResultImagesRecord", "Artist tool-result images record", "An Artist tool.result.images record carrying image content associated with an internal tool-call identity.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistComputerStageRecord", "Artist computer-stage record", "An Artist computer.stage_opened or computer.stage_closed record describing a computer-stage lifecycle transition.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistComputerLaunchRecord", "Artist computer launch record", "An Artist computer.launched record describing the program and surface produced by a launch.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistComputerObservationRecord", "Artist computer observation record", "An Artist computer.observed record describing one structured observation of a surface.", ["artist:ArtistSessionEventRecord"])
    add("artist:ArtistComputerElisionRecord", "Artist computer elision record", "An Artist computer.elided record documenting context elision metadata without asserting a world-state change.", ["artist:ArtistSessionEventRecord"])

    r = {}
    def rel(rid, label, text, dom, rng, ev=E, **kw): r[rid] = relation(rid, label, text, dom, rng, ev, **kw)
    rel("artist:sessionLineage", "Artist session lineage", "Relates an Artist session to a lineage named in its event-log records.", ["artist:ArtistSession"], ["artist:ArtistLineage"])
    rel("artist:eventRecordSession", "Artist event-record session", "Relates an Artist session event record to the Artist session named by its envelope.", ["artist:ArtistSessionEventRecord"], ["artist:ArtistSession"])
    rel("artist:eventRecordLineage", "Artist event-record lineage", "Relates an Artist session event record to the Artist lineage named by its envelope.", ["artist:ArtistSessionEventRecord"], ["artist:ArtistLineage"])
    rel("artist:eventRecordKind", "Artist event-record kind", "Relates an Artist session event record to its exact stable kind string.", ["artist:ArtistSessionEventRecord"], ["comp:StringValue"])
    rel("artist:eventRecordPayload", "Artist event-record payload", "Relates an Artist session event record to its exact structured JSON payload represented as a structured data value; this relation does not interpret provider-private payload internals.", ["artist:ArtistSessionEventRecord"], ["comp:StructuredDataValue"])
    rel("artist:eventRecordSequence", "Artist event-record sequence", "Relates an Artist session event record to its exact monotonic sequence number.", ["artist:ArtistSessionEventRecord"], ["comp:IntegerValue"])
    rel("artist:eventRecordTimestampMillis", "Artist event-record timestamp milliseconds", "Relates an Artist session event record to the Unix-millisecond timestamp carried by the frozen envelope.", ["artist:ArtistSessionEventRecord"], ["comp:IntegerValue"])
    rel("artist:modelTurnToolInvocation", "Artist model turn tool invocation", "Relates an Artist model-turn record containing a tool_call block to the normalized tool invocation identified by that block's internal id.", ["artist:ArtistModelTurnRecord"], ["agent:ToolInvocation"])
    rel("artist:toolResultInvocation", "Artist tool result invocation", "Relates an Artist tool-result record to the normalized tool invocation joined through internal_call_id.", ["artist:ArtistToolResultRecord"], ["agent:ToolInvocation"], ["evidence:artist-gortnite-events", "evidence:artist-gortnite-capture"])
    rel("artist:changeInvocation", "Artist change invocation", "Relates an Artist change record to the tool invocation whose internal/provider call identity the record associates with the mutation evidence.", ["artist:ArtistChangeRecord"], ["agent:ToolInvocation"])
    rel("artist:changePath", "Artist change path", "Relates an Artist change record to the exact filesystem path expression recorded in its path field.", ["artist:ArtistChangeRecord"], ["comp:Path"])
    rel("artist:taskRecordTask", "Artist task-record task", "Relates an Artist task lifecycle record to the durable task identity named by its task field.", ["artist:ArtistTaskRecord"], ["artist:ArtistTask"])
    rel("artist:taskCommand", "Artist task command", "Relates an Artist task.started record to the exact command string recorded for the task.", ["artist:ArtistTaskRecord"], ["comp:StringValue"])
    rel("artist:taskOutput", "Artist task output", "Relates an Artist task.updated record to exact incremental output recorded for the task.", ["artist:ArtistTaskRecord"], ["comp:StringValue"])
    rel("artist:taskExitCode", "Artist task exit code", "Relates an Artist task.finished record to its reported integer exit code when present.", ["artist:ArtistTaskRecord"], ["comp:IntegerValue"])
    rel("artist:taskPersistent", "Artist task persistent flag", "Relates an Artist task.started record to its explicitly recorded persistent flag.", ["artist:ArtistTaskRecord"], ["comp:BooleanValue"])
    rel("artist:taskInterrupted", "Artist task interrupted flag", "Relates an Artist task.finished record to its explicitly recorded interrupted flag.", ["artist:ArtistTaskRecord"], ["comp:BooleanValue"])
    rel("artist:computerActionStep", "Artist computer action step", "Relates an Artist computer.acted record to one exact ordered step record.", ["artist:ArtistComputerActionRecord"], ["artist:ArtistComputerStep"])
    rel("artist:computerStepIndex", "Artist computer step index", "Relates a computer action step to its zero-based index in the recorded action program.", ["artist:ArtistComputerStep"], ["comp:IntegerValue"])
    rel("artist:computerStepAction", "Artist computer step action", "Relates a computer action step to its exact action string.", ["artist:ArtistComputerStep"], ["comp:StringValue"])
    rel("artist:computerStepPayload", "Artist computer step payload", "Relates a computer action step to the text/chord payload explicitly recorded for it.", ["artist:ArtistComputerStep"], ["comp:StringValue"])
    rel("artist:computerStepOutcome", "Artist computer step outcome", "Relates a computer action step to its exact recorded outcome string.", ["artist:ArtistComputerStep"], ["comp:StringValue"])
    rel("artist:computerClaimedTargetLabel", "Artist computer claimed target label", "Relates an Artist computer action record to the model-supplied target label, represented as a string value and kept distinct from the resolved target name.", ["artist:ArtistComputerStep"], ["comp:StringValue"])
    rel("artist:computerResolvedTargetName", "Artist computer resolved target name", "Relates an Artist computer action record to the target name actually resolved by Artist, represented as a string value and not conflated with the model-supplied label.", ["artist:ArtistComputerStep"], ["comp:StringValue"])
    return ontology(
        "muse.artist", "Muse Artist adapter ontology",
        "Artist-specific session/event concepts pinned to the audited Gortnite implementation; generic coding-harness semantics remain in muse.coding_harness.",
        [exact(prov_ref), exact(harness_ref), exact(agent_ref), exact(computing_ref), exact(ufo_b)],
        sorted(set(x for v in c.values() for x in v["evidence"]) | set(x for v in r.values() for x in v["evidence"])),
        c, r,
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    args = parser.parse_args()
    out = args.out

    ufo = ROOT / "packages" / "ufo"
    ufo_a = package_ref(ufo / "ufo-a.muse.json")
    ufo_b = package_ref(ufo / "ufo-b.muse.json")
    ufo_c = package_ref(ufo / "ufo-c.muse.json")

    prov = seal(out, "provenance", provenance_package(), "foundation-provenance.muse.json")
    software = seal(out, "ontology", software_package(prov, ufo_a, ufo_b, ufo_c), "software.muse.json")
    computing = seal(out, "ontology", computing_package(prov, software, ufo_a, ufo_b), "computing.muse.json")
    semantic = seal(out, "ontology", semantic_package(prov, computing, ufo_a, ufo_c), "semantic.muse.json")
    agent = seal(out, "ontology", agent_package(prov, software, computing, semantic, ufo_b, ufo_c), "agent.muse.json")
    harness = seal(out, "ontology", harness_package(prov, software, computing, agent, ufo_b), "coding-harness.muse.json")
    seal(out, "ontology", artist_package(prov, harness, agent, computing, ufo_b), "artist.muse.json")


if __name__ == "__main__":
    main()
