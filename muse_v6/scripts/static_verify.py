#!/usr/bin/env python3
from __future__ import annotations
import hashlib
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXPECTED = {
    "crates/muse-core", "crates/muse-provenance", "crates/muse-ontology", "crates/muse-lexicon",
    "crates/muse-registry", "crates/muse-classification", "crates/muse-reasoning", "crates/muse-resolution",
    "crates/muse-interpretation", "crates/muse-occurrence", "crates/muse-tooling", "crates/muse-artist-adapter", "crates/muse-superstrate", "crates/muse-training", "crates/muse-validation", "crates/muse-io", "crates/muse", "crates/muse-cli",
    "crates/artist-formal", "crates/artist-kernel", "crates/artist-empirical", "crates/artist-theories",
    "crates/artist-cognition", "crates/artist-cog",
}

def fail(msg: str) -> None:
    print(f"COMBINED STATIC VERIFY FAILED: {msg}", file=sys.stderr)
    raise SystemExit(1)

def verify_manifest() -> None:
    manifest=ROOT/"MANIFEST.sha256"
    if not manifest.is_file(): fail("MANIFEST.sha256 missing")
    listed=set()
    for line in manifest.read_text().splitlines():
        if not line.strip(): continue
        digest, rel=line.split("  ",1)
        listed.add(rel)
        p=ROOT/rel
        if not p.is_file(): fail(f"manifest path missing: {rel}")
        if hashlib.sha256(p.read_bytes()).hexdigest()!=digest: fail(f"manifest digest mismatch: {rel}")
    actual={p.relative_to(ROOT).as_posix() for p in ROOT.rglob("*") if p.is_file() and p.name!="MANIFEST.sha256" and "target" not in p.parts and "__pycache__" not in p.parts and p.suffix!=".pyc"}
    if actual!=listed: fail(f"manifest coverage mismatch: missing={sorted(actual-listed)}, stale={sorted(listed-actual)}")

def main() -> None:
    data=tomllib.loads((ROOT/"Cargo.toml").read_text())
    members=set(data["workspace"]["members"])
    if members!=EXPECTED: fail(f"workspace membership mismatch: {members ^ EXPECTED}")
    for member in sorted(EXPECTED):
        if not (ROOT/member/"Cargo.toml").is_file(): fail(f"missing member manifest: {member}")
    for name in ["semantic_static_verify.py","cognitive_static_verify.py"]:
        subprocess.run([sys.executable,str(ROOT/"scripts"/name)],check=True)
    verify_manifest()
    print(f"combined static verification passed: {len(EXPECTED)} workspace crates")

if __name__ == "__main__": main()
