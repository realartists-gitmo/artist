#!/usr/bin/env python3
from pathlib import Path
import hashlib

root = Path(__file__).resolve().parents[1]
paths = sorted(
    path for path in root.rglob('*')
    if path.is_file()
    and path.name != 'MANIFEST.sha256'
    and 'target' not in path.parts
    and '__pycache__' not in path.parts
    and path.suffix != '.pyc'
)
lines = [f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.relative_to(root).as_posix()}" for path in paths]
(root / 'MANIFEST.sha256').write_text('\n'.join(lines) + '\n', encoding='utf-8')
print(f"wrote {len(lines)} manifest entries")
