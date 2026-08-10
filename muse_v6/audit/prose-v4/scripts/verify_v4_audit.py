#!/usr/bin/env python3
import json, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
WORKSPACE=ROOT.parents[1]
P=ROOT/'v4_strict_labels.jsonl'
rows=[json.loads(x) for x in P.open()]
VALID_ROLES={'Subject','DirectObject','IndirectObject','PredicateComplement','Agent','Patient','Theme','Experiencer','Content','Source','Goal','Recipient','Instrument','Location','Manner','Beneficiary','Stimulus','Topic','Possessor','Attribute','Value'}
VALID_MODES={'Assertion','Question','Command','Request','Suggestion','Promise','Quotation','Mention','SourceAnchored'}
STRUCT={'negation','modal','capability','attitude','phase','interrogative','speech_act','implication','counterfactual','causal','temporal','conjunction','disjunction','quotation','quantified'}
GRAM_OP={'modal','capability','phase','attitude','speech_act'}
NUMERIC_Q={'Exactly','AtLeast','AtMost','MoreThan','FewerThan'}
errors=[]
stats={'nodes':0,'occurrences':0,'typed_occurrences':0,'specialized_occurrences':0,'referents':0,'specialized_referents':0,'ellipses':0,'ambiguities':0,'variables':0,'typed_variables':0,'numeric_cardinality':0,'operator_grammar':0}

sys.path.insert(0,str(WORKSPACE/'scripts'))
from ontology_symbols import build_symbol_maps, concept_symbol, relation_symbol

def ontology_index():
    qualified_parents={}; concepts=set(); relations=set()
    for path in list((WORKSPACE/'packages/foundation').glob('*.muse.json'))+list((WORKSPACE/'packages/ufo').glob('*.muse.json')):
        try: package=json.loads(path.read_text())['payload']['document']['package']
        except Exception: continue
        concepts.update(package.get('concepts',{}))
        relations.update(package.get('relations',{}))
        for cid,declaration in package.get('concepts',{}).items():
            qualified_parents.setdefault(cid,set()).update(declaration.get('parents',[]))
    concept_map,relation_map=build_symbol_maps(concepts,relations)
    parents={concept_symbol(cid):{concept_symbol(parent) for parent in ps} for cid,ps in qualified_parents.items()}
    return parents,set(concept_map),set(relation_map)
PARENTS,CONCEPTS,RELATIONS=ontology_index()

def known(c): return c in CONCEPTS

def subtype(actual,required,seen=None):
    if actual==required:return True
    seen=set() if seen is None else seen
    if actual in seen:return False
    seen.add(actual)
    return any(subtype(p,required,seen) for p in PARENTS.get(actual,()))

def iv(s): return s['start'],s['end']
def covered(s, spans):
    a,b=iv(s); return any(x['start']<=a and b<=x['end'] for x in spans)
def exact(text,s):
    try:return text.encode()[s['start']:s['end']].decode('utf-8')==s['text']
    except Exception:return False

def collect_vars(v):
    out=set()
    if isinstance(v,str) and v.startswith('v_'):out.add(v)
    elif isinstance(v,dict):
        for k,x in v.items():
            if k in {'source_spans','operator_spans','operator_grammatical_spans','lexical_anchor','grammatical_spans'}:continue
            out|=collect_vars(x)
    elif isinstance(v,list):
        for x in v:out|=collect_vars(x)
    return out

def check_type(cid,path):
    if not isinstance(cid,str) or not cid:
        errors.append(f'{path}: missing ontology type');return
    if ':' in cid:errors.append(f'{path}: learned ontology concept must be namespaceless: {cid}');return
    if not known(cid):errors.append(f'{path}: unknown ontology concept {cid}')

