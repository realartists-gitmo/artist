#!/usr/bin/env python3
"""Generate sealed, source-pinned UFO-family Muse packages."""
from __future__ import annotations

from copy import deepcopy
from pathlib import Path
import argparse
import hashlib
import json
import os
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "packages" / "ufo"
SEED = ROOT / "packages" / "sources" / "bootstrap"
VERSION = "1.0.0"
ZERO = {"algorithm": "sha256", "value": "0" * 64}


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()


def digest(value):
    return {"algorithm": "sha256", "value": hashlib.sha256(compact(value)).hexdigest()}


def load_seed(name):
    path = SEED / name
    if not path.exists():
        return None
    return json.loads(path.read_text())["payload"]["document"]["package"]


def header(pid, kind, title, description, imports=(), evidence=()):
    return {
        "package": {"id": pid, "version": VERSION, "digest": deepcopy(ZERO)},
        "kind": kind,
        "title": title,
        "description": description,
        "license": "CC-BY-4.0",
        "imports": list(imports),
        "evidence": sorted(evidence),
    }


def exact(ref):
    return {"id": ref["id"], "version": ref["version"], "digest": deepcopy(ref["digest"]), "optional": False}


def profile(entity_mode=None, rigidity=None, sortality=None, identity=None, abstractness=None, **meta):
    return {
        "entity_mode": entity_mode,
        "rigidity": rigidity,
        "sortality": sortality,
        "identity": identity,
        "abstractness": abstractness,
        "metaproperties": dict(sorted(meta.items())),
    }


def concept(cid, label, text, parents=(), evidence="evidence:ufo22", prof=None, alt=(), notes=()):
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
        "disjoint_with": [],
        "profile": prof or profile(entity_mode="individual"),
        "deprecated": False,
        "evidence": [evidence],
    }


def relation(rid, label, text, domains, ranges, evidence, supers=(), inverse=None, chars=(), minimum=None, maximum=None):
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
        "evidence": [evidence],
    }


def add_concepts(target, rows, evidence, mode="individual"):
    for row in rows:
        cid, label, parents, *rest = row
        options = rest[0] if rest else {}
        text = options.pop("text", f"A {label} in the {evidence.removeprefix('evidence:')} theory module.")
        target[cid] = concept(cid, label, text, parents, evidence, options.pop("prof", profile(entity_mode=mode)), **options)


def add_relations(target, rows, evidence):
    for row in rows:
        rid, label, domains, ranges, *rest = row
        options = rest[0] if rest else {}
        text = options.pop("text", f"Relates {label} participants according to the {evidence.removeprefix('evidence:')} theory module.")
        target[rid] = relation(rid, label, text, domains, ranges, evidence, **options)


def partition(parent, members): return {"kind": "complete_partition", "parent": parent, "members": sorted(members)}
def disjoint(*members): return {"kind": "disjoint", "members": sorted(members)}
def equivalent(*members): return {"kind": "equivalent", "members": sorted(members)}
def subs(child, parent): return {"kind": "subsumption", "child": child, "parent": parent}
def rsubs(child, parent): return {"kind": "relation_subsumption", "child": child, "parent": parent}
def exists(subject, rel, obj): return {"kind": "existential_restriction", "subject": subject, "relation": rel, "object": obj}
def universal(subject, rel, obj): return {"kind": "universal_restriction", "subject": subject, "relation": rel, "object": obj}
def card(subject, rel, obj, minimum, maximum): return {"kind": "cardinality", "subject": subject, "relation": rel, "object": obj, "constraint": {"minimum": minimum, "maximum": maximum}}
def atom(predicate, *arguments): return {"predicate": predicate, "arguments": list(arguments)}
def rule(premises, conclusion):
    variables = sorted({arg for a in [*premises, conclusion] for arg in a["arguments"] if arg.startswith("?")})
    return {"kind": "rule", "rule": {"variables": variables, "premises": premises, "conclusion": conclusion}}




def readable_id(value):
    local = value.split(":", 1)[-1]
    local = re.sub(r"([a-z0-9])([A-Z])", r"\1 \2", local)
    return local.replace("_", " ").replace("-", " ").lower()


def module_name(package_id):
    return {
        "muse.ufo.a": "UFO-A",
        "muse.ufo.b": "UFO-B",
        "muse.ufo.c": "UFO-C",
        "muse.ufo.mlt": "UFO-MLT",
        "muse.ufo.ab": "UFO-AB",
        "muse.ufo.services": "the UFO-S service ontology",
        "muse.ufo.legal": "the UFO-L legal ontology",
    }.get(package_id, "the UFO family")


def generated_concept_definition(label, parents, package_id):
    module = module_name(package_id)
    parent_labels = [readable_id(parent) for parent in parents]
    genus = parent_labels[0] if parent_labels else "entity"
    lower = label.lower()

    if lower.endswith(" kind"):
        subject = lower.removesuffix(" kind")
        return f"A rigid sortal that supplies an identity principle for {subject} instances in {module}."
    if lower.endswith(" type"):
        subject = lower.removesuffix(" type")
        return f"A type whose instances are {subject} entities as characterized in {module}."
    if lower.endswith(" role mixin"):
        return f"An anti-rigid non-sortal whose instances play a common relational role in {module}."
    if lower.endswith(" role"):
        return f"An anti-rigid sortal whose instances satisfy a contingent relational condition in {module}."
    if lower.endswith(" phase"):
        return f"An anti-rigid classification whose instances satisfy a contingent intrinsic condition during the {lower.removesuffix(' phase')} stage."
    if lower.endswith(" participation"):
        return f"An event part representing the participation of a {lower.removesuffix(' participation')} participant in an event."
    if lower.endswith(" situation") or lower.endswith(" fact"):
        return f"A {genus} representing a state of affairs in which {lower.removesuffix(' situation').removesuffix(' fact')} conditions obtain."
    if lower.endswith(" event") or lower.endswith(" act") or lower.endswith(" action"):
        return f"A {genus} representing the occurrence of {lower} in {module}."
    if lower.endswith(" relator") or lower.endswith(" agreement") or lower.endswith(" contract") or lower.endswith(" case"):
        return f"A {genus} that mediates the parties and correlated dependent moments constituting a {lower}."
    if lower.endswith(" commitment"):
        return f"A commitment whose bearer is bound by the {lower.removesuffix(' commitment')} content or role specified in {module}."
    if lower.endswith(" claim") or lower.endswith(" claim-right"):
        return f"A claim held by one party against another concerning the content identified by this {module} category."
    if lower.endswith(" position") or lower in {"power", "subjection", "immunity", "disability", "duty", "permission", "no-right", "liberty"}:
        return f"A dependent normative position borne by a legal subject and correlated with another subject's position in {module}."
    if lower.endswith(" norm") or lower.endswith(" rule") or lower.endswith(" policy") or lower.endswith(" directive"):
        return f"A normative description that prescribes, permits, forbids, or otherwise regulates conduct in {module}."
    if lower.endswith(" document") or lower in {"statute", "regulation", "judicial decision"}:
        return f"A social object that records or expresses legally relevant normative or decisional content in {module}."
    if lower.endswith(" agent") or lower.endswith(" person") or lower in {"authority", "institution", "society"}:
        return f"A {genus} recognized as capable of bearing or exercising the agency associated with {lower} in {module}."
    if lower.endswith(" history") or lower.endswith(" line") or lower.endswith(" branch"):
        return f"An abstract temporal structure representing a coherent sequence of occurrences in {module}."
    if lower.endswith(" order"):
        return f"An abstract level used to locate an entity in the cross-level classification structure of {module}."
    if lower.endswith(" categorizer") or lower.endswith(" powertype") or lower.endswith(" partitioning type"):
        return f"A higher-order type that classifies or organizes lower-order types according to the constraints of {module}."
    if lower.endswith(" capability") or lower.endswith(" competence"):
        return f"A disposition whose bearer can realize the behavior or institutional effect identified by {lower} in {module}."
    if lower.endswith(" description") or lower.endswith(" specification"):
        return f"A normative or descriptive social object that specifies the content and conditions of {lower} in {module}."
    if lower.endswith(" result"):
        return f"A situation brought about by the relevant process and evaluated as a {lower} in {module}."
    if lower.endswith(" occurrence"):
        return f"A temporally located occurrence of an event within the histories represented by {module}."
    if lower.endswith(" instant"):
        return "An abstract temporal boundary with no temporal duration."
    if lower.endswith(" interval"):
        return "An abstract temporal region bounded by temporal instants."
    if lower.endswith(" collection"):
        return f"A collective whose membership is {lower.removesuffix(' collection')} over its relevant lifetime."
    if lower == "functional complex":
        return "An object unified by a functional structure in which its components play distinct functional roles."

    parents_text = ", ".join(parent_labels) if parent_labels else "entity"
    return f"A specialization of {parents_text} used to represent {lower} in {module}."


def generated_relation_definition(label, domains, ranges, package_id):
    domain = " or ".join(readable_id(item) for item in domains) if domains else "entity"
    range_ = " or ".join(readable_id(item) for item in ranges) if ranges else "entity"
    return f"Holds from a {domain} to a {range_} when the source {label} the target, as defined in {module_name(package_id)}."


def refine_generated_definitions(package):
    package_id = package["header"]["package"]["id"]
    for declaration in package.get("concepts", {}).values():
        text = declaration["definition"]["text"]
        if re.fullmatch(r"A .+ in the .+ theory module\.", text):
            declaration["definition"]["text"] = generated_concept_definition(
                declaration["preferred_labels"]["en"], declaration["parents"], package_id
            )
    for declaration in package.get("relations", {}).values():
        text = declaration["definition"]
        if text.startswith("Relates ") and "participants according to the" in text:
            declaration["definition"] = generated_relation_definition(
                declaration["preferred_labels"]["en"],
                declaration["domains"],
                declaration["ranges"],
                package_id,
            )
    return package

def ontology(pid, title, description, imports, evidence, concepts, relations, axioms):
    return {"header": header(pid, "ontology", title, description, imports, evidence), "concepts": dict(sorted(concepts.items())), "relations": dict(sorted(relations.items())), "axioms": axioms}


