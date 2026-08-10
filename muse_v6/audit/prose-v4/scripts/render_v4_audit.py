#!/usr/bin/env python3
import json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
rows=[json.loads(x) for x in (ROOT/'v4_strict_labels.jsonl').open()]
def anch(x,key='lexical_anchor'):
 return '+'.join(s['text'] for s in x.get(key,[]))
def val(v):
 if isinstance(v,dict):return expr(v)
 return str(v)
def expr(x):
 t=x.get('type')
 if t=='occurrence':
  rs=', '.join(f"{r['role']}={val(r['value'])}" for r in x.get('roles',[]))
  g=''
  if x.get('tense_aspect'):g+=' '+str(x['tense_aspect'])
  if x.get('voice'):g+=' '+x['voice']
  return f"OCC[{x.get('ontology_type')}:{anch(x)}]({rs}){g}"
 if t=='negation':return f"NOT({expr(x['content'])})"
 if t=='modal':return f"MOD[{x['modality']}]({expr(x['content'])})"
 if t=='capability':return f"CAP({x['bearer']}, {expr(x['content'])})"
 if t=='attitude':return f"ATT[{x['attitude']}]({x['holder']}, {expr(x['content'])})"
 if t=='phase':return f"PHASE[{x['phase']}]({expr(x['content'])})"
 if t=='interrogative':return f"Q[{x['interrogative']}{','+x['variable']+':'+str(x.get('domain')) if x.get('variable') else ''}]({expr(x['body'])})"
 if t=='speech_act':return f"SAY[{x['act']}]({x['speaker']}, {expr(x['content'])})"
 if t=='implication':return f"IF({expr(x['left'])} -> {expr(x['right'])})"
 if t=='causal':return f"CAUSE[{x['relation']}]({val(x['left'])}, {val(x['right'])})"
 if t=='temporal':return f"TEMP[{x['relation']}]({expr(x['subject'])}, {val(x['object'])})"
 if t in {'conjunction','disjunction'}:
  op=' AND ' if t=='conjunction' else ' OR '
  return '('+op.join(expr(y) for y in x['members'])+')'
 if t=='quotation':return f"QUOTE({expr(x['content'])})"
 if t=='quantified':return f"{x['quantifier']} {x['variable']}:{x.get('domain')}({expr(x['body'])})"
 if t=='equality':return f"EQ({val(x['left_term'])},{val(x['right_term'])})"
 return str(x)
out=[]
for r in rows:
 out.append(f"## Sample {r['sample_index']} — {r['provider']} / {r['speaker_role']}\n\nSOURCE: {r['text']}\n")
 for i,s in enumerate(r['statements'],1):out.append(f"{i}. {s['mode']}: {expr(s['expression'])}")
 if r.get('ambiguities'):out.append('AMBIGUITIES: '+json.dumps(r['ambiguities'],ensure_ascii=False))
 out.append('')
Path('/tmp/muse-v4-audit').mkdir(parents=True,exist_ok=True)
Path('/tmp/muse-v4-audit/V4_STRATIFIED_LABEL_AUDIT.md').write_text('\n\n'.join(out))
print('/tmp/muse-v4-audit/V4_STRATIFIED_LABEL_AUDIT.md')