def walk(node,text,path,bound=frozenset()):
    if not isinstance(node,dict):return set()
    stats['nodes']+=1;typ=node.get('type');used=set()
    for fld in ('source_spans','operator_spans','operator_grammatical_spans','lexical_anchor','grammatical_spans'):
        for s in node.get(fld,[]):
            if not exact(text,s):errors.append(f'{path}: invalid {fld} span {s}')
    src=node.get('source_spans',[])
    for fld in ('operator_spans','operator_grammatical_spans','lexical_anchor','grammatical_spans'):
        for s in node.get(fld,[]):
            if src and not covered(s,src):errors.append(f'{path}: {fld} span not inside semantic source')
    if typ=='occurrence':
        stats['occurrences']+=1
        if not node.get('lexical_anchor'):errors.append(f'{path}: open occurrence lacks exact lexical anchor')
        if 'predicate' in node or 'lemma' in node:errors.append(f'{path}: invented predicate/lemma identity')
        cid=node.get('ontology_type');check_type(cid,path+'.ontology_type')
        if cid:
            stats['typed_occurrences']+=1
            if cid not in {'Event','Situation'}:stats['specialized_occurrences']+=1
            if known(cid) and not (subtype(cid,'Event') or subtype(cid,'Situation')):
                errors.append(f'{path}: occurrence type {cid} is not an event/situation subtype')
        hasgram=node.get('tense_aspect') is not None or node.get('voice') is not None
        if hasgram != bool(node.get('grammatical_spans')):errors.append(f'{path}: occurrence grammar/evidence mismatch')
        for i,r in enumerate(node.get('roles',[])):
            role=r.get('role','')
            if role.startswith('SourceAnchored:'):
                cue=role.split(':',1)[1];cue_b=cue.encode();text_b=text.encode();anchor_ivs=[iv(a) for a in node.get('lexical_anchor',[])]
                candidates=[];start=0
                while cue_b:
                    at=text_b.find(cue_b,start)
                    if at<0:break
                    candidates.append((at,at+len(cue_b)));start=at+1
                contained=[c for c in candidates if any(ss['start']<=c[0] and c[1]<=ss['end'] for ss in src)]
                if not contained:errors.append(f'{path}.role{i}: source-anchored role cue {cue!r} not inside occurrence')
                elif not any(all(c[1]<=a or b<=c[0] for a,b in anchor_ivs) for c in contained):errors.append(f'{path}.role{i}: source role cue overlaps lexical predicate material')
            elif role.startswith('Ontology:'):
                rid=role.split(':',1)[1]
                if ':' in rid:errors.append(f'{path}.role{i}: learned ontology relation must be namespaceless: {rid}')
                elif rid not in RELATIONS:errors.append(f'{path}.role{i}: unknown ontology relation {rid}')
            elif role not in VALID_ROLES:errors.append(f'{path}.role{i}: unknown role {role}')
            v=r.get('value')
            if isinstance(v,dict):used|=walk(v,text,f'{path}.role{i}.value',bound)
            else:
                vu=collect_vars(v);used|=vu
                for x in vu:
                    if x not in bound:errors.append(f'{path}.role{i}: unbound variable {x}')
            if isinstance(v,dict):
                for cs in v.get('source_spans',[]):
                    if (not role.startswith('SourceAnchored:')) and src and not covered(cs,src):errors.append(f'{path}.role{i}: nested semantic complement outside occurrence source')
        if node.get('ellipsis_from'):stats['ellipses']+=1
        return used
    if typ in STRUCT and not node.get('operator_spans'):errors.append(f'{path}: structural operator lacks exact source cue')
    opgram=node.get('operator_tense_aspect') is not None or node.get('operator_voice') is not None;gsp=node.get('operator_grammatical_spans',[])
    if opgram:stats['operator_grammar']+=1
    if opgram and typ not in GRAM_OP:errors.append(f'{path}: grammar attached to non-grammatical operator {typ}')
    if opgram != bool(gsp):errors.append(f'{path}: operator grammar/evidence mismatch')
    if typ in {'quantified','interrogative'}:
        v=node.get('variable');domain=node.get('domain')
        if v:
            stats['variables']+=1;check_type(domain,path+'.domain')
            if domain:stats['typed_variables']+=1
        if typ=='quantified':
            q=node.get('quantifier')
            if isinstance(q,dict):
                if q.get('kind') not in NUMERIC_Q or not str(q.get('integer','')).isdigit():errors.append(f'{path}: invalid numeric quantifier {q!r}')
                else:stats['numeric_cardinality']+=1
            elif q not in {'Exists','ForAll'} and not (isinstance(q,str) and q.startswith('SourceAnchored:')):errors.append(f'{path}: invalid quantifier {q!r}')
            elif isinstance(q,str) and q.startswith('SourceAnchored:'):
                cue=q.split(':',1)[1]
                if not cue:errors.append(f'{path}: empty source-anchored quantifier')
        if typ=='interrogative':
            kind=node.get('interrogative')
            if kind in {'Wh','Reason'} and not v:errors.append(f'{path}: {kind} interrogative missing answer variable')
            if kind in {'Polar','Alternative','Tag'} and v:errors.append(f'{path}: non-WH interrogative has answer variable')
        child_used=walk(node.get('body'),text,path+'.body',bound|({v} if v else set()));used|=child_used
        if v and v not in child_used:errors.append(f'{path}: bound variable {v} is unused')
        return used-{v} if v else used
    if typ in {'negation','modal','capability','attitude','phase','quotation'}:return walk(node.get('content'),text,path+'.content',bound)
    if typ=='speech_act':return walk(node.get('content'),text,path+'.content',bound)
    if typ in {'implication','counterfactual','causal'}:
        out=set()
        for side in ('left','right'):
            v=node.get(side)
            if isinstance(v,str) and v.startswith('v_'):
                out.add(v)
                if v not in bound:errors.append(f'{path}.{side}: unbound semantic-target variable {v}')
            else:out|=walk(v,text,path+'.'+side,bound)
        return out
    if typ in {'conjunction','disjunction'}:
        for i,m in enumerate(node.get('members',[])):used|=walk(m,text,f'{path}.m{i}',bound)
        return used
    if typ=='temporal':
        used|=walk(node.get('subject'),text,path+'.subject',bound);used|=collect_vars(node.get('object'))
        for x in collect_vars(node.get('object')):
            if x not in bound:errors.append(f'{path}: unbound temporal variable {x}')
        return used
    if typ=='equality':
        used|=collect_vars(node.get('left_term'));used|=collect_vars(node.get('right_term'))
        for x in used:
            if x not in bound:errors.append(f'{path}: unbound equality variable {x}')
        return used
    return used

