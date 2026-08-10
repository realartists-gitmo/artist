#!/usr/bin/env python3
"""Guard against silently dropping large source clauses from the fixed semantic audit.

This is deliberately a backstop, not a semantic proof: every alphanumeric source byte
must be covered at a high rate by at least one labeled semantic object's exact source span.
"""
import json, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
P=ROOT/'v4_strict_labels.jsonl'
THRESHOLD=0.82
rows=[json.loads(x) for x in P.open()]

def spans(x):
    out=[]
    if isinstance(x,dict):
        for k,v in x.items():
            if k in {'source_spans','operator_spans','operator_grammatical_spans','lexical_anchor','grammatical_spans'} and isinstance(v,list):
                out.extend((s['start'],s['end']) for s in v if isinstance(s,dict) and 'start' in s and 'end' in s)
            elif k!='ambiguities': out.extend(spans(v))
    elif isinstance(x,list):
        for v in x: out.extend(spans(v))
    return out

fail=[]; ratios=[]
for row in rows:
    raw=row['text'].encode('utf-8'); mask=bytearray(len(raw))
    for a,b in spans(row.get('statements',[])):
        a=max(0,min(len(raw),a)); b=max(a,min(len(raw),b))
        mask[a:b]=b'\x01'*(b-a)
    meaningful=[i for i,c in enumerate(raw) if c>=128 or chr(c).isalnum()]
    ratio=(sum(bool(mask[i]) for i in meaningful)/len(meaningful)) if meaningful else 1.0
    ratios.append((row['sample_index'],ratio))
    if ratio<THRESHOLD: fail.append((row['sample_index'],ratio))
if fail:
    print('source coverage verification FAILED')
    for i,r in fail: print(f'ERROR: sample {i} semantic-span coverage {r:.1%} < {THRESHOLD:.0%}')
    sys.exit(1)
mi=min(ratios,key=lambda x:x[1]); avg=sum(x[1] for x in ratios)/len(ratios)
print(f'source coverage verification passed: 36 windows, min={mi[1]:.1%} (sample {mi[0]}), mean={avg:.1%}')
