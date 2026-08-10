#!/usr/bin/env python3
"""Verify deterministic sealing and import closure for Muse foundation packages."""
from __future__ import annotations

from pathlib import Path
from copy import deepcopy
import hashlib
import json
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FOUNDATION = ROOT / "packages" / "foundation"
ZERO = {"algorithm": "sha256", "value": "0" * 64}
EXPECTED = {
    "muse.foundation.provenance",
    "muse.software",
    "muse.computing",
    "muse.semantic",
    "muse.agent",
    "muse.coding_harness",
    "muse.artist",
}


def compact(v): return json.dumps(v, ensure_ascii=False, separators=(",", ":")).encode()
def sha(v): return hashlib.sha256(compact(v)).hexdigest()

def fail(msg):
    print(f"FOUNDATION PACKAGE VERIFY FAILED: {msg}", file=sys.stderr)
    raise SystemExit(1)


def read_packages(directory: Path):
    result = {}
    for path in sorted(directory.glob("*.muse.json")):
        data = json.loads(path.read_text())
        if data.get("format_version") != "muse-json-1": fail(f"bad format: {path}")
        payload = data["payload"]
        if sha(payload) != data["document_digest"]["value"]: fail(f"document digest: {path}")
        document = payload["document"]
        normalized = deepcopy(document)
        actual = normalized["package"]["header"]["package"]["digest"]["value"]
        normalized["package"]["header"]["package"]["digest"] = deepcopy(ZERO)
        if sha(normalized) != actual: fail(f"package digest: {path}")
        ref = document["package"]["header"]["package"]
        result[ref["id"]] = (path, document)
    return result


def main():
    packages = read_packages(FOUNDATION)
    if set(packages) != EXPECTED: fail(f"ids mismatch: {set(packages) ^ EXPECTED}")

    available = dict(packages)
    for path in sorted((ROOT / "packages" / "ufo").glob("*.muse.json")):
        data = json.loads(path.read_text())
        doc = data["payload"]["document"]
        ref = doc["package"]["header"]["package"]
        available[ref["id"]] = (path, doc)
    for pid, (path, doc) in packages.items():
        pkg = doc["package"]
        if doc["kind"] == "ontology":
            if not pkg["concepts"]: fail(f"ontology has no concepts: {pid}")
            for cid, c in pkg["concepts"].items():
                if cid != c["id"]: fail(f"concept identity mismatch {cid}")
                if not c["preferred_labels"] or not c["definition"]["text"]: fail(f"incomplete concept {cid}")
            for rid, r in pkg["relations"].items():
                if rid != r["id"]: fail(f"relation identity mismatch {rid}")
                if not r["domains"] or not r["ranges"]: fail(f"untyped relation {rid}")
        for imp in pkg["header"]["imports"]:
            target = available.get(imp["id"])
            if target is None: fail(f"unknown import {imp['id']} from {pid}")
            target_ref = target[1]["package"]["header"]["package"]
            if imp.get("version") != target_ref["version"] or imp.get("digest") != target_ref["digest"]:
                fail(f"non-exact/stale import {imp['id']} from {pid}")

    with tempfile.TemporaryDirectory() as td:
        subprocess.run([sys.executable, str(ROOT / "scripts" / "generate_foundation_packages.py"), "--out", td], check=True)
        generated = Path(td)
        for path in sorted(FOUNDATION.glob("*.muse.json")):
            other = generated / path.name
            if not other.is_file() or path.read_bytes() != other.read_bytes():
                fail(f"non-deterministic/stale generated package {path.name}")

    concept_count = sum(len(d["package"].get("concepts", {})) for _, d in packages.values())
    relation_count = sum(len(d["package"].get("relations", {})) for _, d in packages.values())
    print(f"foundation package verification passed: {len(packages)} packages, {concept_count} concepts, {relation_count} relations")


if __name__ == "__main__": main()
