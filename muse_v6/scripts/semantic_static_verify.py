#!/usr/bin/env python3
"""Deterministic static verification for the standalone Muse semantic workspace."""
from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import tomllib
from collections import defaultdict

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_DIR = ROOT / "packages" / "ufo"
ERRORS: list[str] = []

EXPECTED_PACKAGE_IDS = {
    "muse.ufo.provenance",
    "muse.ufo.a",
    "muse.ufo.b",
    "muse.ufo.c",
    "muse.ufo.mlt",
    "muse.ufo.ab",
    "muse.ufo.services",
    "muse.ufo.legal",
    "muse.ufo.en.lexicon",
}
EXPECTED_COUNTS = {
    "concepts": 266,
    "relations": 194,
    "axioms": 110,
    "entries": 460,
    "senses": 460,
    "frames": 194,
    "attestations": 460,
    "evidence": 12,
}
REQUIRED_CONCEPTS = {
    "ufo:Entity", "ufo:Endurant", "ufo:Perdurant", "ufo:Relator",
    "ufo:Event", "ufo:Situation", "ufo:Agent", "ufo:SocialCommitment",
    "mlt:FirstOrderType", "mlt:HigherOrderType", "mlt:Powertype",
    "ab:History", "ab:TemporalBranch", "ab:CounterfactualHistory",
    "service:ServiceOffering", "service:ServiceAgreement", "service:ServiceDelivery",
    "legal:LegalNorm", "legal:LegalRelator", "legal:ClaimRight", "legal:Duty",
    "legal:Liberty", "legal:NoRight", "legal:Power", "legal:Subjection",
    "legal:Immunity", "legal:Disability",
}
REQUIRED_RELATIONS = {
    "ufo:instantiates", "ufo:specializes", "ufo:inheresIn", "ufo:mediates",
    "ufo:triggers", "ufo:bringsAbout", "ufo:directlyCauses",
    "ufo:performs", "ufo:hasContent", "ufo:isCorrelativeOf",
    "mlt:partitions", "mlt:isPowertypeOf", "ab:causalSuccessor",
    "service:offeredBy", "service:resultsInAgreement", "service:deliversUnder",
    "legal:grounds", "legal:createsLegalPosition",
}
VALID_ENTITY_MODES = {None, "individual", "type", "either"}
VALID_RIGIDITY = {None, "rigid", "anti_rigid", "semi_rigid"}
VALID_SORTALITY = {None, "sortal", "non_sortal"}
VALID_IDENTITY = {None, "supplies", "carries", "none", "unknown"}
VALID_ABSTRACTNESS = {None, "abstract", "concrete", "mixed", "unspecified"}
VALID_CHARACTERISTICS = {
    "reflexive", "irreflexive", "symmetric", "asymmetric", "antisymmetric",
    "transitive", "functional", "inverse_functional",
}


def error(message: str) -> None:
    ERRORS.append(message)


def canonical_bytes(value: object) -> bytes:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def package_key(ref: dict) -> tuple[str, str, str, str]:
    return (ref["id"], ref["version"], ref["digest"]["algorithm"], ref["digest"]["value"])


def check_workspace() -> None:
    cargo_path = ROOT / "Cargo.toml"
    cargo = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
    members = cargo["workspace"]["members"]
    if "crates/muse-reasoning" not in members:
        error("muse-reasoning is missing from workspace members")
    if len(members) != len(set(members)):
        error("workspace contains duplicate members")
    for member in members:
        if not (ROOT / member / "Cargo.toml").is_file():
            error(f"workspace member lacks Cargo.toml: {member}")
    manifests = [cargo_path, *sorted(ROOT.glob("crates/muse-*/Cargo.toml")), ROOT / "crates" / "muse" / "Cargo.toml"]
    for manifest in manifests:
        text = manifest.read_text(encoding="utf-8")
        data = tomllib.loads(text)
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for name, value in data.get(section, {}).items():
                if isinstance(value, dict) and "path" in value:
                    target = (manifest.parent / value["path"]).resolve()
                    if not (target / "Cargo.toml").is_file():
                        error(f"missing path dependency {name} from {manifest.relative_to(ROOT)}: {target}")


