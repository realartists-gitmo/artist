#!/usr/bin/env python3
"""Toolchain-independent structural audit.

This catches packaging corruption and obvious source omissions. It cannot replace
Rust compilation, Clippy, or tests.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def fail(message: str) -> None:
    print(f"STATIC VERIFY FAILED: {message}", file=sys.stderr)
    raise SystemExit(1)


def scan_balanced(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    stack: list[tuple[str, int, int]] = []
    pairs = {")": "(", "]": "[", "}": "{"}
    state = "code"
    block_depth = 0
    raw_hashes = 0
    line = 1
    column = 0
    index = 0
    while index < len(text):
        char = text[index]
        next_char = text[index + 1] if index + 1 < len(text) else ""
        column += 1
        if state == "code":
            if char == "/" and next_char == "/":
                state = "line_comment"
                index += 1
                column += 1
            elif char == "/" and next_char == "*":
                state = "block_comment"
                block_depth = 1
                index += 1
                column += 1
            elif char == '"':
                state = "string"
            elif char == "r":
                cursor = index + 1
                hashes = 0
                while cursor < len(text) and text[cursor] == "#":
                    hashes += 1
                    cursor += 1
                if cursor < len(text) and text[cursor] == '"':
                    state = "raw_string"
                    raw_hashes = hashes
                    column += cursor - index
                    index = cursor
            elif char == "'":
                cursor = index + 1
                if cursor < len(text) and text[cursor] == "\\":
                    cursor += 2
                else:
                    cursor += 1
                if cursor < len(text) and text[cursor] == "'":
                    state = "char"
            elif char in pairs.values():
                stack.append((char, line, column))
            elif char in pairs:
                if not stack or stack[-1][0] != pairs[char]:
                    fail(f"{path.relative_to(ROOT)}:{line}:{column}: unmatched {char}")
                stack.pop()
        elif state == "line_comment":
            if char == "\n":
                state = "code"
        elif state == "block_comment":
            if char == "/" and next_char == "*":
                block_depth += 1
                index += 1
                column += 1
            elif char == "*" and next_char == "/":
                block_depth -= 1
                index += 1
                column += 1
                if block_depth == 0:
                    state = "code"
        elif state == "string":
            if char == "\\":
                index += 1
                column += 1
            elif char == '"':
                state = "code"
        elif state == "char":
            if char == "\\":
                index += 1
                column += 1
            elif char == "'":
                state = "code"
        elif state == "raw_string":
            if char == '"' and text[index + 1 : index + 1 + raw_hashes] == "#" * raw_hashes:
                index += raw_hashes
                column += raw_hashes
                state = "code"
        if char == "\n":
            line += 1
            column = 0
        index += 1
    if stack:
        char, open_line, open_column = stack[-1]
        fail(f"{path.relative_to(ROOT)}:{open_line}:{open_column}: unclosed {char}")
    if state in {"block_comment", "string", "raw_string", "char"}:
        fail(f"{path.relative_to(ROOT)}: unterminated {state}")


def require_all(path: Path, needles: list[str]) -> None:
    text = path.read_text(encoding="utf-8")
    missing = [needle for needle in needles if needle not in text]
    if missing:
        fail(f"{path.relative_to(ROOT)} missing required forms: {missing}")


def main() -> None:
    if (ROOT / "lean").exists():
        fail("permanent Lean workspace is bundled")

    cargo_files = [ROOT / "Cargo.toml", *sorted((ROOT / "crates").glob("artist-*/Cargo.toml"))]
    for path in cargo_files:
        with path.open("rb") as source:
            tomllib.load(source)

    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    members = set(workspace["workspace"]["members"])
    manifests = {str(path.parent.relative_to(ROOT)) for path in cargo_files[1:]}
    if not manifests.issubset(members):
        fail(f"cognitive crate manifests missing from workspace: {manifests - members}")

    lock_text = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    if "ggithub.com" in lock_text:
        fail("Cargo.lock contains a malformed registry URL")

    rust_files = sorted(path for crate in (ROOT / "crates").glob("artist-*") for path in crate.rglob("*.rs"))
    if not rust_files:
        fail("no Rust source files")
    for path in rust_files:
        scan_balanced(path)
        text = path.read_text(encoding="utf-8")
        if re.search(r"\b(todo!|unimplemented!)\s*\(", text):
            fail(f"placeholder macro in {path.relative_to(ROOT)}")
        if re.search(r"\bunsafe\s*(fn|impl|trait|\{)", text):
            fail(f"unsafe Rust in {path.relative_to(ROOT)}")

    requirements = {
        "crates/artist-formal/src/ontology.rs": [
            "pub struct InterpretedGraph",
            "pub struct InterpretedGraphHash",
            "pub struct InterpretedGraphRef",
            "pub fn canonical_hash(&self) -> InterpretedGraphHash",
            "pub fn typing_diagnostics",
            "InvalidPropositionUniverse",
            "QuotedObject",
            "QuotedTypeMismatch",
            'put_str(&mut bytes, "artist.interpreted-graph/3")',
        ],
        "crates/artist-formal/src/query.rs": [
            "pub submission: InterpretedGraph",
            "Interpretation(#[from] InterpretationError)",
        ],
        "crates/artist-kernel/src/ontology.rs": [
            "pub struct CompiledOntologySubmission",
            "pub fn verify_compiled",
            "pub fn elaborate",
            "ElaborationRole::QuotedObject",
            "submission: InterpretedGraphHash",
            'const COMPILER_VERSION: &str = "artist.ontology-dtt/3"',
            "quoted_object_type: Name",
        ],
        "crates/artist-kernel/src/term.rs": [
            "Universe {",
            "Pi {",
            "Sigma {",
            "Id {",
            "J {",
            "Elim {",
            "pub fn subst_at",
        ],
        "crates/artist-kernel/src/theory.rs": [
            "pub enum ConstructorField",
            "RecursiveFunction",
            "pub fn is_extension_of",
            "pub fn validate(&self)",
        ],
        "crates/artist-theories/src/universal.rs": [
            "pub enum UniversalInstruction",
            "pub fn check_run",
            "pub struct UniversalVerifier",
            "pub struct ComputationCertificate",
        ],
        "crates/artist-theories/src/package.rs": [
            "pub struct PackageExtensions",
            "pub fn validate_imports",
            "check_package_certificate",
            "TranslationSourceMismatch",
        ],
        "crates/artist-cognition/src/certification.rs": [
            "pub struct CertifiedGraphClaim",
            "OntologyCompiler::verify_record",
            "proof_theory.is_extension_of",
            "DependencyMismatch",
        ],
        "crates/artist-cognition/src/query.rs": [
            "pub enum QueryRequest",
            "pub enum QueryStatus",
            "pub enum Continuation",
            "pub fn validate_answer",
            "pub struct CertifiedQueryWitness",
            "pub struct QueryHash",
            "pub query_content: QueryHash",
            "pub theory: TheoryId",
            "continuation.format_version == 3",
            "KernelQuery",
            "WitnessCertificateMismatch",
            "DuplicateWitness",
            "submission: InterpretedGraphHash",
        ],
        "crates/artist-cognition/src/artifact.rs": [
            "pub trait ArtifactContract",
            "pub fn encode_typed",
            "pub fn decode_typed",
            "artist.artifact/1",
            '"artist.formal.interpreted-graph", 3',
            '"artist.cognition.query", 4',
            '"artist.cognition.answer", 4',
            '"artist.empirical.observation", 2',
            '"artist.empirical.inference-artifact", 2',
            '"artist.empirical.posterior", 2',
            '"artist.cognition.discovery-continuation",\n    3',
            '"artist.cognition.kernel-continuation",',
        ],
        "crates/artist-empirical/src/result.rs": [
            "pub struct InferenceCertification",
            "pub certification: Option<InferenceCertification>",
            "OntologyCompiler::verify_compiled",
            "OntologyCompiler::verify_record",
            "ClaimIdentityMismatch",
            "CertificateMismatch",
            "DependencyMismatch",
        ],
        "crates/artist-empirical/src/bayes.rs": [
            "use artist_formal::InterpretedGraphRef",
            "pub enum InferenceMethod",
            "pub struct ModelComparison",
            "pub trait BayesianInferenceEngine",
            "pub trait ModelComparisonEngine",
        ],
        "crates/artist-empirical/src/observation.rs": [
            "pub struct ClockRelation",
            "SourceClockMismatch",
            "pub fn validate_observation_ledger",
        ],
        "crates/artist-cog/src/main.rs": [
            'Some("interpreted-validate")',
            'Some("ontology-compile")',
            'Some("ontology-elaborate")',
            'Some("universal-check")',
            'Some("package-validate")',
            'Some("observation-validate")',
            'Some("posterior-validate")',
            'Some("inference-verify")',
            'Some("artifact-verify")',
        ],
    }
    for relative, needles in requirements.items():
        require_all(ROOT / relative, needles)

    required_docs = [
        "ARCHITECTURE.md",
        "ASSURANCE.md",
        "CONFORMANCE.md",
        "EMPIRICAL.md",
        "FEATURE_COMPLETENESS.md",
        "GOALS.md",
        "INTEGRATION.md",
        "KERNEL.md",
        "ONTOLOGY.md",
        "QUERY.md",
        "SERIALIZATION.md",
        "TCB.md",
        "VALIDATION.md",
    ]
    for name in required_docs:
        if not (ROOT / "docs" / "cognitive-superstrate" / name).is_file():
            fail(f"missing docs/cognitive-superstrate/{name}")

    print(f"static verification passed: {len(rust_files)} Rust files")
    print("note: this structural command does not execute compilation, tests, or Clippy")


if __name__ == "__main__":
    main()
