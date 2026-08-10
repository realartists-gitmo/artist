#!/usr/bin/env python3
"""Non-mutating gate for the frozen 36-window prose-v4 semantic regression."""
from __future__ import annotations
import subprocess, sys, tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
SCRIPTS=ROOT/'scripts'
with tempfile.TemporaryDirectory(prefix='muse-prose-v4-audit-') as td:
    out=Path(td)
    subprocess.run([sys.executable,str(SCRIPTS/'build_strict_audit.py'),'--output-dir',str(out)],check=True)
    expected=(ROOT/'v4_strict_labels.jsonl').read_bytes()
    actual=(out/'v4_strict_labels.jsonl').read_bytes()
    if actual!=expected:
        print('prose-v4 regression verification FAILED: deterministic rebuild differs from committed strict labels')
        sys.exit(1)
for script in ['verify_builder_scope.py','verify_v4_audit.py','verify_source_coverage.py']:
    subprocess.run([sys.executable,str(SCRIPTS/script)],check=True)
print('prose-v4 fixed regression verification passed: deterministic rebuild + scope + structure + coverage')