def strip_rust(text: str) -> str:
    out: list[str] = []
    i = 0
    state = "code"
    depth = 0
    while i < len(text):
        ch = text[i]
        nxt = text[i + 1] if i + 1 < len(text) else ""
        if state == "code":
            if ch == "/" and nxt == "/":
                state = "line"; i += 2; continue
            if ch == "/" and nxt == "*":
                state = "block"; depth = 1; i += 2; continue
            if ch == '"':
                state = "string"; out.append(" "); i += 1; continue
            out.append(ch); i += 1
        elif state == "line":
            if ch == "\n": out.append("\n"); state = "code"
            i += 1
        elif state == "block":
            if ch == "/" and nxt == "*": depth += 1; i += 2
            elif ch == "*" and nxt == "/":
                depth -= 1; i += 2
                if depth == 0: state = "code"
            else: i += 1
        else:
            if ch == "\\": i += 2
            elif ch == '"': state = "code"; out.append(" "); i += 1
            else: out.append("\n" if ch == "\n" else " "); i += 1
    return "".join(out)


def check_rust() -> None:
    pairs = {"(": ")", "[": "]", "{": "}"}
    close = {v: k for k, v in pairs.items()}
    for path in sorted([*ROOT.glob("crates/muse-*/**/*.rs"), *ROOT.glob("crates/muse/**/*.rs")]):
        raw = path.read_text(encoding="utf-8")
        cleaned = strip_rust(raw)
        stack: list[tuple[str, int]] = []
        for pos, ch in enumerate(cleaned):
            if ch in pairs: stack.append((ch, pos))
            elif ch in close:
                if not stack or stack[-1][0] != close[ch]:
                    error(f"unbalanced {ch} in {path.relative_to(ROOT)} at byte {pos}"); break
                stack.pop()
        if stack: error(f"unclosed delimiters in {path.relative_to(ROOT)}: {stack[-3:]}")
        if "#![forbid(unsafe_code)]" not in raw:
            error(f"missing unsafe-code prohibition: {path.relative_to(ROOT)}")
        if re.search(r"if\s+let[^\n]*\n\s*&&", raw):
            error(f"Rust newer-than-1.85 let-chain syntax: {path.relative_to(ROOT)}")


def detect_cycle(edges: dict[str, set[str]], label: str) -> None:
    state: dict[str, int] = {}
    def visit(node: str, trail: list[str]) -> None:
        mark = state.get(node, 0)
        if mark == 2: return
        if mark == 1:
            error(f"{label} cycle: {' -> '.join([*trail, node])}"); return
        state[node] = 1
        for parent in sorted(edges.get(node, set())):
            visit(parent, [*trail, node])
        state[node] = 2
    for node in sorted(edges): visit(node, [])


def check_atom(atom: dict, concepts: set[str], relations: set[str], where: str) -> set[str]:
    predicate = atom.get("predicate")
    args = atom.get("arguments")
    if not isinstance(args, list):
        error(f"rule atom arguments are not a list in {where}"); return set()
    if predicate == "instance_of":
        if len(args) != 2 or str(args[1]).startswith("?") or args[1] not in concepts:
            error(f"invalid instance_of atom {atom} in {where}")
    elif predicate not in relations or len(args) != 2:
        error(f"invalid relation atom {atom} in {where}")
    return {arg for arg in args if isinstance(arg, str) and arg.startswith("?")}


