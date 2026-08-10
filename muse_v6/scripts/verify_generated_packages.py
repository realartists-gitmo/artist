#!/usr/bin/env python3
"""Verify sealed UFO packages without modifying the source tree."""
from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PACKAGE_DIR = ROOT / "packages" / "ufo"
GENERATOR = ROOT / "scripts" / "generate_ufo_packages.py"


def hashes(directory: Path) -> dict[str, str]:
    return {
        path.name: hashlib.sha256(path.read_bytes()).hexdigest()
        for path in sorted(directory.glob("*.muse.json"))
    }


def generate(directory: Path) -> dict[str, str]:
    subprocess.run(
        [sys.executable, str(GENERATOR), "--output-dir", str(directory)],
        check=True,
    )
    return hashes(directory)


committed = hashes(PACKAGE_DIR)
with tempfile.TemporaryDirectory(prefix="muse-ufo-verify-") as tmp:
    output = Path(tmp)
    generated = generate(output)
    if committed != generated:
        print("generated package files are stale or manually modified", file=sys.stderr)
        for name in sorted(set(committed) | set(generated)):
            if committed.get(name) != generated.get(name):
                print(
                    f"  {name}: committed={committed.get(name)} generated={generated.get(name)}",
                    file=sys.stderr,
                )
        raise SystemExit(1)

    second = generate(output)
    if generated != second:
        print("UFO package generator is not deterministic", file=sys.stderr)
        raise SystemExit(1)

print(f"generated package verification passed: {len(committed)} files")