def seal(kind, package, filename):
    if kind == "ontology":
        package = refine_generated_definitions(package)
    document = {"kind": kind, "package": package}
    normalized = deepcopy(document)
    normalized["package"]["header"]["package"]["digest"] = deepcopy(ZERO)
    package["header"]["package"]["digest"] = digest(normalized)
    payload = {"document_kind": "package", "document": document}
    # Typed Rust serialization is the canonical authority for both the package
    # self-digest and envelope digest. Python can assemble source data, but it
    # must not guess the field ordering of Muse's serde structs.
    raw = OUT / f".{filename}.raw.json"
    output = OUT / filename
    raw.write_text(json.dumps(document, ensure_ascii=False) + "\n")
    seal_bin = Path(os.environ.get("MUSE_SEAL_BIN", ROOT / "target" / "debug" / "muse"))
    if not seal_bin.is_file():
        raise RuntimeError(f"Muse sealing binary is unavailable at {seal_bin}; build muse-cli or set MUSE_SEAL_BIN")
    subprocess.run([str(seal_bin), "seal-package", str(raw), str(output)], check=True)
    raw.unlink()
    sealed = json.loads(output.read_text())["payload"]["document"]["package"]
    return deepcopy(sealed["header"]["package"])


def source(sid, kind, title, creators, locator, citation, issued, notes=()):
    return {"id": sid, "kind": kind, "title": title, "creators": creators, "locator": locator, "citation": citation, "issued": issued, "notes": list(notes)}


def ev(eid, sid, note):
    return {"id": eid, "status": "canonical", "source": sid, "generated_by": None, "attributed_to": [], "confidence": 10000, "excerpt": None, "locator": None, "notes": [note]}


def provenance_package():
    sources = {
        "source:ufo22": source("source:ufo22", "peer_reviewed_paper", "UFO: Unified Foundational Ontology", ["Giancarlo Guizzardi", "Alessander Botti Benevides", "Claudenir M. Fonseca", "Daniele Porello", "João Paulo A. Almeida", "Tiago Prince Sales"], "https://doi.org/10.3233/AO-210256", "Applied Ontology 17(1), 167-210.", "2022"),
        "source:ufo-b": source("source:ufo-b", "peer_reviewed_paper", "Events as Entities in Ontology-Driven Conceptual Modeling", ["João Paulo A. Almeida", "Ricardo de Almeida Falbo", "Giancarlo Guizzardi"], "https://doi.org/10.1007/978-3-030-33223-5_39", "ER 2019, 469-483.", "2019"),
        "source:ufo-c": source("source:ufo-c", "documentation", "Official UFO-C class specification", ["NEMO Research Group"], "https://ontology.com.br/ufo/ufo-c/spec/", "Official UFO-C hierarchy and definitions.", "2015"),
        "source:ufo-story": source("source:ufo-story", "peer_reviewed_paper", "The UFO Story", ["Giancarlo Guizzardi", "Gerd Wagner", "João Paulo A. Almeida", "Renata Guizzardi"], "https://doi.org/10.3233/AO-150157", "Applied Ontology 10(3-4), 259-271.", "2015"),
        "source:mlt": source("source:mlt", "peer_reviewed_paper", "Extending the Foundations of Ontology-Based Conceptual Modeling with a Multi-Level Theory", ["Victorio A. Carvalho", "Claudenir M. Fonseca", "João Paulo A. Almeida", "Giancarlo Guizzardi"], "https://doi.org/10.1007/978-3-319-25264-3_9", "ER 2015.", "2015"),
        "source:types": source("source:types", "peer_reviewed_paper", "Incorporating Types of Types in Ontology-Driven Conceptual Modeling", ["Claudenir M. Fonseca", "Giancarlo Guizzardi", "João Paulo A. Almeida", "Tiago Prince Sales"], "https://doi.org/10.1007/978-3-031-17995-2_2", "ER 2022, 18-34.", "2022"),
        "source:ufo-ab": source("source:ufo-ab", "peer_reviewed_paper", "Towards a Unified Theory of Endurants and Perdurants: UFO-AB", ["Alessander Botti Benevides", "João Paulo A. Almeida", "Giancarlo Guizzardi"], "https://ceur-ws.org/Vol-2518/paper-FOUST2.pdf", "JOWO/FOUST 2019.", "2019"),
        "source:ufo-s": source("source:ufo-s", "peer_reviewed_paper", "A Commitment-Based Reference Ontology for Services", ["Julio Cesar Nardi", "Ricardo de Almeida Falbo", "João Paulo A. Almeida", "Giancarlo Guizzardi", "Luis Ferreira Pires", "Marten J. van Sinderen", "Nicola Guarino", "Claudenir Morais Fonseca"], "https://doi.org/10.1016/j.is.2015.01.012", "Information Systems 54, 263-288.", "2015"),
        "source:ufo-s-corr": source("source:ufo-s-corr", "peer_reviewed_paper", "Corrigendum to A Commitment-Based Reference Ontology for Services", ["Julio Cesar Nardi et al."], "https://doi.org/10.1016/j.is.2015.09.008", "Information Systems 56, 133-134.", "2016"),
        "source:ufo-l": source("source:ufo-l", "peer_reviewed_paper", "Conceptual Modeling of Legal Relations", ["Cristine Griffo", "João Paulo A. Almeida", "Giancarlo Guizzardi"], "https://doi.org/10.1007/978-3-030-00847-5_14", "ER 2018, 169-183.", "2018"),
        "source:ufo-l-judicial": source("source:ufo-l-judicial", "peer_reviewed_paper", "Legal Theories and Judicial Decision-Making: An Ontological Analysis", ["Cristine Griffo", "João Paulo A. Almeida", "Giancarlo Guizzardi"], "https://doi.org/10.3233/FAIA200661", "FOIS 2020, 63-76.", "2020"),
        "source:ufo-l-powers": source("source:ufo-l-powers", "peer_reviewed_paper", "Legal Powers, Subjections, Disabilities, and Immunities", ["Cristine Griffo", "João Paulo A. Almeida", "João A. O. Lima", "Tiago Prince Sales", "Giancarlo Guizzardi"], "https://doi.org/10.3233/AO-220267", "Applied Ontology 18(3).", "2023"),
    }
    evidence = {
        "evidence:ufo22": ev("evidence:ufo22", "source:ufo22", "Consolidated UFO-A backbone and micro-theories."),
        "evidence:ufo-b": ev("evidence:ufo-b", "source:ufo-b", "Event, participation, time, manifestation, change and causation."),
        "evidence:ufo-c": ev("evidence:ufo-c", "source:ufo-c", "Official intentional/social hierarchy, cross-checked with the UFO Story."),
        "evidence:ufo-story": ev("evidence:ufo-story", "source:ufo-story", "A/B/C theory-family organization."),
        "evidence:mlt": ev("evidence:mlt", "source:mlt", "Multi-level orders and relations."),
        "evidence:types": ev("evidence:types", "source:types", "Updated types-of-types distinctions."),
        "evidence:ufo-ab": ev("evidence:ufo-ab", "source:ufo-ab", "Branching-time A/B integration."),
        "evidence:ufo-s": ev("evidence:ufo-s", "source:ufo-s", "Service offering, negotiation, agreement, delivery and outcome."),
        "evidence:ufo-s-corr": ev("evidence:ufo-s-corr", "source:ufo-s-corr", "Corrected service model."),
        "evidence:ufo-l": ev("evidence:ufo-l", "source:ufo-l", "Legal relators and Hohfeldian positions."),
        "evidence:ufo-l-judicial": ev("evidence:ufo-l-judicial", "source:ufo-l-judicial", "Legal things, norms, events and judicial reasoning."),
        "evidence:ufo-l-powers": ev("evidence:ufo-l-powers", "source:ufo-l-powers", "Competence-level legal positions."),
    }
    return {"header": header("muse.ufo.provenance", "provenance", "Muse UFO-family provenance", "Primary source and evidence records for the bundled UFO family.", (), ()), "sources": dict(sorted(sources.items())), "agents": {}, "activities": {}, "evidence": dict(sorted(evidence.items())), "assertions": {}}


