#!/usr/bin/env python3
"""Toolchain-independent verification of Muse semantic-v6 pre-label readiness."""
from __future__ import annotations
import json,re,subprocess,sys,tomllib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'scripts'))
from ontology_symbols import build_symbol_maps,CONCEPT_RENAMES,RELATION_RENAMES
errors=[]
def fail(x): errors.append(x)

def load_active_ontology():
    concepts=set();relations=set();owners={}
    paths=sorted((ROOT/'packages/foundation').glob('*.muse.json'))+sorted((ROOT/'packages/ufo').glob('*.muse.json'))
    for path in paths:
        pkg=json.loads(path.read_text())['payload']['document']['package']
        for q in pkg.get('concepts',{}):
            if ('c',q) in owners: fail(f'duplicate qualified concept {q}')
            owners[('c',q)]=path; concepts.add(q)
        for q in pkg.get('relations',{}):
            if ('r',q) in owners: fail(f'duplicate qualified relation {q}')
            owners[('r',q)]=path; relations.add(q)
    return concepts,relations

cargo=tomllib.loads((ROOT/'Cargo.toml').read_text())
members=cargo['workspace']['members']; defaults=cargo['workspace']['default-members']
if set(members)!=set(defaults): fail('workspace members/default-members differ')
lock=tomllib.loads((ROOT/'Cargo.lock').read_text())
local={p['name']:p for p in lock['package'] if 'source' not in p}
member_names={tomllib.loads((ROOT/m/'Cargo.toml').read_text())['package']['name'] for m in members}
if set(local)!=member_names: fail(f'local lock/workspace mismatch: missing={sorted(member_names-set(local))} extra={sorted(set(local)-member_names)}')

concepts,relations=load_active_ontology()
try: cs,rs=build_symbol_maps(concepts,relations)
except ValueError as e: fail(str(e)); cs={};rs={}
if len(cs)!=len(concepts) or len(rs)!=len(relations): fail('namespaceless ontology registry is not one-to-one')
if set(cs)&set(rs): fail(f'concept/relation canonical symbol collision: {sorted(set(cs)&set(rs))}')
if any(':' in x for x in set(cs)|set(rs)): fail('learned ontology registry contains namespace-qualified symbol')
expected_c={'ufo:Delegation','agent:Delegation','mlt:Individual','ufo:Individual','mlt:Type','ufo:Type'}
expected_r={'mlt:instantiates','ufo:instantiates','mlt:specializes','ufo:specializes','service:fulfillsCommitment','ufo:fulfillsCommitment','service:violatesCommitment','ufo:violatesCommitment'}
if set(CONCEPT_RENAMES)!=expected_c: fail('concept collision rename table drift')
if set(RELATION_RENAMES)!=expected_r: fail('relation collision rename table drift')

for required in [
    'docs/SEMANTIC_LABEL_SPEC.md','docs/PRELABEL_CONTRACT.md',
    'audit/semantic-v6/ADVERSARIAL_DRAW.md','audit/semantic-v6/cases.json',
    'audit/semantic-v6/scripts/verify_adversarial_draw.py',
]:
    if not (ROOT/required).is_file(): fail(f'missing {required}')

# No unfinished work markers in Rust source entering the training contract.
for path in ROOT.rglob('*.rs'):
    if re.search(r'\b(?:TODO|FIXME|HACK|XXX)\b',path.read_text()): fail(f'work marker remains in {path.relative_to(ROOT)}')

checks=[
    [sys.executable,str(ROOT/'scripts/verify_foundation_packages.py')],
    [sys.executable,str(ROOT/'scripts/verify_generated_packages.py')],
    [sys.executable,str(ROOT/'scripts/verify_semantic_v6_source.py')],
    [sys.executable,str(ROOT/'scripts/semantic_static_verify.py')],
    [sys.executable,str(ROOT/'scripts/static_verify.py')],
    [sys.executable,str(ROOT/'audit/prose-v4/scripts/verify_regression.py')],
]
adv=[sys.executable,str(ROOT/'audit/semantic-v6/scripts/verify_adversarial_draw.py')]
archive=Path('/mnt/data/agentic-sessions-big-37(2).zip')
if archive.is_file(): adv += ['--source-archive',str(archive)]
checks.append(adv)
for cmd in checks:
    result=subprocess.run(cmd,cwd=ROOT)
    if result.returncode: fail(f'failed verifier {Path(cmd[1]).relative_to(ROOT)}')

if errors:
    print('semantic-v6 pre-label verification FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print(f'semantic-v6 pre-label verification passed: {len(members)} crates, {len(concepts)} concepts, {len(relations)} relations, {len(cs)}+{len(rs)} collision-free learned symbols')