for row in rows:
    text=row['text'];idx=row['sample_index']
    if idx<1 or idx>36:errors.append(f'S{idx}: out of fixed audit range')
    rtypes=row.get('referent_types')
    if not isinstance(rtypes,dict) or not rtypes:errors.append(f'S{idx}: missing referent type inventory')
    else:
        stats['referents']+=len(rtypes)
        for name,cid in rtypes.items():
            check_type(cid,f'S{idx}.referent[{name}]')
            if cid not in {'Entity','Object','Agent'}:stats['specialized_referents']+=1
    stats['ambiguities']+=len(row.get('ambiguities',[]))
    for j,st in enumerate(row.get('statements',[])):
        mode=st.get('mode','');base=mode.split(':',1)[0]
        if base not in VALID_MODES:errors.append(f'S{idx}.{j}: invalid/legacy top-level mode {mode}')
        if base=='SourceAnchored':
            cue=mode.split(':',1)[1] if ':' in mode else ''
            if not cue or cue not in text:errors.append(f'S{idx}.{j}: source-anchored force lacks exact cue')
        if base=='Question' and st.get('expression',{}).get('type')!='interrogative':errors.append(f'S{idx}.{j}: direct question must root at Interrogative')
        walk(st.get('expression'),text,f'S{idx}.{j}')

if len(rows)!=36:errors.append(f'expected 36 fixed windows, got {len(rows)}')
if stats['typed_occurrences']!=stats['occurrences']:errors.append('not every occurrence is ontologically typed')
if stats['typed_variables']!=stats['variables']:errors.append('not every bound variable is ontologically typed')
if errors:
    print('strict v4 audit verification FAILED')
    for e in errors[:200]:print('ERROR:',e)
    print('errors:',len(errors));sys.exit(1)
print('strict v4 audit verification passed:',json.dumps(stats,sort_keys=True))