def check_packages() -> None:
    paths = sorted(PACKAGE_DIR.glob("*.muse.json"))
    if len(paths) != 9:
        error(f"expected 9 sealed UFO packages, found {len(paths)}")
    documents: dict[tuple[str, str, str, str], tuple[Path, dict]] = {}
    by_id: dict[str, tuple[Path, dict]] = {}
    concepts: dict[str, tuple[Path, dict]] = {}
    relations: dict[str, tuple[Path, dict]] = {}
    evidence: set[str] = set()
    lexicons: list[tuple[Path, dict]] = []
    axiom_records: list[tuple[Path, dict]] = []
    totals = defaultdict(int)

    for path in paths:
        try: data = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:
            error(f"invalid JSON {path.relative_to(ROOT)}: {exc}"); continue
        if data.get("format_version") != "muse-json-1": error(f"unsupported format in {path.relative_to(ROOT)}")
        payload = data.get("payload", {})
        actual_doc = hashlib.sha256(canonical_bytes(payload)).hexdigest()
        recorded = data.get("document_digest", {})
        if recorded != {"algorithm": "sha256", "value": actual_doc}:
            error(f"document digest mismatch: {path.relative_to(ROOT)}")
        if payload.get("document_kind") != "package":
            error(f"not a package document: {path.relative_to(ROOT)}"); continue
        document = payload.get("document", {})
        kind = document.get("kind")
        package = document.get("package", {})
        header = package.get("header", {})
        if header.get("kind") != kind: error(f"package/header kind mismatch: {path.relative_to(ROOT)}")
        ref = header.get("package", {})
        if not {"id", "version", "digest"}.issubset(ref):
            error(f"invalid package reference in {path.relative_to(ROOT)}"); continue
        normalized = copy.deepcopy(document)
        normalized["package"]["header"]["package"]["digest"] = {"algorithm": "sha256", "value": "0" * 64}
        expected_content = hashlib.sha256(canonical_bytes(normalized)).hexdigest()
        if ref["digest"] != {"algorithm": "sha256", "value": expected_content}:
            error(f"package content digest mismatch: {path.relative_to(ROOT)}")
        key = package_key(ref)
        if key in documents: error(f"duplicate package reference {ref}")
        if ref["id"] in by_id: error(f"duplicate package id {ref['id']}")
        documents[key] = (path, document); by_id[ref["id"]] = (path, document)
        for ev in header.get("evidence", []):
            if not isinstance(ev, str): error(f"invalid header evidence in {path.relative_to(ROOT)}")

        if kind == "ontology":
            pcs = package.get("concepts", {}); prs = package.get("relations", {}); axs = package.get("axioms", [])
            totals["concepts"] += len(pcs); totals["relations"] += len(prs); totals["axioms"] += len(axs)
            for cid, decl in pcs.items():
                if cid != decl.get("id"): error(f"concept key mismatch {cid}")
                if cid in concepts: error(f"duplicate concept {cid}")
                concepts[cid] = (path, decl)
            for rid, decl in prs.items():
                if rid != decl.get("id"): error(f"relation key mismatch {rid}")
                if rid in relations: error(f"duplicate relation {rid}")
                relations[rid] = (path, decl)
            axiom_records.extend((path, axiom) for axiom in axs)
        elif kind == "provenance":
            records = package.get("evidence", {})
            totals["evidence"] += len(records)
            evidence.update(records)
        elif kind == "lexicon":
            lexicons.append((path, package))
            for field in ("entries", "senses", "frames", "attestations"):
                totals[field] += len(package.get(field, {}))
        else:
            error(f"unknown package kind {kind!r} in {path.relative_to(ROOT)}")

    found_ids = set(by_id)
    if found_ids != EXPECTED_PACKAGE_IDS:
        error(f"package IDs differ; missing={sorted(EXPECTED_PACKAGE_IDS-found_ids)}, extra={sorted(found_ids-EXPECTED_PACKAGE_IDS)}")

    for _, (path, document) in documents.items():
        for req in document["package"]["header"].get("imports", []):
            matches = [k for k in documents if k[0] == req.get("id")]
            if req.get("version") is not None: matches = [k for k in matches if k[1] == req["version"]]
            if req.get("digest") is not None:
                dg = req["digest"]; matches = [k for k in matches if k[2] == dg.get("algorithm") and k[3] == dg.get("value")]
            if not matches and not req.get("optional", False): error(f"unsatisfied import in {path.relative_to(ROOT)}: {req}")
            if len(matches) > 1: error(f"ambiguous import in {path.relative_to(ROOT)}: {req}")

    concept_ids = set(concepts); relation_ids = set(relations)
    concept_edges: dict[str, set[str]] = defaultdict(set)
    relation_edges: dict[str, set[str]] = defaultdict(set)
    declared_disjoint: set[tuple[str, str]] = set()

    for cid, (path, decl) in concepts.items():
        labels = decl.get("preferred_labels", {})
        if not labels or not all(str(v).strip() == v and v for v in labels.values()): error(f"invalid labels on {cid}")
        definition = decl.get("definition", {})
        if not str(definition.get("text", "")).strip(): error(f"missing definition on {cid}")
        parents = set(decl.get("parents", [])); concept_edges[cid].update(parents)
        for parent in parents:
            if parent not in concept_ids: error(f"unknown parent {parent} of {cid} in {path.relative_to(ROOT)}")
        genus = definition.get("genus")
        if genus is not None and genus not in concept_ids: error(f"unknown genus {genus} of {cid}")
        for other in decl.get("disjoint_with", []):
            if other not in concept_ids: error(f"unknown disjoint concept {other} of {cid}")
            if other == cid: error(f"self-disjoint concept {cid}")
            declared_disjoint.add(tuple(sorted((cid, other))))
        prof = decl.get("profile", {})
        if prof.get("entity_mode") not in VALID_ENTITY_MODES: error(f"invalid entity_mode on {cid}")
        if prof.get("rigidity") not in VALID_RIGIDITY: error(f"invalid rigidity on {cid}")
        if prof.get("sortality") not in VALID_SORTALITY: error(f"invalid sortality on {cid}")
        if prof.get("identity") not in VALID_IDENTITY: error(f"invalid identity profile on {cid}")
        if prof.get("abstractness") not in VALID_ABSTRACTNESS: error(f"invalid abstractness on {cid}")
        for ev in decl.get("evidence", []):
            if ev not in evidence: error(f"unknown evidence {ev} on concept {cid}")

    for rid, (path, decl) in relations.items():
        if not decl.get("preferred_labels") or not str(decl.get("definition", "")).strip(): error(f"missing label/definition on {rid}")
        for concept in [*decl.get("domains", []), *decl.get("ranges", [])]:
            if concept not in concept_ids: error(f"unknown concept {concept} on relation {rid} in {path.relative_to(ROOT)}")
        supers = set(decl.get("super_relations", [])); relation_edges[rid].update(supers)
        for parent in supers:
            if parent not in relation_ids: error(f"unknown super-relation {parent} of {rid}")
        inverse = decl.get("inverse")
        if inverse is not None:
            if inverse not in relation_ids: error(f"unknown inverse {inverse} of {rid}")
            elif relations[inverse][1].get("inverse") != rid: error(f"non-reciprocal inverse {rid} <-> {inverse}")
        chars = set(decl.get("characteristics", []))
        if not chars <= VALID_CHARACTERISTICS: error(f"unknown relation characteristics on {rid}: {sorted(chars-VALID_CHARACTERISTICS)}")
        if {"reflexive", "irreflexive"} <= chars or {"symmetric", "asymmetric"} <= chars:
            error(f"contradictory relation characteristics on {rid}")
        card = decl.get("cardinality")
        if card is not None:
            lo, hi = card.get("minimum"), card.get("maximum")
            if lo is not None and (not isinstance(lo, int) or lo < 0): error(f"invalid minimum cardinality on {rid}")
            if hi is not None and (not isinstance(hi, int) or hi < 0): error(f"invalid maximum cardinality on {rid}")
            if lo is not None and hi is not None and lo > hi: error(f"minimum exceeds maximum on {rid}")
        for ev in decl.get("evidence", []):
            if ev not in evidence: error(f"unknown evidence {ev} on relation {rid}")

    for path, axiom in axiom_records:
        kind = axiom.get("kind"); where = f"{path.relative_to(ROOT)}:{kind}"
        if kind == "subsumption":
            if axiom.get("child") not in concept_ids or axiom.get("parent") not in concept_ids: error(f"invalid subsumption in {where}")
            else: concept_edges[axiom["child"]].add(axiom["parent"])
        elif kind in {"disjoint", "equivalent"}:
            members = axiom.get("members", [])
            if len(set(members)) < 2 or any(x not in concept_ids for x in members): error(f"invalid {kind} in {where}")
            elif kind == "disjoint":
                for index, left in enumerate(members):
                    for right in members[index + 1:]:
                        declared_disjoint.add(tuple(sorted((left, right))))
        elif kind == "complete_partition":
            parent, members = axiom.get("parent"), axiom.get("members", [])
            if parent not in concept_ids or len(set(members)) < 2 or any(x not in concept_ids for x in members): error(f"invalid partition in {where}")
            else:
                for member in members: concept_edges[member].add(parent)
                for index, left in enumerate(members):
                    for right in members[index + 1:]:
                        declared_disjoint.add(tuple(sorted((left, right))))
        elif kind == "relation_subsumption":
            child, parent = axiom.get("child"), axiom.get("parent")
            if child not in relation_ids or parent not in relation_ids: error(f"invalid relation subsumption in {where}")
            else: relation_edges[child].add(parent)
        elif kind in {"domain", "range"}:
            if axiom.get("relation") not in relation_ids or axiom.get("concept") not in concept_ids: error(f"invalid {kind} axiom in {where}")
        elif kind in {"existential_restriction", "universal_restriction"}:
            if axiom.get("subject") not in concept_ids or axiom.get("relation") not in relation_ids or axiom.get("object") not in concept_ids: error(f"invalid {kind} in {where}")
        elif kind == "cardinality":
            if axiom.get("subject") not in concept_ids or axiom.get("relation") not in relation_ids or (axiom.get("object") is not None and axiom.get("object") not in concept_ids): error(f"invalid cardinality references in {where}")
            cons = axiom.get("constraint", {}); lo, hi = cons.get("minimum"), cons.get("maximum")
            if lo is not None and hi is not None and lo > hi: error(f"invalid cardinality bounds in {where}")
        elif kind == "rule":
            rule = axiom.get("rule", {}); premises = rule.get("premises", [])
            if not premises: error(f"rule has no premises in {where}")
            used: set[str] = set()
            for atom in [*premises, rule.get("conclusion", {})]: used |= check_atom(atom, concept_ids, relation_ids, where)
            if set(rule.get("variables", [])) != used: error(f"rule variable mismatch in {where}: declared={rule.get('variables')}, used={sorted(used)}")
        else: error(f"unknown axiom kind {kind!r} in {where}")

    detect_cycle(concept_edges, "concept hierarchy")
    detect_cycle(relation_edges, "relation hierarchy")

    ancestor_cache: dict[str, set[str]] = {}
    def ancestors(concept: str) -> set[str]:
        if concept in ancestor_cache:
            return ancestor_cache[concept]
        result = set(concept_edges.get(concept, set()))
        for parent in list(result):
            result.update(ancestors(parent))
        ancestor_cache[concept] = result
        return result

    def semantically_disjoint(left: str, right: str) -> bool:
        return any(
            tuple(sorted((left_member, right_member))) in declared_disjoint
            for left_member in {left, *ancestors(left)}
            for right_member in {right, *ancestors(right)}
        )

    for concept in sorted(concept_ids):
        lineage = sorted({concept, *ancestors(concept)})
        for index, left in enumerate(lineage):
            for right in lineage[index + 1:]:
                if semantically_disjoint(left, right):
                    error(f"unsatisfiable concept {concept}: lineage contains disjoint {left} and {right}")
    for relation, (_, declaration) in relations.items():
        for side in ("domains", "ranges"):
            values = declaration.get(side, [])
            for index, left in enumerate(values):
                for right in values[index + 1:]:
                    if semantically_disjoint(left, right):
                        error(f"unsatisfiable relation {relation}: conjunctive {side} contain disjoint {left} and {right}")

    if len(lexicons) != 1: error(f"expected one bundled lexicon, found {len(lexicons)}")
    concept_targets: set[str] = set(); relation_targets: set[str] = set()
    for path, package in lexicons:
        registered = set(documents)
        for ref in package.get("ontology_packages", []):
            if package_key(ref) not in registered: error(f"lexicon requires unavailable ontology package {ref}")
        entries = package.get("entries", {}); senses = package.get("senses", {}); frames = package.get("frames", {}); attestations = package.get("attestations", {})
        sense_attestations: defaultdict[str, int] = defaultdict(int)
        for aid, att in attestations.items():
            if aid != att.get("id"): error(f"attestation key mismatch {aid}")
            if att.get("sense") not in senses: error(f"attestation {aid} references unknown sense")
            else: sense_attestations[att["sense"]] += 1
            if att.get("source") not in evidence: error(f"attestation {aid} references unknown evidence")
        for eid, entry in entries.items():
            if eid != entry.get("id"): error(f"entry key mismatch {eid}")
            if not entry.get("forms"): error(f"entry {eid} has no forms")
            for sid in entry.get("senses", []):
                if sid not in senses: error(f"entry {eid} references unknown sense {sid}")
        for sid, sense in senses.items():
            if sid != sense.get("id") or sense.get("entry") not in entries: error(f"invalid sense identity/entry {sid}")
            target = sense.get("target", {}); target_kind = target.get("target"); target_id = target.get("id")
            if target_kind == "concept":
                if target_id not in concept_ids: error(f"sense {sid} targets unknown concept {target_id}")
                concept_targets.add(target_id)
            elif target_kind == "relation":
                if target_id not in relation_ids: error(f"sense {sid} targets unknown relation {target_id}")
                relation_targets.add(target_id)
            else: error(f"unsupported bundled sense target {target_kind!r} on {sid}")
            for ev in sense.get("evidence", []):
                if ev not in evidence: error(f"sense {sid} references unknown evidence {ev}")
            if sense_attestations[sid] == 0: error(f"sense {sid} has no attestation")
        framed_relations: set[str] = set()
        for fid, frame in frames.items():
            if fid != frame.get("id") or frame.get("sense") not in senses: error(f"invalid frame {fid}")
            sense = senses.get(frame.get("sense"), {}); target = sense.get("target", {})
            if target.get("target") != "relation": error(f"frame {fid} does not belong to a relation sense")
            else: framed_relations.add(target.get("id"))
            for arg in frame.get("arguments", []):
                for selection in arg.get("selection", []):
                    if selection.get("required_type") not in concept_ids: error(f"frame {fid} references unknown type {selection.get('required_type')}")
        if framed_relations != relation_ids: error(f"predicate-frame coverage differs: missing={sorted(relation_ids-framed_relations)}, extra={sorted(framed_relations-relation_ids)}")
    if concept_targets != concept_ids: error(f"concept lexical coverage differs: missing={sorted(concept_ids-concept_targets)}, extra={sorted(concept_targets-concept_ids)}")
    if relation_targets != relation_ids: error(f"relation lexical coverage differs: missing={sorted(relation_ids-relation_targets)}, extra={sorted(relation_targets-relation_ids)}")

    for key, expected in EXPECTED_COUNTS.items():
        if totals[key] != expected: error(f"expected {expected} {key}, found {totals[key]}")
    missing_concepts = REQUIRED_CONCEPTS - concept_ids
    missing_relations = REQUIRED_RELATIONS - relation_ids
    if missing_concepts: error(f"required concepts missing: {sorted(missing_concepts)}")
    if missing_relations: error(f"required relations missing: {sorted(missing_relations)}")


