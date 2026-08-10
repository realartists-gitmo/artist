#!/usr/bin/env python3
from __future__ import annotations
import argparse, hashlib, json, re, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
AUDIT=ROOT/'audit/semantic-v5'
errors=[]
def fail(x): errors.append(x)
def need(path,marker):
    text=path.read_text()
    if marker not in text: fail(f'{path.relative_to(ROOT)} missing {marker}')

def check_manifest(name,expected_classes):
    path=AUDIT/name
    data=json.loads(path.read_text())
    cases=data.get('cases',[])
    ids=[c.get('id') for c in cases]
    if len(ids)!=len(set(ids)): fail(f'{name}: duplicate case id')
    classes={c.get('class') for c in cases}
    missing=set(expected_classes)-classes
    if missing: fail(f'{name}: missing classes {sorted(missing)}')
    for case in cases:
        if not case.get('source'): fail(f'{name}/{case.get("id")}: missing source')
        if 'line' not in case and 'lines' not in case: fail(f'{name}/{case.get("id")}: missing line(s)')
        if 'sha256' in case and not re.fullmatch(r'[0-9a-f]{64}',case['sha256']): fail(f'{name}/{case.get("id")}: invalid sha256')
        for record in case.get('records',[]):
            if not re.fullmatch(r'[0-9a-f]{64}',record.get('sha256','')): fail(f'{name}/{case.get("id")}: invalid record sha256')
    return data

first=check_manifest('draw.json',{
    'question_options','task_state','delegation_prompt','lifecycle_duplicate','json_encoded_string',
    'program_contains_tool_syntax','scalar_control','message_aliases','reported_claims','opaque_multimodal','huge_visibility_boundary'
})
second=check_manifest('draw2.json',{
    'requested_edit_not_effect','delegation_prompt_scope','requested_task_state','network_prompt_scope',
    'failure_with_partial_observations','requested_delivery_not_success','provider_status_not_effect_status',
    'json_encoded_result_transport','stream_snapshot_visibility'
})

need(ROOT/'crates/muse-training/src/lib.rs','ToolCallUpdate')
need(ROOT/'crates/muse-training/src/lib.rs','StructuredFieldValue::Opaque')
need(ROOT/'crates/muse-training/src/lib.rs','alias_paths')
need(ROOT/'crates/muse-training/src/lib.rs','container_paths')
need(ROOT/'crates/muse-training/src/lib.rs','Tool,')
need(ROOT/'crates/muse-training/src/lib.rs','concept_set_has_subtype')
need(ROOT/'crates/muse-training/src/lib.rs','relation_is_subrelation')
need(ROOT/'crates/muse-superstrate/src/lib.rs','ConceptId::from("ufo:Proposition")')
need(ROOT/'scripts/generate_foundation_packages.py','agent:QuestionPrompt')
need(ROOT/'scripts/generate_foundation_packages.py','agent:ChoiceOption')
need(ROOT/'scripts/generate_foundation_packages.py','agent:ToolInvocationUpdate')
need(ROOT/'scripts/generate_foundation_packages.py','agent:MessageDelivery')
need(ROOT/'scripts/generate_foundation_packages.py','agent:toolResultOutcome')
for marker in ['model-visible','cumulative UI snapshots','toolResultOutcome','status` is provider/tool-schema dependent','ontology-ancestry checks']:
    need(ROOT/'docs/SEMANTIC_LABEL_SPEC.md',marker)

ap=argparse.ArgumentParser()
ap.add_argument('--source-root',type=Path)
args=ap.parse_args()
if args.source_root:
    for manifest in (first,second):
        for case in manifest['cases']:
            src=args.source_root/case['source']
            if not src.is_file(): fail(f'missing external draw source {src}'); continue
            wanted=[]
            if 'line' in case: wanted=[(case['line'],case.get('sha256'))]
            else: wanted=[(r['line'],r['sha256']) for r in case.get('records',[])]
            targets={n:h for n,h in wanted}
            found={}
            with src.open('rb') as fh:
                for n,line in enumerate(fh,1):
                    if n in targets: found[n]=hashlib.sha256(line).hexdigest()
                    if len(found)==len(targets): break
            for n,h in targets.items():
                if found.get(n)!=h: fail(f'external draw digest mismatch {case["id"]}:{n}')
if errors:
    print('semantic-v5 adversarial audit FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print(f'semantic-v5 adversarial audit passed: {len(first["cases"])} first-draw + {len(second["cases"])} second-draw cases')
