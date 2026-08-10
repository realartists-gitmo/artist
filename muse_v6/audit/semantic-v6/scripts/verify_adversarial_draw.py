#!/usr/bin/env python3
from __future__ import annotations
import argparse, hashlib, json, re, sys, zipfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
AUDIT=ROOT/'audit/semantic-v6'
errors=[]
def fail(x): errors.append(x)
def need(path:Path, marker:str):
    text=path.read_text()
    if marker not in text: fail(f'{path.relative_to(ROOT)} missing {marker}')
def forbid(path:Path, marker:str):
    text=path.read_text()
    if marker in text: fail(f'{path.relative_to(ROOT)} contains forbidden {marker}')

data=json.loads((AUDIT/'cases.json').read_text())
if data.get('schema')!='muse-semantic-v6-adversarial-1': fail('wrong audit schema')
cases=data.get('cases',[])
if len(cases)<12: fail('audit requires at least 12 independent adversarial cases')
ids=[c.get('id') for c in cases]
if len(ids)!=len(set(ids)): fail('duplicate adversarial case id')
for c in cases:
    if not c.get('source') or not isinstance(c.get('line'),int) or c['line']<1: fail(f'{c.get("id")}: invalid source locator')
    if not re.fullmatch(r'[0-9a-f]{64}',c.get('sha256','')): fail(f'{c.get("id")}: invalid sha256')
    if not c.get('needle') or not c.get('invariants'): fail(f'{c.get("id")}: missing needle/invariants')

learn=ROOT/'crates/muse-training/src/semantic_v6.rs'
compile=ROOT/'crates/muse-training/src/v6_compile.rs'
training=ROOT/'crates/muse-training/src/lib.rs'
foundation=ROOT/'scripts/generate_foundation_packages.py'
tooling=ROOT/'crates/muse-tooling/src/lib.rs'
for m in ['ScopedOperator','PluralScaleCardinality','Cardinality','Focus','Question','LearnedAmbiguity','LearnedArithmeticOperator']:
    need(learn,m)
for m in ['PropositionalOperator','EpistemicQualificationOperator','CapabilityOperator','GenericOperator','MaximizationOperator','optimizationTarget','ExclusiveFocusOperator','QuestionPrompt','ChoiceOption','toolInvocationRequestedEffect']:
    need(foundation,m)
for m in ['validate_source_coverage','alternatives do not cover the same source footprint','arguments.len() != 2','toolResultOfInvocation','toolInvocationUpdateFor']:
    need(compile,m)
for m in ['normalization_contracts: BTreeMap','OpaqueEncoding::Base64','duplicate JSON object key','source_adapter_version','ModelVisible','exact_json_numbers_do_not_round_through_f64']:
    need(training,m)
for m in ['toolInvocationRequestedEffect','toolInvocationObservedEffect','toolResultOutcome']:
    need(tooling,m)
forbid(learn,'SourceAnchored')
forbid(learn,'PresentationMode')
forbid(learn,'ParticipantRole')

ap=argparse.ArgumentParser()
ap.add_argument('--source-archive',type=Path)
args=ap.parse_args()
archive=args.source_archive
if archive is None:
    candidate=Path('/mnt/data/agentic-sessions-big-37(2).zip')
    if candidate.is_file(): archive=candidate
if archive is not None:
    if not archive.is_file(): fail(f'missing source archive {archive}')
    else:
        with zipfile.ZipFile(archive) as z:
            names=set(z.namelist())
            for c in cases:
                member='kept/'+c['source']
                if member not in names: fail(f'{c["id"]}: missing archive member {member}'); continue
                lines=z.read(member).splitlines(keepends=True)
                if c['line']>len(lines): fail(f'{c["id"]}: line out of range'); continue
                raw=lines[c['line']-1]
                if len(raw)!=c['raw_line_bytes']: fail(f'{c["id"]}: byte-length mismatch')
                if hashlib.sha256(raw).hexdigest()!=c['sha256']: fail(f'{c["id"]}: source digest mismatch')
                if c['needle'] not in raw.decode('utf-8','replace'): fail(f'{c["id"]}: source needle missing')
if errors:
    print('semantic-v6 adversarial audit FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print(f'semantic-v6 adversarial audit passed: {len(cases)} pinned cases'+(' + source archive' if archive else ''))