def check_bootstrap() -> None:
    required = {"ufo-core.muse.json", "ufo-social.muse.json", "ufo-mlt.muse.json"}
    found = {path.name for path in (ROOT / "packages" / "sources" / "bootstrap").glob("*.muse.json")}
    if found != required: error(f"bootstrap seed set differs: expected={sorted(required)}, found={sorted(found)}")


def check_manifest() -> None:
    manifest = ROOT / "MANIFEST.sha256"
    if not manifest.exists(): error("MANIFEST.sha256 is missing"); return
    listed: set[str] = set()
    for line in manifest.read_text(encoding="utf-8").splitlines():
        if not line.strip(): continue
        try: expected, relative = line.split("  ", 1)
        except ValueError: error(f"invalid manifest line: {line!r}"); continue
        listed.add(relative); path = ROOT / relative
        if not path.is_file(): error(f"manifest path missing: {relative}"); continue
        if hashlib.sha256(path.read_bytes()).hexdigest() != expected: error(f"manifest digest mismatch: {relative}")
    expected_files = {
        str(path.relative_to(ROOT)) for path in ROOT.rglob("*")
        if path.is_file() and path.name != "MANIFEST.sha256" and "target" not in path.parts
    }
    if listed != expected_files:
        error(f"manifest coverage differs: missing={sorted(expected_files-listed)}, stale={sorted(listed-expected_files)}")


def main() -> int:
    check_workspace(); check_rust(); check_bootstrap(); check_packages()
    if ERRORS:
        for message in ERRORS: print(f"ERROR: {message}")
        print(f"static verification failed with {len(ERRORS)} error(s)")
        return 1
    rust_files = len([*ROOT.glob("crates/muse-*/**/*.rs"), *ROOT.glob("crates/muse/**/*.rs")])
    print(f"static verification passed: {rust_files} Rust files, 9 sealed packages, {EXPECTED_COUNTS['concepts']} concepts, {EXPECTED_COUNTS['relations']} relations, {EXPECTED_COUNTS['axioms']} axioms")
    return 0


if __name__ == "__main__":
    sys.exit(main())