def package_a(prov):
    seed = load_seed("ufo-core.muse.json")
    b_concepts = {"ufo:AtomicEvent", "ufo:ComplexEvent", "ufo:Event", "ufo:Participation", "ufo:Situation", "ufo:TimePoint"}
    b_relations = {"ufo:bringsAbout", "ufo:directlyCauses", "ufo:eventPartOf", "ufo:hasBeginPoint", "ufo:hasEndPoint", "ufo:isResultSituationOf", "ufo:isTriggerSituationOf", "ufo:participatesIn", "ufo:triggers"}
    c = {k: deepcopy(v) for k, v in seed["concepts"].items() if k not in b_concepts}
    r = {k: deepcopy(v) for k, v in seed["relations"].items() if k not in b_relations}
    add_concepts(c, [
        ("ufo:RelationshipType", "relationship type", ["ufo:Type"]),
        ("ufo:MaterialRelationshipType", "material relationship type", ["ufo:RelationshipType"]),
        ("ufo:ComparativeRelationshipType", "comparative relationship type", ["ufo:RelationshipType"]),
        ("ufo:SubstantialType", "substantial type", ["ufo:EndurantType"]),
        ("ufo:MomentType", "moment type", ["ufo:EndurantType"]),
        ("ufo:ObjectType", "object type", ["ufo:SubstantialType"]),
        ("ufo:CollectiveType", "collective type", ["ufo:SubstantialType"]),
        ("ufo:QuantityType", "quantity type", ["ufo:SubstantialType"]),
        ("ufo:RelatorType", "relator type", ["ufo:MomentType"]),
        ("ufo:IntrinsicMomentType", "intrinsic moment type", ["ufo:MomentType"]),
        ("ufo:ModeType", "mode type", ["ufo:IntrinsicMomentType"]),
        ("ufo:QualityType", "quality type", ["ufo:IntrinsicMomentType"]),
        ("ufo:RigidSortal", "rigid sortal", ["ufo:Sortal"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal")}),
        ("ufo:AntiRigidSortal", "anti-rigid sortal", ["ufo:Sortal"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal")}),
        ("ufo:RigidNonSortal", "rigid non-sortal", ["ufo:NonSortal"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="non_sortal")}),
        ("ufo:AntiRigidNonSortal", "anti-rigid non-sortal", ["ufo:NonSortal"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="non_sortal")}),
        ("ufo:SemiRigidNonSortal", "semi-rigid non-sortal", ["ufo:NonSortal"], {"prof": profile(entity_mode="type", rigidity="semi_rigid", sortality="non_sortal")}),
        ("ufo:ObjectKind", "object kind", ["ufo:Kind", "ufo:ObjectType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:CollectiveKind", "collective kind", ["ufo:Kind", "ufo:CollectiveType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:QuantityKind", "quantity kind", ["ufo:Kind", "ufo:QuantityType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:RelatorKind", "relator kind", ["ufo:Kind", "ufo:RelatorType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:ModeKind", "mode kind", ["ufo:Kind", "ufo:ModeType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:QualityKind", "quality kind", ["ufo:Kind", "ufo:QualityType"], {"prof": profile(entity_mode="type", rigidity="rigid", sortality="sortal", identity="supplies")}),
        ("ufo:FunctionalComplex", "functional complex", ["ufo:Object"]),
        ("ufo:FixedCollection", "fixed collection", ["ufo:Collective"]),
        ("ufo:VariableCollection", "variable collection", ["ufo:Collective"]),
        ("ufo:TemporalEntity", "temporal entity", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ufo:TemporalInstant", "temporal instant", ["ufo:TemporalEntity"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ufo:TemporalInterval", "temporal interval", ["ufo:TemporalEntity"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
    ], "evidence:ufo22")
    add_relations(r, [
        ("ufo:hasProperPart", "has proper part", ["ufo:Entity"], ["ufo:Entity"], {"inverse": "ufo:properPartOf", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:componentOf", "is component of", ["ufo:Object"], ["ufo:Object"], {"inverse": "ufo:hasComponent", "supers": ["ufo:properPartOf"]}),
        ("ufo:hasComponent", "has component", ["ufo:Object"], ["ufo:Object"], {"inverse": "ufo:componentOf", "supers": ["ufo:hasProperPart"]}),
        ("ufo:memberOf", "is member of", ["ufo:Substantial"], ["ufo:Collective"], {"inverse": "ufo:hasMember", "supers": ["ufo:properPartOf"]}),
        ("ufo:hasMember", "has member", ["ufo:Collective"], ["ufo:Substantial"], {"inverse": "ufo:memberOf", "supers": ["ufo:hasProperPart"]}),
        ("ufo:subCollectionOf", "is subcollection of", ["ufo:Collective"], ["ufo:Collective"], {"supers": ["ufo:properPartOf"]}),
        ("ufo:subQuantityOf", "is subquantity of", ["ufo:Quantity"], ["ufo:Quantity"], {"supers": ["ufo:properPartOf"]}),
        ("ufo:overlaps", "overlaps", ["ufo:Entity"], ["ufo:Entity"], {"chars": ["symmetric"]}),
        ("ufo:hasUltimateBearer", "has ultimate bearer", ["ufo:Moment"], ["ufo:Substantial"], {"chars": ["functional"]}),
        ("ufo:quaIndividualOf", "is qua individual of", ["ufo:QuaIndividual"], ["ufo:Endurant"], {"chars": ["functional"]}),
        ("ufo:hasQuaIndividualPart", "has qua-individual part", ["ufo:Relator"], ["ufo:QuaIndividual"], {"supers": ["ufo:hasProperPart"]}),
        ("ufo:characterizedBy", "is characterized by", ["ufo:EndurantType"], ["ufo:MomentType"]),
        ("ufo:associatedWith", "is associated with", ["ufo:QualityType"], ["ufo:QualityStructure"], {"chars": ["functional"]}),
        ("ufo:projectsTo", "projects to", ["ufo:Quality"], ["ufo:Quale"], {"chars": ["functional"]}),
        ("ufo:containsQuale", "contains quale", ["ufo:QualityStructure"], ["ufo:Quale"]),
        ("ufo:hasLife", "has life", ["ufo:Endurant"], ["ufo:Perdurant"], {"chars": ["functional"]}),
        ("ufo:functionsAs", "functions as", ["ufo:Object"], ["ufo:Type"]),
    ], "evidence:ufo22")
    # Repair reciprocal inverses from the seed and additions.
    r["ufo:properPartOf"]["inverse"] = "ufo:hasProperPart"
    axioms = [
        partition("ufo:Entity", ["ufo:Individual", "ufo:Type"]),
        partition("ufo:Individual", ["ufo:AbstractIndividual", "ufo:ConcreteIndividual"]),
        partition("ufo:ConcreteIndividual", ["ufo:Endurant", "ufo:Perdurant"]),
        disjoint("ufo:Substantial", "ufo:Moment"),
        partition("ufo:Substantial", ["ufo:Object", "ufo:Collective", "ufo:Quantity"]),
        partition("ufo:Moment", ["ufo:IntrinsicMoment", "ufo:Relator"]),
        partition("ufo:IntrinsicMoment", ["ufo:Mode", "ufo:Quality"]),
        partition("ufo:RelationshipType", ["ufo:MaterialRelationshipType", "ufo:ComparativeRelationshipType"]),
        partition("ufo:EndurantType", ["ufo:Sortal", "ufo:NonSortal"]),
        partition("ufo:RigidSortal", ["ufo:Kind", "ufo:SubKind"]),
        partition("ufo:AntiRigidSortal", ["ufo:Phase", "ufo:Role"]),
        equivalent("ufo:RigidNonSortal", "ufo:Category"),
        equivalent("ufo:SemiRigidNonSortal", "ufo:Mixin"),
        partition("ufo:AntiRigidNonSortal", ["ufo:PhaseMixin", "ufo:RoleMixin"]),
        disjoint("ufo:Quale", "ufo:Set"),
        card("ufo:IntrinsicMoment", "ufo:inheresIn", "ufo:Endurant", 1, 1),
        card("ufo:Relator", "ufo:mediates", "ufo:Endurant", 2, None),
        card("ufo:Quality", "ufo:projectsTo", "ufo:Quale", 1, 1),
        card("ufo:QualityType", "ufo:associatedWith", "ufo:QualityStructure", 1, 1),
        card("ufo:QuaIndividual", "ufo:quaIndividualOf", "ufo:Endurant", 1, 1),
        exists("ufo:Relator", "ufo:hasQuaIndividualPart", "ufo:QuaIndividual"),
        universal("ufo:Relator", "ufo:mediates", "ufo:Endurant"),
        universal("ufo:Moment", "ufo:hasUltimateBearer", "ufo:Substantial"),
    ]
    return ontology("muse.ufo.a", "Muse UFO-A", "Executable source-derived UFO-A endurant, type, dependence, mereology, constitution and quality package.", [exact(prov)], ["evidence:ufo22"], c, r, axioms)


def package_b(prov, a):
    c = {}
    add_concepts(c, [
        ("ufo:Event", "event", ["ufo:Perdurant"]),
        ("ufo:AtomicEvent", "atomic event", ["ufo:Event"]),
        ("ufo:ComplexEvent", "complex event", ["ufo:Event"]),
        ("ufo:Participation", "participation", ["ufo:Event"]),
        ("ufo:AgentParticipation", "agent participation", ["ufo:Participation"]),
        ("ufo:ObjectParticipation", "object participation", ["ufo:Participation"]),
        ("ufo:Creation", "creation", ["ufo:Participation"]),
        ("ufo:Termination", "termination", ["ufo:Participation"]),
        ("ufo:Change", "change", ["ufo:Participation"]),
        ("ufo:Usage", "usage", ["ufo:Participation"]),
        ("ufo:Situation", "situation", ["ufo:Endurant"], {"notes": ["UFO-B treats situations as endurants; the A package deliberately does not close Endurant into Substantial/Moment."]}),
        ("ufo:EventType", "event type", ["ufo:PerdurantType"], {"prof": profile(entity_mode="type")}),
        ("ufo:SituationType", "situation type", ["ufo:EndurantType"], {"prof": profile(entity_mode="type")}),
        ("ufo:TimePoint", "time point", ["ufo:TemporalInstant"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ufo:EventOccurrence", "event occurrence", ["ufo:Event"]),
        ("ufo:CausalSituation", "causal situation", ["ufo:Situation"]),
        ("ufo:TriggerSituation", "trigger situation", ["ufo:Situation"]),
        ("ufo:ResultSituation", "result situation", ["ufo:Situation"]),
        ("ufo:StateTransition", "state transition", ["ufo:ComplexEvent"]),
        ("ufo:HistoricalDependence", "historical dependence", ["ufo:ExternallyDependentMode"]),
    ], "evidence:ufo-b")
    r = {}
    add_relations(r, [
        ("ufo:eventPartOf", "is event part of", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:properPartOf"], "inverse": "ufo:hasEventPart", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:hasEventPart", "has event part", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:hasProperPart"], "inverse": "ufo:eventPartOf", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:hasParticipation", "has participation", ["ufo:Event"], ["ufo:Participation"], {"inverse": "ufo:participationIn"}),
        ("ufo:participationIn", "is participation in", ["ufo:Participation"], ["ufo:Event"], {"inverse": "ufo:hasParticipation", "supers": ["ufo:eventPartOf"], "chars": ["functional"]}),
        ("ufo:hasParticipant", "has participant", ["ufo:Participation"], ["ufo:Endurant"], {"inverse": "ufo:participatesThrough", "chars": ["functional"]}),
        ("ufo:participatesThrough", "participates through", ["ufo:Endurant"], ["ufo:Participation"], {"inverse": "ufo:hasParticipant"}),
        ("ufo:participatesIn", "participates in", ["ufo:Endurant"], ["ufo:Event"]),
        ("ufo:exclusivelyDependsOn", "exclusively depends on", ["ufo:Participation"], ["ufo:Endurant"], {"supers": ["ufo:existentiallyDependsOn"], "chars": ["functional"]}),
        ("ufo:hasBeginPoint", "has begin point", ["ufo:Event"], ["ufo:TimePoint"], {"chars": ["functional"]}),
        ("ufo:hasEndPoint", "has end point", ["ufo:Event"], ["ufo:TimePoint"], {"chars": ["functional"]}),
        ("ufo:before", "is before", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:after", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:after", "is after", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:before", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:meets", "meets", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:metBy", "chars": ["irreflexive"]}),
        ("ufo:metBy", "is met by", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:meets", "chars": ["irreflexive"]}),
        ("ufo:overlapsEvent", "overlaps event", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:overlappedBy", "chars": ["irreflexive"]}),
        ("ufo:overlappedBy", "is overlapped by", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:overlapsEvent", "chars": ["irreflexive"]}),
        ("ufo:starts", "starts", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:startedBy", "chars": ["irreflexive"]}),
        ("ufo:startedBy", "is started by", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:starts", "chars": ["irreflexive"]}),
        ("ufo:during", "is during", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:containsEvent", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:containsEvent", "contains event", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:during", "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("ufo:finishes", "finishes", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:finishedBy", "chars": ["irreflexive"]}),
        ("ufo:finishedBy", "is finished by", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:finishes", "chars": ["irreflexive"]}),
        ("ufo:temporallyEquals", "temporally equals", ["ufo:Event"], ["ufo:Event"], {"inverse": "ufo:temporallyEquals", "chars": ["reflexive", "symmetric", "transitive"]}),
        ("ufo:triggers", "triggers", ["ufo:Situation"], ["ufo:Event"]),
        ("ufo:bringsAbout", "brings about", ["ufo:Event"], ["ufo:Situation"]),
        ("ufo:isTriggerSituationOf", "is trigger situation of", ["ufo:Situation"], ["ufo:Event"], {"supers": ["ufo:triggers"]}),
        ("ufo:isResultSituationOf", "is result situation of", ["ufo:Situation"], ["ufo:Event"]),
        ("ufo:isManifestationOf", "is manifestation of", ["ufo:Event"], ["ufo:Disposition"], {"inverse": "ufo:hasManifestation"}),
        ("ufo:hasManifestation", "has manifestation", ["ufo:Disposition"], ["ufo:Event"], {"inverse": "ufo:isManifestationOf"}),
        ("ufo:activates", "activates", ["ufo:Situation"], ["ufo:Disposition"]),
        ("ufo:causes", "causes", ["ufo:Event"], ["ufo:Event"], {"chars": ["irreflexive"]}),
        ("ufo:directlyCauses", "directly causes", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:causes"], "chars": ["irreflexive", "asymmetric"]}),
        ("ufo:creates", "creates", ["ufo:Event"], ["ufo:Endurant"]),
        ("ufo:terminates", "terminates", ["ufo:Event"], ["ufo:Endurant"]),
        ("ufo:changes", "changes", ["ufo:Event"], ["ufo:Endurant"]),
        ("ufo:historicallyDependsOn", "historically depends on", ["ufo:Endurant"], ["ufo:Event"]),
    ], "evidence:ufo-b")
    axioms = [
        disjoint("ufo:AtomicEvent", "ufo:ComplexEvent"),
        partition("ufo:Participation", ["ufo:AgentParticipation", "ufo:ObjectParticipation", "ufo:Creation", "ufo:Termination", "ufo:Change", "ufo:Usage"]),
        card("ufo:Event", "ufo:hasBeginPoint", "ufo:TimePoint", 1, 1),
        card("ufo:Event", "ufo:hasEndPoint", "ufo:TimePoint", 1, 1),
        card("ufo:Participation", "ufo:participationIn", "ufo:Event", 1, 1),
        card("ufo:Participation", "ufo:hasParticipant", "ufo:Endurant", 1, 1),
        exists("ufo:Event", "ufo:hasParticipation", "ufo:Participation"),
        universal("ufo:Event", "ufo:hasEventPart", "ufo:Event"),
        rule([atom("ufo:bringsAbout", "?e1", "?s"), atom("ufo:triggers", "?s", "?e2")], atom("ufo:directlyCauses", "?e1", "?e2")),
        rule([atom("ufo:isManifestationOf", "?e", "?d")], atom("ufo:manifests", "?d", "?e")),
        rule([atom("ufo:hasParticipant", "?p", "?x"), atom("ufo:participationIn", "?p", "?e")], atom("ufo:participatesIn", "?x", "?e")),
    ]
    return ontology("muse.ufo.b", "Muse UFO-B", "Executable source-derived UFO-B event, participation, temporal, manifestation, change and causation package.", [exact(prov), exact(a)], ["evidence:ufo-b"], c, r, axioms)


def package_c(prov, a, b):
    seed = load_seed("ufo-social.muse.json")
    c = deepcopy(seed["concepts"])
    r = deepcopy(seed["relations"])
    # The seed used multiple domain entries as alternatives. Muse domains are conjunctive,
    # so retain a safe common domain and express the narrower communicative case as a subrelation.
    r["ufo:aimsAt"]["domains"] = ["ufo:Entity"]
    r["ufo:hasContent"]["domains"] = ["ufo:Entity"]
    add_concepts(c, [
        ("ufo:PhysicalObject", "physical object", ["ufo:Object"]),
        ("ufo:Resource", "resource", ["ufo:Object"]),
        ("ufo:Society", "society", ["ufo:CollectiveSocialAgent"]),
        ("ufo:Institution", "institution", ["ufo:InstitutionalAgent"]),
        ("ufo:NormativeSystem", "normative system", ["ufo:SocialObject"]),
        ("ufo:InternalCommitment", "internal commitment", ["ufo:Commitment"]),
        ("ufo:Appointment", "appointment", ["ufo:Commitment"]),
        ("ufo:AtomicCommitment", "atomic commitment", ["ufo:Commitment"]),
        ("ufo:ComplexCommitment", "complex commitment", ["ufo:Commitment"]),
        ("ufo:OpenCommitment", "open commitment", ["ufo:Commitment"]),
        ("ufo:ClosedCommitment", "closed commitment", ["ufo:Commitment"]),
        ("ufo:AppointmentGoal", "appointment goal", ["ufo:Goal"]),
        ("ufo:ActionContribution", "action contribution", ["ufo:AgentParticipation"]),
        ("ufo:ResourceParticipation", "resource participation", ["ufo:ObjectParticipation"]),
        ("ufo:Delegatum", "delegatum", ["ufo:SocialRelator"]),
        ("ufo:SocialRoleMixin", "social role mixin", ["ufo:RoleMixin"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="non_sortal", identity="none")}),
        ("ufo:MaterialRelation", "material relation", ["ufo:MaterialRelationshipType"], {"prof": profile(entity_mode="type")}),
        ("ufo:Delegation", "delegation", ["ufo:MaterialRelation"], {"prof": profile(entity_mode="type")}),
        ("ufo:InstitutionalAct", "institutional act", ["ufo:CommunicativeAct"]),
        ("ufo:RecognizedAgent", "recognized agent", ["ufo:SocialAgent"]),
        ("ufo:SocialSituation", "social situation", ["ufo:Situation"]),
        ("ufo:CommitmentSituation", "commitment situation", ["ufo:SocialSituation"]),
        ("ufo:ClaimSituation", "claim situation", ["ufo:SocialSituation"]),
        ("ufo:NormativeProposition", "normative proposition", ["ufo:Proposition"]),
        ("ufo:Directive", "directive", ["ufo:NormativeDescription"]),
        ("ufo:Policy", "policy", ["ufo:NormativeDescription"]),
        ("ufo:Rule", "rule", ["ufo:NormativeDescription"]),
        ("ufo:SocialRoleType", "social role type", ["ufo:Role"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("ufo:AgentType", "agent type", ["ufo:ObjectType"], {"prof": profile(entity_mode="type")}),
        ("ufo:ActionType", "action type", ["ufo:EventType"], {"prof": profile(entity_mode="type")}),
    ], "evidence:ufo-c")
    add_relations(r, [
        ("ufo:bearsIntentionalMoment", "bears intentional moment", ["ufo:Agent"], ["ufo:IntentionalMoment"], {"inverse": "ufo:intentionalMomentInheresIn"}),
        ("ufo:intentionalMomentInheresIn", "intentional moment inheres in", ["ufo:IntentionalMoment"], ["ufo:Agent"], {"inverse": "ufo:bearsIntentionalMoment", "supers": ["ufo:inheresIn"], "chars": ["functional"]}),
        ("ufo:expresses", "expresses", ["ufo:CommunicativeAct"], ["ufo:Proposition"]),
        ("ufo:createsSocialRelator", "creates social relator", ["ufo:InstitutionalAct"], ["ufo:SocialRelator"]),
        ("ufo:modifiesSocialRelator", "modifies social relator", ["ufo:InstitutionalAct"], ["ufo:SocialRelator"]),
        ("ufo:terminatesSocialRelator", "terminates social relator", ["ufo:InstitutionalAct"], ["ufo:SocialRelator"]),
        ("ufo:hasDebtor", "has debtor", ["ufo:SocialCommitment"], ["ufo:Agent"], {"chars": ["functional"]}),
        ("ufo:hasCreditor", "has creditor", ["ufo:SocialCommitment"], ["ufo:Agent"], {"chars": ["functional"]}),
        ("ufo:claimHolder", "has claim holder", ["ufo:SocialClaim"], ["ufo:Agent"], {"chars": ["functional"]}),
        ("ufo:claimAgainst", "has claim against", ["ufo:SocialClaim"], ["ufo:Agent"], {"chars": ["functional"]}),
        ("ufo:composesSocialRelator", "composes social relator", ["ufo:SocialMoment"], ["ufo:SocialRelator"], {"supers": ["ufo:properPartOf"]}),
        ("ufo:mediatesSocialAgent", "mediates social agent", ["ufo:SocialRelator"], ["ufo:Agent"], {"supers": ["ufo:mediates"]}),
        ("ufo:recognizedBy", "is recognized by", ["ufo:SocialObject"], ["ufo:Institution"]),
        ("ufo:governedBy", "is governed by", ["ufo:Institution"], ["ufo:NormativeDescription"]),
        ("ufo:assignsRole", "assigns role", ["ufo:Institution"], ["ufo:SocialRole"]),
        ("ufo:fulfillsCommitment", "fulfills commitment", ["ufo:Action"], ["ufo:Commitment"]),
        ("ufo:violatesCommitment", "violates commitment", ["ufo:Action"], ["ufo:Commitment"]),
        ("ufo:delegatesTo", "delegates to", ["ufo:Agent"], ["ufo:Agent"]),
        ("ufo:delegatesGoal", "delegates goal", ["ufo:Delegatum"], ["ufo:Goal"]),
        ("ufo:contributesToAction", "contributes to action", ["ufo:ActionContribution"], ["ufo:Action"]),
        ("ufo:usesResource", "uses resource", ["ufo:Action"], ["ufo:Resource"]),
    ], "evidence:ufo-c")
    r["ufo:expresses"]["super_relations"] = ["ufo:hasContent"]
    axioms = [
        partition("ufo:Agent", ["ufo:PhysicalAgent", "ufo:SocialAgent"]),
        partition("ufo:SocialAgent", ["ufo:CollectiveSocialAgent", "ufo:InstitutionalAgent"]),
        partition("ufo:MentalMoment", ["ufo:Belief", "ufo:Desire"]),
        partition("ufo:Action", ["ufo:AtomicAction", "ufo:ComplexAction"]),
        partition("ufo:Commitment", ["ufo:InternalCommitment", "ufo:SocialCommitment", "ufo:Appointment"]),
        disjoint("ufo:AtomicCommitment", "ufo:ComplexCommitment"),
        disjoint("ufo:OpenCommitment", "ufo:ClosedCommitment"),
        card("ufo:IntentionalMoment", "ufo:hasContent", "ufo:Proposition", 1, 1),
        card("ufo:SocialCommitment", "ufo:hasDebtor", "ufo:Agent", 1, 1),
        card("ufo:SocialCommitment", "ufo:hasCreditor", "ufo:Agent", 1, 1),
        card("ufo:SocialClaim", "ufo:claimHolder", "ufo:Agent", 1, 1),
        card("ufo:SocialClaim", "ufo:claimAgainst", "ufo:Agent", 1, 1),
        exists("ufo:SocialCommitment", "ufo:isCorrelativeOf", "ufo:SocialClaim"),
        exists("ufo:SocialClaim", "ufo:isCorrelativeOf", "ufo:SocialCommitment"),
        exists("ufo:SocialRelator", "ufo:mediatesSocialAgent", "ufo:Agent"),
        rule([atom("ufo:performs", "?agent", "?action")], atom("ufo:participatesIn", "?agent", "?action")),
        rule([atom("ufo:composesSocialRelator", "?moment", "?relator")], atom("ufo:properPartOf", "?moment", "?relator")),
    ]
    return ontology("muse.ufo.c", "Muse UFO-C", "Executable source-derived UFO-C intentional, action, commitment, claim, social-relator, role and normative package.", [exact(prov), exact(a), exact(b)], ["evidence:ufo-c", "evidence:ufo-story"], c, r, axioms)


def package_mlt(prov, a):
    c = {}
    add_concepts(c, [
        ("mlt:OntologicalEntity", "ontological entity", ["ufo:Entity"]),
        ("mlt:Individual", "MLT individual", ["mlt:OntologicalEntity"]),
        ("mlt:Type", "MLT type", ["mlt:OntologicalEntity"]),
        ("mlt:FirstOrderType", "first-order type", ["mlt:Type"]),
        ("mlt:SecondOrderType", "second-order type", ["mlt:Type"]),
        ("mlt:ThirdOrderType", "third-order type", ["mlt:Type"]),
        ("mlt:HigherOrderType", "higher-order type", ["mlt:Type"]),
        ("mlt:Categorizer", "categorizer", ["mlt:HigherOrderType"]),
        ("mlt:Powertype", "powertype", ["mlt:HigherOrderType", "mlt:PartitioningType"]),
        ("mlt:PartitioningType", "partitioning type", ["mlt:HigherOrderType"]),
        ("mlt:CompleteCategorizer", "complete categorizer", ["mlt:Categorizer"]),
        ("mlt:DisjointCategorizer", "disjoint categorizer", ["mlt:Categorizer"]),
        ("mlt:CompleteDisjointCategorizer", "complete disjoint categorizer", ["mlt:CompleteCategorizer", "mlt:DisjointCategorizer"]),
        ("mlt:TypeOrder", "type order", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("mlt:OrderAssignment", "order assignment", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
    ], "evidence:mlt")
    r = {}
    add_relations(r, [
        ("mlt:instantiates", "MLT instantiates", ["mlt:OntologicalEntity"], ["mlt:Type"]),
        ("mlt:specializes", "MLT specializes", ["mlt:Type"], ["mlt:Type"], {"chars": ["transitive"]}),
        ("mlt:properSpecializes", "properly specializes", ["mlt:Type"], ["mlt:Type"], {"supers": ["mlt:specializes"], "chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("mlt:subordinates", "subordinates", ["mlt:Type"], ["mlt:Type"], {"chars": ["transitive"]}),
        ("mlt:characterizes", "characterizes", ["mlt:HigherOrderType"], ["mlt:Type"]),
        ("mlt:completelyCharacterizes", "completely characterizes", ["mlt:HigherOrderType"], ["mlt:Type"], {"supers": ["mlt:characterizes"]}),
        ("mlt:disjointlyCharacterizes", "disjointly characterizes", ["mlt:HigherOrderType"], ["mlt:Type"], {"supers": ["mlt:characterizes"]}),
        ("mlt:partitions", "partitions", ["mlt:PartitioningType"], ["mlt:Type"], {"supers": ["mlt:completelyCharacterizes", "mlt:disjointlyCharacterizes"]}),
        ("mlt:isPowertypeOf", "is powertype of", ["mlt:Powertype"], ["mlt:Type"], {"supers": ["mlt:partitions"]}),
        ("mlt:hasOrder", "has ontological order", ["mlt:OntologicalEntity"], ["mlt:TypeOrder"], {"chars": ["functional"]}),
        ("mlt:immediatelyHigherThan", "is immediately higher than", ["mlt:TypeOrder"], ["mlt:TypeOrder"], {"chars": ["irreflexive", "asymmetric"]}),
        ("mlt:higherThan", "is higher than", ["mlt:TypeOrder"], ["mlt:TypeOrder"], {"chars": ["irreflexive", "asymmetric", "transitive"]}),
        ("mlt:classifiesType", "classifies type", ["mlt:HigherOrderType"], ["mlt:Type"]),
        ("mlt:categorizes", "categorizes", ["mlt:Categorizer"], ["mlt:Type"], {"supers": ["mlt:classifiesType"]}),
        ("mlt:hasBaseType", "has base type", ["mlt:HigherOrderType"], ["mlt:Type"], {"chars": ["functional"]}),
    ], "evidence:mlt")
    axioms = [
        partition("mlt:OntologicalEntity", ["mlt:Individual", "mlt:Type"]),
        disjoint("mlt:FirstOrderType", "mlt:SecondOrderType", "mlt:ThirdOrderType"),
        subs("mlt:SecondOrderType", "mlt:HigherOrderType"),
        subs("mlt:ThirdOrderType", "mlt:HigherOrderType"),
        card("mlt:OntologicalEntity", "mlt:hasOrder", "mlt:TypeOrder", 1, 1),
        card("mlt:HigherOrderType", "mlt:hasBaseType", "mlt:Type", 1, 1),
        exists("mlt:Powertype", "mlt:isPowertypeOf", "mlt:Type"),
        exists("mlt:PartitioningType", "mlt:partitions", "mlt:Type"),
        rsubs("mlt:isPowertypeOf", "mlt:partitions"),
        rsubs("mlt:partitions", "mlt:characterizes"),
        rule([atom("mlt:immediatelyHigherThan", "?x", "?y")], atom("mlt:higherThan", "?x", "?y")),
    ]
    return ontology("muse.ufo.mlt", "Muse UFO-MLT", "Executable multi-level theory with orders, cross-level instantiation, characterization, powertypes and partitions.", [exact(prov), exact(a)], ["evidence:mlt", "evidence:types"], c, r, axioms)


def package_ab(prov, a, b):
    c = {}
    add_concepts(c, [
        ("ab:History", "history", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ab:ActualHistory", "actual history", ["ab:History"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ab:CounterfactualHistory", "counterfactual history", ["ab:History"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ab:BranchPoint", "branch point", ["ufo:TimePoint"]),
        ("ab:TemporalBranch", "temporal branch", ["ab:History"]),
        ("ab:PossibleWorld", "possible world", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ab:ModalSituation", "modal situation", ["ufo:Situation"]),
        ("ab:Occurrence", "occurrence", ["ufo:EventOccurrence"]),
        ("ab:Presence", "presence", ["ufo:Situation"]),
        ("ab:WorldLine", "world line", ["ab:History"]),
        ("ab:CounterfactualLine", "counterfactual line", ["ab:WorldLine"]),
        ("ab:ModalClaim", "modal claim", ["ufo:AbstractIndividual"], {"prof": profile(entity_mode="individual", abstractness="abstract")}),
        ("ab:AlethicNecessity", "alethic necessity", ["ab:ModalClaim"]),
        ("ab:AlethicPossibility", "alethic possibility", ["ab:ModalClaim"]),
    ], "evidence:ufo-ab")
    r = {}
    add_relations(r, [
        ("ab:containsTimePoint", "contains time point", ["ab:History"], ["ufo:TimePoint"]),
        ("ab:containsOccurrence", "contains occurrence", ["ab:History"], ["ab:Occurrence"]),
        ("ab:occursInHistory", "occurs in history", ["ab:Occurrence"], ["ab:History"], {"inverse": "ab:containsOccurrence", "chars": ["functional"]}),
        ("ab:branchesAt", "branches at", ["ab:TemporalBranch"], ["ab:BranchPoint"], {"chars": ["functional"]}),
        ("ab:branchesFrom", "branches from", ["ab:History"], ["ab:History"], {"chars": ["irreflexive", "asymmetric"]}),
        ("ab:incomparableWith", "is incomparable with", ["ab:History"], ["ab:History"], {"inverse": "ab:incomparableWith", "chars": ["irreflexive", "symmetric"]}),
        ("ab:actualizes", "actualizes", ["ab:ActualHistory"], ["ab:PossibleWorld"]),
        ("ab:supports", "supports", ["ab:History"], ["ab:ModalClaim"]),
        ("ab:necessarilySupports", "necessarily supports", ["ab:PossibleWorld"], ["ab:ModalClaim"]),
        ("ab:possiblySupports", "possibly supports", ["ab:PossibleWorld"], ["ab:ModalClaim"]),
        ("ab:presentAt", "is present at", ["ufo:Endurant"], ["ufo:TimePoint"]),
        ("ab:occursAt", "occurs at", ["ufo:Event"], ["ufo:TimePoint"]),
        ("ab:obtainsAt", "obtains at", ["ufo:Situation"], ["ufo:TimePoint"]),
        ("ab:causalSuccessor", "has causal successor", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:directlyCauses"]}),
        ("ab:temporalSuccessor", "has temporal successor", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:before"]}),
        ("ab:manifestsDisposition", "manifests disposition", ["ufo:Event"], ["ufo:Disposition"], {"supers": ["ufo:isManifestationOf"]}),
        ("ab:hasAlternative", "has alternative", ["ab:History"], ["ab:CounterfactualHistory"]),
        ("ab:sharesPastWith", "shares past with", ["ab:History"], ["ab:History"], {"inverse": "ab:sharesPastWith", "chars": ["symmetric"]}),
        ("ab:precedesOnHistory", "precedes on history", ["ufo:Event"], ["ufo:Event"], {"supers": ["ufo:before"]}),
    ], "evidence:ufo-ab")
    r["ab:containsOccurrence"]["inverse"] = "ab:occursInHistory"
    axioms = [
        partition("ab:History", ["ab:ActualHistory", "ab:CounterfactualHistory"]),
        partition("ab:ModalClaim", ["ab:AlethicNecessity", "ab:AlethicPossibility"]),
        card("ab:Occurrence", "ab:occursInHistory", "ab:History", 1, 1),
        card("ab:TemporalBranch", "ab:branchesAt", "ab:BranchPoint", 1, 1),
        exists("ab:History", "ab:containsTimePoint", "ufo:TimePoint"),
        rule([atom("ufo:bringsAbout", "?e1", "?s"), atom("ufo:triggers", "?s", "?e2")], atom("ab:causalSuccessor", "?e1", "?e2")),
        rule([atom("ab:branchesFrom", "?h1", "?h0"), atom("ab:branchesFrom", "?h2", "?h0")], atom("ab:sharesPastWith", "?h1", "?h2")),
    ]
    return ontology("muse.ufo.ab", "Muse UFO-AB", "Executable A/B bridge for branching histories, temporal presence, occurrences, modality and causal succession.", [exact(prov), exact(a), exact(b)], ["evidence:ufo-ab"], c, r, axioms)


def package_services(prov, c_ref):
    c = {}
    add_concepts(c, [
        ("service:ServiceThing", "service thing", ["ufo:Entity"]),
        ("service:ServiceAgent", "service agent", ["ufo:Agent", "service:ServiceThing"]),
        ("service:ServiceProvider", "service provider", ["service:ServiceAgent"]),
        ("service:TargetCustomer", "target customer", ["service:ServiceAgent"]),
        ("service:ServiceCustomer", "service customer", ["service:TargetCustomer"]),
        ("service:ServiceRole", "service role", ["ufo:SocialRole", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("service:ProviderRole", "provider role", ["service:ServiceRole"]),
        ("service:CustomerRole", "customer role", ["service:ServiceRole"]),
        ("service:TargetCustomerRoleMixin", "target customer role mixin", ["ufo:RoleMixin", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="non_sortal", identity="none")}),
        ("service:ServiceOffering", "service offering", ["ufo:SocialRelator", "service:ServiceThing"]),
        ("service:ServiceOfferingCommitment", "service offering commitment", ["ufo:SocialCommitment", "service:ServiceThing"]),
        ("service:ServiceOfferingClaim", "service offering claim", ["ufo:SocialClaim", "service:ServiceThing"]),
        ("service:ServiceNegotiation", "service negotiation", ["ufo:Interaction", "service:ServiceThing"]),
        ("service:ServiceAgreement", "service agreement", ["ufo:SocialRelator", "service:ServiceThing"]),
        ("service:ProviderCommitment", "provider commitment", ["ufo:SocialCommitment", "service:ServiceThing"]),
        ("service:ProviderClaim", "provider claim", ["ufo:SocialClaim", "service:ServiceThing"]),
        ("service:CustomerCommitment", "customer commitment", ["ufo:SocialCommitment", "service:ServiceThing"]),
        ("service:CustomerClaim", "customer claim", ["ufo:SocialClaim", "service:ServiceThing"]),
        ("service:ServiceDelivery", "service delivery", ["ufo:Action", "service:ServiceThing"]),
        ("service:ServiceParticipation", "service participation", ["ufo:ActionContribution", "service:ServiceThing"]),
        ("service:ProviderParticipation", "provider participation", ["service:ServiceParticipation"]),
        ("service:CustomerParticipation", "customer participation", ["service:ServiceParticipation"]),
        ("service:ServiceResult", "service result", ["ufo:Situation", "service:ServiceThing"]),
        ("service:SatisfactionSituation", "satisfaction situation", ["service:ServiceResult"]),
        ("service:FailureSituation", "failure situation", ["service:ServiceResult"]),
        ("service:ServiceDescription", "service description", ["ufo:NormativeDescription", "service:ServiceThing"]),
        ("service:ServiceSpecification", "service specification", ["service:ServiceDescription"]),
        ("service:ServiceLevelAgreement", "service-level agreement", ["service:ServiceAgreement"]),
        ("service:ServiceCapability", "service capability", ["ufo:Disposition", "service:ServiceThing"]),
        ("service:ServiceResource", "service resource", ["ufo:Resource", "service:ServiceThing"]),
        ("service:ServiceLifecycle", "service lifecycle", ["ufo:ComplexEvent", "service:ServiceThing"]),
        ("service:OfferingPhase", "offering phase", ["ufo:Phase", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("service:NegotiationPhase", "negotiation phase", ["ufo:Phase", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("service:DeliveryPhase", "delivery phase", ["ufo:Phase", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("service:PostDeliveryPhase", "post-delivery phase", ["ufo:Phase", "service:ServiceThing"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
    ], "evidence:ufo-s")
    r = {}
    add_relations(r, [
        ("service:offeredBy", "is offered by", ["service:ServiceOffering"], ["service:ServiceProvider"], {"chars": ["functional"]}),
        ("service:targets", "targets", ["service:ServiceOffering"], ["service:TargetCustomer"]),
        ("service:describedBy", "is described by", ["service:ServiceThing"], ["service:ServiceDescription"]),
        ("service:hasOfferingCommitment", "has offering commitment", ["service:ServiceOffering"], ["service:ServiceOfferingCommitment"]),
        ("service:hasOfferingClaim", "has offering claim", ["service:ServiceOffering"], ["service:ServiceOfferingClaim"]),
        ("service:negotiates", "negotiates", ["service:ServiceAgent"], ["service:ServiceNegotiation"]),
        ("service:negotiatesOffering", "negotiates offering", ["service:ServiceNegotiation"], ["service:ServiceOffering"]),
        ("service:resultsInAgreement", "results in agreement", ["service:ServiceNegotiation"], ["service:ServiceAgreement"]),
        ("service:agreementProvider", "has agreement provider", ["service:ServiceAgreement"], ["service:ServiceProvider"], {"chars": ["functional"]}),
        ("service:agreementCustomer", "has agreement customer", ["service:ServiceAgreement"], ["service:ServiceCustomer"], {"chars": ["functional"]}),
        ("service:hasProviderCommitment", "has provider commitment", ["service:ServiceAgreement"], ["service:ProviderCommitment"]),
        ("service:hasProviderClaim", "has provider claim", ["service:ServiceAgreement"], ["service:ProviderClaim"]),
        ("service:hasCustomerCommitment", "has customer commitment", ["service:ServiceAgreement"], ["service:CustomerCommitment"]),
        ("service:hasCustomerClaim", "has customer claim", ["service:ServiceAgreement"], ["service:CustomerClaim"]),
        ("service:deliversUnder", "delivers under", ["service:ServiceDelivery"], ["service:ServiceAgreement"]),
        ("service:performedByProvider", "is performed by provider", ["service:ServiceDelivery"], ["service:ServiceProvider"]),
        ("service:hasCustomerParticipation", "has customer participation", ["service:ServiceDelivery"], ["service:CustomerParticipation"]),
        ("service:hasProviderParticipation", "has provider participation", ["service:ServiceDelivery"], ["service:ProviderParticipation"]),
        ("service:usesServiceResource", "uses service resource", ["service:ServiceDelivery"], ["service:ServiceResource"]),
        ("service:producesResult", "produces result", ["service:ServiceDelivery"], ["service:ServiceResult"]),
        ("service:fulfillsCommitment", "fulfills commitment", ["service:ServiceDelivery"], ["ufo:SocialCommitment"], {"supers": ["ufo:fulfillsCommitment"]}),
        ("service:violatesCommitment", "violates commitment", ["service:ServiceDelivery"], ["ufo:SocialCommitment"], {"supers": ["ufo:violatesCommitment"]}),
        ("service:satisfiesAgreement", "satisfies agreement", ["service:SatisfactionSituation"], ["service:ServiceAgreement"]),
        ("service:failsAgreement", "fails agreement", ["service:FailureSituation"], ["service:ServiceAgreement"]),
        ("service:requiresCapability", "requires capability", ["service:ServiceOffering"], ["service:ServiceCapability"]),
        ("service:capabilityInheresIn", "capability inheres in", ["service:ServiceCapability"], ["service:ServiceProvider"], {"supers": ["ufo:inheresIn"], "chars": ["functional"]}),
        ("service:offeringPrecedesNegotiation", "offering precedes negotiation", ["service:ServiceOffering"], ["service:ServiceNegotiation"]),
        ("service:agreementPrecedesDelivery", "agreement precedes delivery", ["service:ServiceAgreement"], ["service:ServiceDelivery"]),
        ("service:deliveryPrecedesResult", "delivery precedes result", ["service:ServiceDelivery"], ["service:ServiceResult"]),
        ("service:partOfLifecycle", "is part of service lifecycle", ["service:ServiceThing"], ["service:ServiceLifecycle"]),
    ], "evidence:ufo-s")
    axioms = [
        partition("service:ServiceRole", ["service:ProviderRole", "service:CustomerRole"]),
        partition("service:ServiceParticipation", ["service:ProviderParticipation", "service:CustomerParticipation"]),
        partition("service:ServiceResult", ["service:SatisfactionSituation", "service:FailureSituation"]),
        disjoint("service:OfferingPhase", "service:NegotiationPhase", "service:DeliveryPhase", "service:PostDeliveryPhase"),
        card("service:ServiceOffering", "service:offeredBy", "service:ServiceProvider", 1, 1),
        exists("service:ServiceOffering", "service:targets", "service:TargetCustomer"),
        exists("service:ServiceOffering", "service:hasOfferingCommitment", "service:ServiceOfferingCommitment"),
        exists("service:ServiceOffering", "service:hasOfferingClaim", "service:ServiceOfferingClaim"),
        card("service:ServiceAgreement", "service:agreementProvider", "service:ServiceProvider", 1, 1),
        card("service:ServiceAgreement", "service:agreementCustomer", "service:ServiceCustomer", 1, 1),
        exists("service:ServiceAgreement", "service:hasProviderCommitment", "service:ProviderCommitment"),
        exists("service:ServiceAgreement", "service:hasCustomerCommitment", "service:CustomerCommitment"),
        exists("service:ServiceDelivery", "service:deliversUnder", "service:ServiceAgreement"),
        exists("service:ServiceDelivery", "service:producesResult", "service:ServiceResult"),
        rule([atom("service:producesResult", "?delivery", "?result")], atom("ufo:bringsAbout", "?delivery", "?result")),
        rule([atom("service:performedByProvider", "?delivery", "?provider")], atom("ufo:performs", "?provider", "?delivery")),
    ]
    return ontology("muse.ufo.services", "Muse UFO-S services", "Executable commitment-based service offering, negotiation, agreement, delivery, outcome and lifecycle package, including corrigendum corrections.", [exact(prov), exact(c_ref)], ["evidence:ufo-s", "evidence:ufo-s-corr"], c, r, axioms)


def package_legal(prov, c_ref):
    c = {}
    add_concepts(c, [
        ("legal:LegalThing", "legal thing", ["ufo:Entity"]),
        ("legal:LegalIndividual", "legal individual", ["ufo:Individual", "legal:LegalThing"]),
        ("legal:LegalType", "legal type", ["ufo:Type", "legal:LegalThing"], {"prof": profile(entity_mode="type")}),
        ("legal:LegalAgent", "legal agent", ["ufo:Agent", "legal:LegalIndividual"]),
        ("legal:NaturalLegalPerson", "natural legal person", ["legal:LegalAgent"]),
        ("legal:JuridicalPerson", "juridical person", ["legal:LegalAgent"]),
        ("legal:LegalObject", "legal object", ["ufo:SocialObject", "legal:LegalIndividual"]),
        ("legal:LegalDocument", "legal document", ["legal:LegalObject"]),
        ("legal:LegalNormativeDescription", "legal normative description", ["ufo:NormativeDescription", "legal:LegalObject"]),
        ("legal:LegalNorm", "legal norm", ["legal:LegalNormativeDescription"]),
        ("legal:LegalRule", "legal rule", ["legal:LegalNorm"]),
        ("legal:LegalPrinciple", "legal principle", ["legal:LegalNorm"]),
        ("legal:LegalPolicy", "legal policy", ["legal:LegalNorm"]),
        ("legal:LegalEvent", "legal event", ["ufo:Event", "legal:LegalIndividual"]),
        ("legal:LegalAct", "legal act", ["ufo:InstitutionalAct", "legal:LegalEvent"]),
        ("legal:JudicialAct", "judicial act", ["legal:LegalAct"]),
        ("legal:LegislativeAct", "legislative act", ["legal:LegalAct"]),
        ("legal:AdministrativeAct", "administrative act", ["legal:LegalAct"]),
        ("legal:LegalPosition", "legal position", ["ufo:SocialMoment", "legal:LegalIndividual"]),
        ("legal:ConductPosition", "conduct legal position", ["legal:LegalPosition"]),
        ("legal:CompetencePosition", "competence legal position", ["legal:LegalPosition"]),
        ("legal:ClaimRight", "claim-right", ["legal:ConductPosition"]),
        ("legal:Duty", "duty", ["legal:ConductPosition"]),
        ("legal:Permission", "permission", ["legal:ConductPosition"], {"alt": ["liberty", "privilege"]}),
        ("legal:NoRight", "no-right", ["legal:ConductPosition"]),
        ("legal:Power", "power", ["legal:CompetencePosition"]),
        ("legal:Subjection", "subjection", ["legal:CompetencePosition"], {"alt": ["liability"]}),
        ("legal:Immunity", "immunity", ["legal:CompetencePosition"]),
        ("legal:Disability", "disability", ["legal:CompetencePosition"]),
        ("legal:LegalRelator", "legal relator", ["ufo:SocialRelator", "legal:LegalIndividual"]),
        ("legal:SimpleLegalRelator", "simple legal relator", ["legal:LegalRelator"]),
        ("legal:ComplexLegalRelator", "complex legal relator", ["legal:LegalRelator"]),
        ("legal:RightDutyRelator", "right-duty relator", ["legal:SimpleLegalRelator"]),
        ("legal:NoRightPermissionRelator", "no-right-permission relator", ["legal:SimpleLegalRelator"]),
        ("legal:PowerSubjectionRelator", "power-subjection relator", ["legal:SimpleLegalRelator"]),
        ("legal:DisabilityImmunityRelator", "disability-immunity relator", ["legal:SimpleLegalRelator"]),
        ("legal:Liberty", "liberty", ["legal:ComplexLegalRelator"]),
        ("legal:LegalSubjectRole", "legal subject role", ["ufo:SocialRole", "legal:LegalType"], {"prof": profile(entity_mode="type", rigidity="anti_rigid", sortality="sortal", identity="carries")}),
        ("legal:RightHolderRole", "right-holder role", ["legal:LegalSubjectRole"]),
        ("legal:DutyHolderRole", "duty-holder role", ["legal:LegalSubjectRole"]),
        ("legal:PowerHolderRole", "power-holder role", ["legal:LegalSubjectRole"]),
        ("legal:SubjectedRole", "subjected role", ["legal:LegalSubjectRole"]),
        ("legal:ImmunityHolderRole", "immunity-holder role", ["legal:LegalSubjectRole"]),
        ("legal:DisabledRole", "disabled role", ["legal:LegalSubjectRole"]),
        ("legal:LegalSituation", "legal situation", ["ufo:Situation", "legal:LegalIndividual"]),
        ("legal:ComplianceSituation", "compliance situation", ["legal:LegalSituation"]),
        ("legal:ViolationSituation", "violation situation", ["legal:LegalSituation"]),
        ("legal:LegalFact", "legal fact", ["legal:LegalSituation"]),
        ("legal:LegalCase", "legal case", ["legal:ComplexLegalRelator"]),
        ("legal:JudicialDecision", "judicial decision", ["legal:LegalDocument"]),
        ("legal:LegalInterpretation", "legal interpretation", ["ufo:Proposition", "legal:LegalIndividual"]),
        ("legal:Contract", "contract", ["legal:ComplexLegalRelator"]),
        ("legal:Statute", "statute", ["legal:LegalDocument"]),
        ("legal:Regulation", "regulation", ["legal:LegalDocument"]),
        ("legal:Jurisdiction", "jurisdiction", ["legal:LegalObject"]),
        ("legal:Authority", "authority", ["legal:LegalAgent"]),
        ("legal:Competence", "competence", ["ufo:Disposition", "legal:LegalIndividual"]),
        ("legal:Sanction", "sanction", ["legal:LegalAct"]),
        ("legal:Remedy", "remedy", ["legal:LegalAct"]),
        ("legal:Omission", "omission", ["ufo:Event", "legal:LegalIndividual"]),
    ], "evidence:ufo-l")
    r = {}
    add_relations(r, [
        ("legal:definedIn", "is defined in", ["legal:LegalThing"], ["legal:LegalNormativeDescription"]),
        ("legal:recognizesLegalThing", "recognizes legal thing", ["legal:LegalNormativeDescription"], ["legal:LegalThing"]),
        ("legal:grounds", "grounds", ["legal:LegalNorm"], ["legal:LegalIndividual"]),
        ("legal:hasLegalContent", "has legal content", ["legal:LegalIndividual"], ["ufo:Proposition"]),
        ("legal:inheresInLegalSubject", "inheres in legal subject", ["legal:LegalPosition"], ["legal:LegalAgent"], {"supers": ["ufo:inheresIn"], "chars": ["functional"]}),
        ("legal:externallyDependsOnLegalSubject", "externally depends on legal subject", ["legal:LegalPosition"], ["legal:LegalAgent"], {"supers": ["ufo:externallyDependsOn"], "chars": ["functional"]}),
        ("legal:isLegalCorrelativeOf", "is legal correlative of", ["legal:LegalPosition"], ["legal:LegalPosition"], {"inverse": "legal:isLegalCorrelativeOf", "chars": ["irreflexive", "symmetric"]}),
        ("legal:isLegalOppositeOf", "is legal opposite of", ["legal:LegalPosition"], ["legal:LegalPosition"], {"inverse": "legal:isLegalOppositeOf", "chars": ["irreflexive", "symmetric"]}),
        ("legal:composesLegalRelator", "composes legal relator", ["legal:LegalPosition"], ["legal:LegalRelator"], {"supers": ["ufo:properPartOf"]}),
        ("legal:containsLegalRelator", "contains legal relator", ["legal:ComplexLegalRelator"], ["legal:LegalRelator"], {"inverse": "legal:legalRelatorPartOf", "supers": ["ufo:hasProperPart"]}),
        ("legal:legalRelatorPartOf", "is legal-relator part of", ["legal:LegalRelator"], ["legal:ComplexLegalRelator"], {"inverse": "legal:containsLegalRelator", "supers": ["ufo:properPartOf"]}),
        ("legal:mediatesLegalSubject", "mediates legal subject", ["legal:LegalRelator"], ["legal:LegalAgent"], {"supers": ["ufo:mediates"]}),
        ("legal:hasRightHolder", "has right holder", ["legal:RightDutyRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:hasDutyHolder", "has duty holder", ["legal:RightDutyRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:hasPowerHolder", "has power holder", ["legal:PowerSubjectionRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:hasSubjectedParty", "has subjected party", ["legal:PowerSubjectionRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:hasImmunityHolder", "has immunity holder", ["legal:DisabilityImmunityRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:hasDisabledParty", "has disabled party", ["legal:DisabilityImmunityRelator"], ["legal:LegalAgent"], {"chars": ["functional"]}),
        ("legal:createsLegalPosition", "creates legal position", ["legal:LegalAct"], ["legal:LegalPosition"]),
        ("legal:modifiesLegalPosition", "modifies legal position", ["legal:LegalAct"], ["legal:LegalPosition"]),
        ("legal:terminatesLegalPosition", "terminates legal position", ["legal:LegalAct"], ["legal:LegalPosition"]),
        ("legal:createsLegalRelator", "creates legal relator", ["legal:LegalAct"], ["legal:LegalRelator"]),
        ("legal:modifiesLegalRelator", "modifies legal relator", ["legal:LegalAct"], ["legal:LegalRelator"]),
        ("legal:terminatesLegalRelator", "terminates legal relator", ["legal:LegalAct"], ["legal:LegalRelator"]),
        ("legal:authorizes", "authorizes", ["legal:LegalNorm"], ["ufo:Action"]),
        ("legal:requires", "requires", ["legal:LegalNorm"], ["ufo:Proposition"]),
        ("legal:forbids", "forbids", ["legal:LegalNorm"], ["ufo:Proposition"]),
        ("legal:satisfiesDuty", "satisfies duty", ["legal:ComplianceSituation"], ["legal:Duty"]),
        ("legal:violatesDuty", "violates duty", ["legal:ViolationSituation"], ["legal:Duty"]),
        ("legal:declaresViolation", "declares violation", ["legal:LegalAct"], ["legal:ViolationSituation"]),
        ("legal:exercisesPower", "exercises power", ["legal:LegalAct"], ["legal:Power"]),
        ("legal:appliesNorm", "applies norm", ["legal:LegalAct"], ["legal:LegalNorm"]),
        ("legal:interprets", "interprets", ["legal:LegalAgent"], ["legal:LegalNorm"]),
        ("legal:hasInterpretation", "has interpretation", ["legal:LegalNorm"], ["legal:LegalInterpretation"]),
        ("legal:withinJurisdiction", "is within jurisdiction", ["legal:LegalThing"], ["legal:Jurisdiction"]),
        ("legal:hasAuthority", "has authority", ["legal:LegalAgent"], ["legal:Competence"]),
        ("legal:imposesSanction", "imposes sanction", ["legal:Authority"], ["legal:Sanction"]),
        ("legal:grantsRemedy", "grants remedy", ["legal:Authority"], ["legal:Remedy"]),
        ("legal:decidesCase", "decides case", ["legal:JudicialAct"], ["legal:LegalCase"]),
        ("legal:recordsDecision", "records decision", ["legal:JudicialAct"], ["legal:JudicialDecision"]),
    ], "evidence:ufo-l")
    axioms = [
        partition("legal:LegalPosition", ["legal:ConductPosition", "legal:CompetencePosition"]),
        partition("legal:ConductPosition", ["legal:ClaimRight", "legal:Duty", "legal:Permission", "legal:NoRight"]),
        partition("legal:CompetencePosition", ["legal:Power", "legal:Subjection", "legal:Immunity", "legal:Disability"]),
        partition("legal:LegalRelator", ["legal:SimpleLegalRelator", "legal:ComplexLegalRelator"]),
        partition("legal:SimpleLegalRelator", ["legal:RightDutyRelator", "legal:NoRightPermissionRelator", "legal:PowerSubjectionRelator", "legal:DisabilityImmunityRelator"]),
        partition("legal:LegalSituation", ["legal:ComplianceSituation", "legal:ViolationSituation"]),
        partition("legal:LegalAgent", ["legal:NaturalLegalPerson", "legal:JuridicalPerson"]),
        card("legal:LegalPosition", "legal:inheresInLegalSubject", "legal:LegalAgent", 1, 1),
        card("legal:LegalPosition", "legal:externallyDependsOnLegalSubject", "legal:LegalAgent", 1, 1),
        card("legal:RightDutyRelator", "legal:hasRightHolder", "legal:LegalAgent", 1, 1),
        card("legal:RightDutyRelator", "legal:hasDutyHolder", "legal:LegalAgent", 1, 1),
        card("legal:PowerSubjectionRelator", "legal:hasPowerHolder", "legal:LegalAgent", 1, 1),
        card("legal:PowerSubjectionRelator", "legal:hasSubjectedParty", "legal:LegalAgent", 1, 1),
        card("legal:DisabilityImmunityRelator", "legal:hasImmunityHolder", "legal:LegalAgent", 1, 1),
        card("legal:DisabilityImmunityRelator", "legal:hasDisabledParty", "legal:LegalAgent", 1, 1),
        exists("legal:ClaimRight", "legal:isLegalCorrelativeOf", "legal:Duty"),
        exists("legal:Duty", "legal:isLegalCorrelativeOf", "legal:ClaimRight"),
        exists("legal:Permission", "legal:isLegalCorrelativeOf", "legal:NoRight"),
        exists("legal:NoRight", "legal:isLegalCorrelativeOf", "legal:Permission"),
        exists("legal:Power", "legal:isLegalCorrelativeOf", "legal:Subjection"),
        exists("legal:Subjection", "legal:isLegalCorrelativeOf", "legal:Power"),
        exists("legal:Immunity", "legal:isLegalCorrelativeOf", "legal:Disability"),
        exists("legal:Disability", "legal:isLegalCorrelativeOf", "legal:Immunity"),
        rule([atom("legal:composesLegalRelator", "?position", "?relator")], atom("ufo:properPartOf", "?position", "?relator")),
        rule([atom("legal:mediatesLegalSubject", "?relator", "?agent")], atom("ufo:mediates", "?relator", "?agent")),
    ]
    return ontology("muse.ufo.legal", "Muse UFO-L legal", "Executable legal norms, agents, acts, Hohfeldian conduct and competence positions, legal relators, compliance, violation and institutional change package.", [exact(prov), exact(c_ref)], ["evidence:ufo-l", "evidence:ufo-l-judicial", "evidence:ufo-l-powers"], c, r, axioms)


def slug(value):
    value = re.sub(r"([a-z0-9])([A-Z])", r"\1-\2", value).lower()
    return re.sub(r"[^a-z0-9]+", "-", value).strip("-") or "item"


def lexicon(prov, refs, ontology_files):
    concepts, relations = {}, {}
    for path in ontology_files:
        pkg = json.loads(path.read_text())["payload"]["document"]["package"]
        concepts.update(pkg["concepts"]); relations.update(pkg["relations"])
    entries, senses, frames, attestations = {}, {}, {}, {}
    used = set()
    def eid(label, pos):
        base = f"entry:en:{slug(label)}:{pos}"; candidate = base; n = 2
        while candidate in used: candidate = f"{base}-{n}"; n += 1
        used.add(candidate); return candidate
    for cid, decl in sorted(concepts.items()):
        label = decl["preferred_labels"]["en"]; entry = eid(label, "noun"); sense = f"sense:en:{slug(cid)}"
        forms = [label, *decl.get("alternate_labels", {}).get("en", [])]
        entries[entry] = {"id": entry, "language": "en", "lemma": label, "part_of_speech": "noun", "forms": [{"written": form, "kind": "lemma" if form == label else "variant", "features": {}, "script": "Latn"} for form in dict.fromkeys(forms)], "senses": [sense], "deprecated": False, "notes": []}
        senses[sense] = {"id": sense, "entry": entry, "definition": decl["definition"]["text"], "target": {"target": "concept", "id": cid}, "usage": {"domains": ["foundational ontology"], "registers": ["technical"], "communities": ["ontology engineering"], "jurisdictions": [], "temporal_scope": None, "requires_quotation": False, "notes": []}, "relations": [], "evidence": decl["evidence"], "deprecated": False}
        aid = f"attestation:en:{slug(cid)}"
        attestations[aid] = {"id": aid, "sense": sense, "kind": "positive", "excerpt": label, "source": decl["evidence"][0], "locator": cid, "register": "technical", "domain": "foundational ontology", "annotations": {"attestation_type": "source-derived preferred designation", "status": "terminological attestation, not corpus-frequency evidence"}}
    for rid, decl in sorted(relations.items()):
        label = decl["preferred_labels"]["en"]; entry = eid(label, "verb"); sense = f"sense:en:{slug(rid)}"
        entries[entry] = {"id": entry, "language": "en", "lemma": label, "part_of_speech": "verb", "forms": [{"written": label, "kind": "lemma", "features": {}, "script": "Latn"}], "senses": [sense], "deprecated": False, "notes": []}
        senses[sense] = {"id": sense, "entry": entry, "definition": decl["definition"], "target": {"target": "relation", "id": rid}, "usage": {"domains": ["foundational ontology"], "registers": ["technical"], "communities": ["ontology engineering"], "jurisdictions": [], "temporal_scope": None, "requires_quotation": False, "notes": []}, "relations": [], "evidence": decl["evidence"], "deprecated": False}
        domains = decl["domains"] or ["ufo:Entity"]; ranges = decl["ranges"] or ["ufo:Entity"]
        fid = f"frame:en:{slug(rid)}"
        frames[fid] = {"id": fid, "sense": sense, "arguments": [
            {"id": "subject", "semantic_role": "subject", "syntactic_realizations": ["subject", "agent", "source"], "selection": [{"required_type": domains[0], "allow_subtypes": True, "negated": False}], "optional": False, "repeated": False},
            {"id": "object", "semantic_role": "object", "syntactic_realizations": ["object", "complement", "target"], "selection": [{"required_type": ranges[0], "allow_subtypes": True, "negated": False}], "optional": False, "repeated": False},
        ], "patterns": [{"pattern": f"SUBJECT {label} OBJECT", "voice": "active", "construction": "binary predicate", "notes": []}], "presuppositions": [], "result_conditions": [], "evidence": decl["evidence"]}
        aid = f"attestation:en:{slug(rid)}"
        attestations[aid] = {"id": aid, "sense": sense, "kind": "positive", "excerpt": label, "source": decl["evidence"][0], "locator": rid, "register": "technical", "domain": "foundational ontology", "annotations": {"attestation_type": "source-derived preferred designation", "status": "terminological attestation, not corpus-frequency evidence"}}
    return {"header": header("muse.ufo.en.lexicon", "lexicon", "Muse UFO-family English lexicon", "English designation, sense, attestation and predicate-frame coverage for every bundled UFO-family concept and relation.", [exact(x) for x in [*refs, prov]], ["evidence:ufo22", "evidence:ufo-b", "evidence:ufo-c", "evidence:mlt", "evidence:ufo-ab", "evidence:ufo-s", "evidence:ufo-l"]), "ontology_packages": sorted(refs, key=lambda x: (x["id"], x["version"], x["digest"]["value"])), "entries": dict(sorted(entries.items())), "senses": dict(sorted(senses.items())), "frames": dict(sorted(frames.items())), "attestations": dict(sorted(attestations.items()))}


def main(output_dir: Path | None = None):
    global OUT
    if output_dir is not None:
        OUT = output_dir
    OUT.mkdir(parents=True, exist_ok=True)
    for path in OUT.glob("*.muse.json"):
        path.unlink()
    prov = seal("provenance", provenance_package(), "ufo-provenance.muse.json")
    a = seal("ontology", package_a(prov), "ufo-a.muse.json")
    b = seal("ontology", package_b(prov, a), "ufo-b.muse.json")
    c = seal("ontology", package_c(prov, a, b), "ufo-c.muse.json")
    mlt = seal("ontology", package_mlt(prov, a), "ufo-mlt.muse.json")
    ab = seal("ontology", package_ab(prov, a, b), "ufo-ab.muse.json")
    services = seal("ontology", package_services(prov, c), "ufo-services.muse.json")
    legal = seal("ontology", package_legal(prov, c), "ufo-legal.muse.json")
    files = [OUT / name for name in [
        "ufo-a.muse.json", "ufo-b.muse.json", "ufo-c.muse.json",
        "ufo-mlt.muse.json", "ufo-ab.muse.json",
        "ufo-services.muse.json", "ufo-legal.muse.json",
    ]]
    lex = lexicon(prov, [a, b, c, mlt, ab, services, legal], files)
    seal("lexicon", lex, "ufo-en-lexicon.muse.json")
    print(f"generated {len(list(OUT.glob('*.muse.json')))} UFO package files")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="write generated packages to this directory instead of packages/ufo",
    )
    args = parser.parse_args()
    main(args.output_dir)
