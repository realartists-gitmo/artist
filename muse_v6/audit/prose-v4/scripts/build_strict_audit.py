#!/usr/bin/env python3
import argparse, json, re, hashlib
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
WORKSPACE=ROOT.parents[1]
import sys
sys.path.insert(0,str(WORKSPACE/'scripts'))
from ontology_symbols import concept_symbol, relation_symbol
_parser=argparse.ArgumentParser()
_parser.add_argument('--output-dir', type=Path, default=ROOT)
_args=_parser.parse_args()
SRC=ROOT/'audit_windows.jsonl'
OUT=_args.output_dir.resolve()
OUT.mkdir(parents=True, exist_ok=True)
rows={r['sample_index']:r for r in map(json.loads,SRC.open())}

# Audit-only nested projection of muse-prose-label-4.1. Lexical vocabulary remains
# exact-source-anchored, while every semantic object is ontologically typed.

def sp(text, needle, nth=0):
    starts=[m.start() for m in re.finditer(re.escape(needle),text)]
    if nth>=len(starts): raise ValueError(f'needle {needle!r} occurrence {nth} missing from {text!r}')
    a=starts[nth]; b=a+len(needle)
    return {'start':len(text[:a].encode()),'end':len(text[:b].encode()),'text':needle}

def spans_union(*xs):
    vals=[]
    for x in xs:
        if not x: continue
        if isinstance(x,list): vals.extend(x)
        elif isinstance(x,dict) and 'source_spans' in x: vals.extend(x['source_spans'])
        else: vals.append(x)
    uniq={(v['start'],v['end'],v['text']):v for v in vals}
    return [uniq[k] for k in sorted(uniq)]

def term(v): return v

def sp_in(text, needle, container):
    candidates=[]
    for n in range(len(re.findall(re.escape(needle),text))):
        x=sp(text,needle,n)
        if container['start'] <= x['start'] and x['end'] <= container['end']:
            candidates.append(x)
    if not candidates: raise ValueError(f'needle {needle!r} not inside {container["text"]!r}')
    return candidates[0]

COPULAS={'is','are','was','were','be','been','being',"'s","'re"}
SOFTWARE_ACTIVITY_TYPES={
    'compile':'se:CompilationActivity','compiling':'se:CompilationActivity','compiled':'se:CompilationActivity',
    'test':'se:TestingActivity','testing':'se:TestingActivity','tested':'se:TestingActivity',
    'build':'se:BuildActivity','building':'se:BuildActivity','built':'se:BuildActivity',
}

def infer_occurrence_type(anchor, roles, grammar, aspect=None):
    anchor_items=[anchor] if isinstance(anchor,(str,dict)) else list(anchor)
    words=[x.lower() for x in anchor_items if isinstance(x,str)]
    for word in words:
        key=re.sub(r'[^a-z]+','',word)
        if key in SOFTWARE_ACTIVITY_TYPES:
            return SOFTWARE_ACTIVITY_TYPES[key]
    grammar_words={re.sub(r'[^a-z]+','',x.lower()) for x in grammar if isinstance(x,str)}
    if aspect in {'Progressive','PerfectProgressive'}:
        subjects=[v for r,v in roles if r=='Subject' and isinstance(v,str)]
        if subjects and any(infer_referent_type(v) in {'ufo:Agent','agent:SoftwareAgent','harness:CodingAgent'} for v in subjects): return 'ufo:Action'
        return 'ufo:Event'
    # Predicative adjectives/nouns under a copula describe an obtaining situation.
    if grammar_words & COPULAS and not any(re.sub(r'[^a-z]+','',w) in COPULAS for w in words):
        return 'ufo:Situation'
    # Possession/existence and relational statives are situations rather than actions.
    if any(re.sub(r'[^a-z]+','',w) in {'has','have','had','contains','includes','means','equals','depends','remains'} for w in words):
        return 'ufo:Situation'
    # An intentionally described act with an explicit agent-like subject can use UFO Action.
    subjects=[v for r,v in roles if r=='Subject' and isinstance(v,str)]
    if subjects and any(infer_referent_type(v) in {'ufo:Agent','agent:SoftwareAgent','harness:CodingAgent'} for v in subjects):
        return 'ufo:Action'
    return 'ufo:Event'

def infer_referent_type(name):
    n=name.lower()
    if n.startswith('v_'): return None
    if any(k in n for k in ('speaker_and_agent','agent_or_peer')): return 'ufo:Agent'
    if n in {'agent','codex','grok','opus','sonnet','chatgpt'} or n.endswith('_agent'): return 'agent:SoftwareAgent'
    if n=='speaker' or n.endswith('_speaker') or n in {'user','we','i'}: return 'ufo:Agent'
    if 'tool' in n: return 'agent:Tool'
    if 'command' in n: return 'comp:Command'
    if 'path' in n: return 'comp:Path'
    if any(k in n for k in ('source_file','file')): return 'comp:File'
    if any(k in n for k in ('source_code','code_artifact','codebase','patch')): return 'se:SourceCodeArtifact'
    if any(k in n for k in ('software','program','binary','executable')): return 'se:SoftwareArtifact'
    if 'test_artifact' in n: return 'se:TestArtifact'
    if any(k in n for k in ('proposition','claim','question_content')): return 'ufo:Proposition'
    if any(k in n for k in ('quality','margin','degree','ratio','distance','size','width','height','rate')): return 'ufo:Quality'
    return 'ufo:Entity'

def infer_variable_domain(var):
    n=(var or '').lower()
    if any(k in n for k in ('build','task','run','event','action')): return 'ufo:Event'
    if any(k in n for k in ('reason','claim','proposition')): return 'ufo:Proposition'
    if any(k in n for k in ('agent','speaker','who')): return 'ufo:Agent'
    if 'file' in n: return 'comp:File'
    return 'ufo:Entity'

def O(text, anchor, phrase, roles=(), tense=None, aspect=None, voice=None, grammar=(), ontology_type=None):
    src=[sp(text,phrase) if isinstance(phrase,str) else phrase]
    anchor_items=[anchor] if isinstance(anchor,(str,dict)) else anchor
    a=[sp_in(text,x,src[0]) if isinstance(x,str) else x for x in anchor_items]
    gs=[sp_in(text,x,src[0]) if isinstance(x,str) else x for x in grammar]
    qualified_type=ontology_type or infer_occurrence_type(anchor,roles,grammar,aspect)
    normalized_roles=[]
    for r,v in roles:
        # When the ontology establishes an intentional action and the grammatical subject is an agent, preserve that semantic relation.
        if r=='Subject' and qualified_type in {'ufo:Action','se:BuildActivity','se:CompilationActivity','se:TestingActivity'} and isinstance(v,str) and infer_referent_type(v) in {'ufo:Agent','agent:SoftwareAgent','harness:CodingAgent'}:
            r='Agent'
        normalized_roles.append({'role':r,'value':v})
    return {'type':'occurrence','ontology_type':concept_symbol(qualified_type),'lexical_anchor':a,'roles':normalized_roles,
            'tense_aspect':None if tense is None and aspect is None else {'tense':tense,'aspect':aspect or 'Simple'},
            'voice':voice,'grammatical_spans':gs,'source_spans':src}

def _semantic_spans(x):
    if isinstance(x,dict): return x.get('source_spans',[])
    if isinstance(x,list):
        z=[]
        for y in x: z.extend(_semantic_spans(y))
        return z
    return []

def sp_near(text, needle, context):
    candidates=[sp(text,needle,n) for n in range(len(re.findall(re.escape(needle),text)))]
    if not candidates: raise ValueError(f'needle {needle!r} missing')
    spans=_semantic_spans(context) if not (isinstance(context,list) and context and isinstance(context[0],dict) and 'start' in context[0]) else context
    if not spans: return candidates[0]
    lo=min(x['start'] for x in spans); hi=max(x['end'] for x in spans)
    def score(x):
        if lo <= x['start'] and x['end'] <= hi: return (0,abs(x['start']-lo))
        if x['end'] <= lo: return (lo-x['end'],0)
        return (x['start']-hi,1)
    return min(candidates,key=score)

def cue_spans(text,cue,context):
    items=[cue] if isinstance(cue,(str,dict)) else cue
    return [sp_near(text,x,context) if isinstance(x,str) else x for x in items]

def wrap(t,text,cue,child,**kw):
    c=cue_spans(text,cue,child)
    return {'type':t,'operator_spans':c,'source_spans':spans_union(c,child),**kw,'content':child}

def NEG(text,cue,x): return wrap('negation',text,cue,x)
def MOD(text,kind,cue,x): return wrap('modal',text,cue,x,modality=kind)
def CAP(text,bearer,cue,x): return wrap('capability',text,cue,x,bearer=bearer)
def ATT(text,kind,holder,cue,x): return wrap('attitude',text,cue,x,attitude=kind,holder=holder)
def PHASE(text,kind,cue,x): return wrap('phase',text,cue,x,phase=kind)
def QUOTE(text,cue,x): return wrap('quotation',text,cue,x)
def IQ(text,kind,cue,body,var=None,domain=None,ellipsis_from=None):
    c=cue_spans(text,cue,body)
    if var and domain is None: domain=infer_variable_domain(var)
    if domain is not None: domain=concept_symbol(domain)
    z={'type':'interrogative','interrogative':kind,'variable':var,'domain':domain,'body':body,'operator_spans':c,'source_spans':spans_union(c,body)}
    if ellipsis_from: z['ellipsis_from']=ellipsis_from
    return z

def SA(text,act,speaker,cue,content,addressees=()):
    c=cue_spans(text,cue,content)
    return {'type':'speech_act','act':act,'speaker':speaker,'addressees':list(addressees),'content':content,'operator_spans':c,'source_spans':spans_union(c,content)}
def BIN(t,text,cue,a,b,**kw):
    c=cue_spans(text,cue,[a,b])
    return {'type':t,'operator_spans':c,'source_spans':spans_union(c,a,b),'left':a,'right':b,**kw}
def IMP(text,cue,a,b): return BIN('implication',text,cue,a,b)
def CAUSE(text,rel,cue,a,b): return BIN('causal',text,cue,a,b,relation=rel)
def TEMP(text,rel,cue,a,anchor):
    context=[a,anchor] if isinstance(anchor,dict) else a
    c=cue_spans(text,cue,context)
    if isinstance(anchor,str): anchor={'kind':'source_time','span':sp_near(text,anchor,a)}
    return {'type':'temporal','relation':rel,'subject':a,'object':anchor,'operator_spans':c,'source_spans':spans_union(c,a, anchor.get('span') if isinstance(anchor,dict) else None)}
def NARY(t,text,cue,*members):
    c=cue_spans(text,cue,list(members))
    return {'type':t,'members':list(members),'operator_spans':c,'source_spans':spans_union(c,*members)}
def AND(text,cue,*m): return NARY('conjunction',text,cue,*m)
def OR(text,cue,*m): return NARY('disjunction',text,cue,*m)
def EQ(text,cue,left,right):
    c=cue_spans(text,cue,[])
    return {'type':'equality','left_term':left,'right_term':right,'operator_spans':c,'source_spans':c}
def QUANT(text,kind,cue,var,body,domain=None):
    c=cue_spans(text,cue,body)
    if domain is None: domain=infer_variable_domain(var)
    domain=concept_symbol(domain)
    numeric={'SourceAnchored:only 607':{'kind':'Exactly','integer':'607'},'SourceAnchored:only one':{'kind':'Exactly','integer':'1'},'SourceAnchored:28':{'kind':'Exactly','integer':'28'}}
    quantifier=numeric.get(kind,kind)
    return {'type':'quantified','quantifier':quantifier,'variable':var,'domain':domain,'body':body,'operator_spans':c,'source_spans':spans_union(c,body)}
def OPGRAM(text,node,tense=None,aspect=None,voice=None,grammar=()):
    node=dict(node); node['operator_tense_aspect']=None if tense is None and aspect is None else {'tense':tense,'aspect':aspect or 'Simple'}; node['operator_voice']=voice; node['operator_grammatical_spans']=[sp_near(text,x,node.get('operator_spans',[])) for x in grammar]; return node
def ST(mode,expr,note=None):
    d={'mode':mode,'expression':expr,'source_spans':expr['source_spans']}
    if note:d['note']=note
    return d

spec={}
def add(i,*statements,ambiguities=()): spec[i]={'statements':list(statements),'ambiguities':list(ambiguities)}

# 01
T=rows[1]['text']
greet=O(T,'Hi','Hi Opus',[('Subject','speaker'),('IndirectObject','agent')])
construct=O(T,'constructing',"we were constructing 'one master debate database to rule them all.'", [('Subject','speaker'),('DirectObject','db')], tense='Past',aspect='Progressive',voice='Active',grammar=('were','constructing'))
idiotic=O(T,['completely','idiotic'],'This was completely idiotic',[('Subject','db')],tense='Past',voice='Active',grammar=('was',))
nuked=O(T,'nuked','nuked our disk use',[('Subject','db'),('DirectObject','disk_use')],tense='Past',voice='Active',grammar=('nuked',))
truth=O(T,['kernel','truth'],'there was a kernel of truth inside it',[('Subject','kernel_truth'),('SourceAnchored:inside','db')],tense='Past',voice='Active',grammar=('was',))
deleg=O(T,'delegated','which parts of the task I delegated to you vs codex vs grok',[('Subject','speaker'),('DirectObject','v_parts'),('SourceAnchored:to','agent_or_peer')],tense='Past',voice='Active',grammar=('delegated',))
which=IQ(T,'Wh','which',deleg,var='v_parts')
remember=ATT(T,'Recollection','speaker','remember',which)
remember=NEG(T,"can't",CAP(T,'speaker',"can't",remember))
commit=O(T,'committed','you committed anything about it to global memory',[('Subject','agent'),('DirectObject','anything_about_db'),('SourceAnchored:to','memory')],tense='Past',voice='Active',grammar=('committed',))
ifcommit=IQ(T,'Polar','if',commit)
know_att=OPGRAM(T,ATT(T,'Knowledge','speaker',['dont','know'],ifcommit),tense='Present',voice='Active',grammar=('dont','know'))
know=NEG(T,'dont',know_att)
did=IQ(T,'Polar','Did',commit,ellipsis_from='previous committed proposition')
start=O(T,'starting',"where we're starting from",[('Subject','speaker_and_agent'),('SourceAnchored:from','v_start')],tense='Present',aspect='Progressive',voice='Active',grammar=("we're",'starting'))
where=IQ(T,'Wh','where',start,var='v_start')
see=O(T,'see',"see where we're starting from",[('Subject','speaker'),('Content',where)])
want=OPGRAM(T,ATT(T,'Desire','speaker','want',see),tense='Present',voice='Active',grammar=('want',))
rev=O(T,'had',"I've just had a revelation",[('Subject','speaker'),('DirectObject','revelation')],tense='Present',aspect='Perfect',voice='Active',grammar=("I've",'had'))
think=OPGRAM(T,ATT(T,'Thought','speaker','think',rev),tense='Present',voice='Active',grammar=('think',))
because=CAUSE(T,'Motivates','because',think,want)
add(1,ST('SourceAnchored:Hi',greet),ST('Assertion',TEMP(T,'At','Earlier',construct,'Earlier')),ST('Assertion',idiotic),ST('Assertion',nuked),ST('Assertion',truth),ST('Assertion',remember),ST('Assertion',know),ST('Question',did,'licensed local ellipsis'),ST('Assertion',want),ST('Assertion',think),ST('Assertion',because))

# 02
T=rows[2]['text']
build=O(T,'build',"we're going to build the LM mid flywheel",[('Subject','speaker_and_agent'),('DirectObject','lm_mid_flywheel')],tense='Future',voice='Active',grammar=("we're going to",'build'))
intent=build
improve=O(T,'improves','it improves the quality of the whole flywheel stacking',[('Subject','lm_mid_flywheel'),('DirectObject','flywheel_quality')],tense='Present',voice='Active',grammar=('improves',))
cause=CAUSE(T,'Motivates','since',improve,build)
mistake=O(T,'making','How are you still making this mistake',[('Subject','agent'),('DirectObject','mistake'),('SourceAnchored:How','v_manner')],tense='Present',aspect='Progressive',voice='Active',grammar=('making',))
stillmistake=PHASE(T,'Continue','still',mistake)
howmistake=IQ(T,'Wh','How',stillmistake,var='v_manner')
sol1=O(T,['better','acoustics'],'better acoustics',[('PredicateComplement','solution')])
sol2=O(T,['audio','aware','model'],'audio aware model',[('PredicateComplement','solution')])
solutions=OR(T,'or',sol1,sol2)
said=OPGRAM(T,SA(T,'Assertion','agent','said',solutions,('speaker',)),tense='Past',voice='Active',grammar=('said',))
train=O(T,'training','training the encoder on more unlabelled audio',[('Subject','speaker_and_agent'),('DirectObject','encoder'),('SourceAnchored:on','more_unlabelled_audio')])
solve=O(T,'solve','we solve by training the encoder on more unlabelled audio',[('Subject','speaker_and_agent'),('DirectObject','better_acoustics'),('SourceAnchored:by',train)])
imagine=OPGRAM(T,ATT(T,'Assumption','speaker','imagine',solve),tense='Present',voice='Active',grammar=('imagine',))
trainq=O(T,'train','how much unlabelled audio did we train our current parakeet on?',[('Subject','speaker_and_agent'),('DirectObject','parakeet'),('SourceAnchored:on','v_amount')],tense='Past',voice='Active',grammar=('did','train'))
qamount=IQ(T,'Wh','how much',trainq,var='v_amount')
hours=O(T,'hours','How many hours?',[('Subject','parakeet'),('SourceAnchored:How many','v_hours')])
qhours=IQ(T,'Wh','How many',hours,var='v_hours',ellipsis_from='preceding training question')
running=O(T,'running','intake still running',[('Subject','intake')],tense='Present',aspect='Progressive',voice='Active',grammar=('running',))
qrunning=IQ(T,'Polar','Is',PHASE(T,'Continue','still',running))
at=O(T,'at','where is it at',[('Subject','intake'),('SourceAnchored:where','v_status')],tense='Present',voice='Active',grammar=('is',))
qwhere=IQ(T,'Wh','where',at,var='v_status')
eta=O(T,'ETA',"what's the ETA?",[('Subject','intake'),("SourceAnchored:what's",'v_eta')],tense='Present',voice='Active',grammar=("what's",))
qeta=IQ(T,'Wh','what',eta,var='v_eta')
add(2,ST('Assertion',intent),ST('Assertion',cause),ST('Question',howmistake),ST('Assertion',said),ST('Assertion',imagine),ST('Question',qamount),ST('Question',qhours,'licensed local ellipsis'),ST('Question',qrunning),ST('Question',qwhere),ST('Question',qeta))

# 03
T=rows[3]['text']
get=O(T,'get','get the pref composite',[('Subject','agent'),('DirectObject','pref_composite')])
impl=O(T,'implemented',"it's not already implemented",[('Subject','e_adjustment')],tense='Present',voice='Passive',grammar=("it's",'implemented'))
notimpl=NEG(T,'not',impl)
reported_impl=SA(T,'Command','agent','say',O(T,'implement','implement the E adjustment',[('Subject','speaker_and_agent'),('DirectObject','e_adjustment')]),('speaker',))
mean=O(T,'mean',"When you say implement the E adjustment, do you mean it's not already implemented",[('Subject','agent'),('Content',notimpl),('SourceAnchored:When',reported_impl)],tense='Present',voice='Active',grammar=('mean',))
qmean=IQ(T,'Polar','do',mean)
run=O(T,'run',"We're going to run a test league with 4 of the devs tonight",[('Subject','speaker_and_agent'),('DirectObject','test_league'),('SourceAnchored:with','four_devs')],tense='Future',voice='Active',grammar=("We're going to",'run'))
intentrun=run
runat=TEMP(T,'At','tonight',intentrun,'tonight')
working=O(T,'working','we get everything working',[('Subject','everything')])
possible=MOD(T,'Possible','possible',runat)
conditions=AND(T,'if',working,possible)
cond=IMP(T,'if',conditions,runat)
need=O(T,'need','you need for this',[('Subject','agent'),('DirectObject','data')],tense='Present',voice='Active',grammar=('need',))
qneed=IQ(T,'Polar','Is',need)
add(3,ST('Command',get),ST('Question',qmean),ST('Assertion',cond),ST('Question',qneed),ambiguities=({'kind':'Coreference','cue':'that','alternatives':['pref composite','test-league output','E-adjustment context']},))

# 04
T=rows[4]['text']
verify=O(T,'verify','verify until everythign was done',[('Subject','agent')])
doneall=O(T,'done','everythign was done',[('Subject','everything')],tense='Past',voice='Passive',grammar=('was','done'))
before_done=TEMP(T,'Before','until',verify,doneall)
notverify=MOD(T,'Prohibited','not',before_done)
told=OPGRAM(T,SA(T,'Command','speaker','told',notverify,('agent',)),tense='Past',voice='Active',grammar=('told',))
build=O(T,['cargo','build','thrashing'],'a cargo build thrashing the machine',[('Subject','cargo_build'),('DirectObject','machine')])
see=O(T,'see','I come here to see a cargo build thrashing the machine',[('Subject','speaker'),('DirectObject',build)],tense='Present',voice='Active',grammar=('come','see'))
dead=O(T,['dead','serious'],'That was dead serious',[('Subject','prior_instruction')],tense='Past',voice='Active',grammar=('was',))
s9=O(T,'done','Step 9 is not done',[('Subject','step9')],tense='Present',voice='Passive',grammar=('is','done'))
ns9=NEG(T,'not',s9)
build_instance=O(T,['cargo','build'],'a cargo build',[('Subject','v_build')])
existbuild=QUANT(T,'Exists','there','v_build',build_instance)
why_body={'type':'causal','relation':'Explains','left':'v_reason','right':existbuild,'operator_spans':[sp_near(T,'why',existbuild)],'source_spans':spans_union(sp_near(T,'why',existbuild),existbuild)}
why=IQ(T,'Reason','why',why_body,var='v_reason')
follow=O(T,'followed','instructions are followed',[('Subject','instructions')],tense='Present',voice='Passive',grammar=('are','followed'))
obl=MOD(T,'SourceAnchored','extremely',MOD(T,'Obligatory','critical',follow))
s10=O(T,'done','step 10 is not done',[('Subject','step10')],tense='Present',voice='Passive',grammar=('is','done'))
ns10=NEG(T,'not',s10)
building=O(T,['building','going'],'building going on',[('Subject','v_building')])
any_building=QUANT(T,'Exists','any','v_building',building)
prohib=MOD(T,'Prohibited',"shouldn't",any_building)
cond10=IMP(T,'if',ns10,prohib)
edit=O(T,'Edit','Edit until it looks eyeball FULLY complete (all steps)',[('Subject','agent'),('DirectObject','workspace')])
complete=O(T,'complete','it looks eyeball FULLY complete (all steps)',[('Subject','workspace')],tense='Present',voice='Active',grammar=('looks',))
untilc=TEMP(T,'Before','until',edit,complete)
verify2=O(T,'verify','you can verify',[('Subject','agent')])
perm=MOD(T,'Permitted','can',verify2)
after=TEMP(T,'After','THEN',perm,complete)
dead2=O(T,['Dead','serious'],'Dead serious',[('Subject','prior_instruction')])
add(4,ST('Assertion',told),ST('Assertion',see),ST('Assertion',dead),ST('Assertion',ns9),ST('Question',why),ST('Assertion',obl),ST('Assertion',cond10),ST('Assertion',dead2),ST('Command',untilc),ST('Command',after))

# 05
T=rows[5]['text']
scient=O(T,'scientific','be more scientific about this',[('Subject','speaker_and_agent')])
needscient=MOD(T,'Obligatory','need',scient)
samp1=O(T,'sample','Take a smaller sample of pool-2 rows',[('Subject','agent'),('DirectObject','pool2_rows')])
samp2=O(T,'sample','a proportionate sample of sonnet labelled rows',[('Subject','agent'),('DirectObject','sonnet_rows')])
benefit=O(T,'benefitting','the relabel is benefitting primarily from model strength',[('Subject','relabel'),('SourceAnchored:from','model_strength')],tense='Present',aspect='Progressive',voice='Active',grammar=('is','benefitting'))
iterb=O(T,'iteration','iteration',[('Subject','relabel')])
bad=O(T,['being','dickhead'],'is just being a dickhead',[('Subject','relabel')],tense='Present',aspect='Progressive',voice='Active',grammar=('is','being'))
alts=OR(T,'or',benefit,iterb,bad)
ialt=IQ(T,'Alternative','if',alts)
det=O(T,"determine", "That'll determine if the relabel is benefitting primarily from model strength, iteration, or is just being a dickhead", [('Subject','sampling_result'),('Content',ialt)],tense='Future',voice='Active',grammar=("That'll",'determine'))
good=O(T,'good','200 pool-2 rows + whatevers proportionate from the other rows should be good',[('Subject','sample_plan')])
expect=ATT(T,'Expectation','speaker','should',good)
thought=OPGRAM(T,ATT(T,'Thought','speaker','think',expect),tense='Present',voice='Active',grammar=('think',))
parallel=O(T,['Set','parallel'],'Set the sonnets off those in parallel',[('Subject','agent'),('DirectObject','sonnets')])
getrates=O(T,'Get','Get us diff disagreement rates across this',[('Subject','agent'),('DirectObject','diff_rates'),('IndirectObject','speaker_and_agent')])
boned=O(T,'boned','how boned we are',[('Subject','speaker_and_agent'),('SourceAnchored:how','v_degree')],tense='Present',voice='Active',grammar=('are',))
how=IQ(T,'Wh','how',boned,var='v_degree')
figure=O(T,'figure out','we can figure out how boned we are',[('Subject','speaker_and_agent'),('Content',how)])
purpose=CAUSE(T,'Purpose','so',getrates,figure)
add(5,ST('Assertion',needscient),ST('Command',samp1),ST('Command',samp2),ST('Assertion',det),ST('Assertion',thought),ST('Command',parallel),ST('Command',getrates),ST('Assertion',purpose))

# 06
T=rows[6]['text']
crash=O(T,'crashed','blendr crashed',[('Subject','blender')],tense='Past',voice='Active',grammar=('crashed',))
large=O(T,'large','Vitruvian mesh being so large',[('Subject','mesh')])
cause=CAUSE(T,'Causes','because',large,crash)
sure=OPGRAM(T,ATT(T,'SourceAnchored','speaker',["I'm",'sure','no doubt'],cause),tense='Present',voice='Active',grammar=("I'm",'sure'))
req=O(T,['really','require'],"Our game doesn't really require a realistic model",[('Subject','game'),('DirectObject','realistic_model')],tense='Present',voice='Active',grammar=("doesn't",'require'))
nreq=NEG(T,"doesn't",req)
better=O(T,'better','we might even be better off with stylized',[('Subject','game'),('SourceAnchored:with','stylized')])
maybe=MOD(T,'Possible','might',better)
plugv=O(T,'plug in','plug in just Vitruvian',[('Subject','agent'),('DirectObject','vitruvian')])
down=O(T,'downscale','downscale it',[('Subject','agent'),('DirectObject','vitruvian')])
crash2=O(T,'crash',"I don't crash",[('Subject','speaker')])
ncrash=NEG(T,"don't",crash2)
purp=CAUSE(T,'Purpose','so',down,ncrash)
possible=MOD(T,'Possible','possible',down)
cond=IMP(T,'if',possible,purp)
plugboth=O(T,sp(T,'plug in'),'whichever of the Antonia-Reom had both genders again',[('Subject','agent'),('DirectObject','antonia_both_genders')]); plugboth['source_spans']=spans_union(plugboth['source_spans'],sp(T,'plug in')); plugboth['ellipsis_from']='preceding plug-in request'
look=O(T,'look',"I'll take a look",[('Subject','speaker')],tense='Future',voice='Active',grammar=("I'll",'look'))
done=O(T,'done',"you're done",[('Subject','agent')],tense='Present',voice='Passive',grammar=("you're",'done'))
after=TEMP(T,'After','after',look,done)
add(6,ST('Assertion',crash),ST('Assertion',sure),ST('Assertion',nreq),ST('Assertion',maybe),ST('Request',plugv),ST('Request',cond),ST('Request',plugboth,'licensed local ellipsis of plug-in predicate'),ST('Assertion',after))

# 07
T=rows[7]['text']
shamble=O(T,'shambling',"it's shambling",[('Subject','mcp')],tense='Present',aspect='Progressive',voice='Active',grammar=("it's",'shambling'))
patch=O(T,'patch','patch us this MCP',[('Subject','speaker_and_agent'),('DirectObject','mcp')])
need=MOD(T,'SourceAnchored','really',MOD(T,'Obligatory','need',patch))
patch2=O(T,'patch','patch it through our shim to work how we need it',[('Subject','speaker_and_agent'),('DirectObject','mcp'),('SourceAnchored:through','shim')])
cap=CAP(T,'speaker_and_agent','Can',patch2)
workneed=O(T,'work','work how we need it',[('Subject','mcp'),('SourceAnchored:how','needed_manner')])
patchpurpose=CAUSE(T,'Purpose','to',patch2,workneed)
cap=CAP(T,'speaker_and_agent','Can',patchpurpose)
qcap=IQ(T,'Polar','Can',cap)
fail=O(T,'failed','The MCP failed before modifying the asset',[('Subject','mcp')],tense='Past',voice='Active',grammar=('failed',))
modify=O(T,'modifying','modifying the asset',[('Subject','mcp'),('DirectObject','asset')])
before=TEMP(T,'Before','before',fail,modify)
call=O(T,'calls','it calls a nonexistent maxx method',[('Subject','clearnode_script'),('DirectObject','maxx_method')],tense='Present',voice='Active',grammar=('calls',))
bug=O(T,'bug','clearnode.lua has a script bug',[('Subject','clearnode_script'),('DirectObject','script_bug')],tense='Present',voice='Active',grammar=('has',))
torso_fail=O(T,'failed','The torso-generation call then failed twice',[('Subject','torso_generation_call'),('SourceAnchored:twice','twice')],tense='Past',voice='Active',grammar=('failed',))
ret=O(T,'returned','the Vengi MCP returned an invalid initialization response',[('Subject','vengi_mcp'),('DirectObject','invalid_init_response')],tense='Past',voice='Active',grammar=('returned',))
why_torso=CAUSE(T,'Causes','because',ret,torso_fail)
unch=O(T,'unchanged','the current asset should still be unchanged',[('Subject','asset')])
still_unch=PHASE(T,'Continue','still',unch)
expect=ATT(T,'Expectation','chatgpt','should',still_unch)
img=O(T,'use','I did not use image generation',[('Subject','chatgpt'),('DirectObject','image_generation')],tense='Past',voice='Active',grammar=('did','use'))
nimg=NEG(T,'not',img)
def reported(x): return OPGRAM(T,SA(T,'Assertion','chatgpt','ChatGPT said',x,('speaker',)),tense='Past',voice='Active',grammar=('said',))
said_before,said_bug,said_call,said_torso,said_expect,said_nimg=[reported(x) for x in (before,bug,call,why_torso,expect,nimg)]
show=O(T,'shows','the most recent screenshot shows the console error',[('Subject','screenshot'),('DirectObject','console_error')],tense='Present',voice='Active',grammar=('shows',))
add(7,ST('Assertion',shamble),ST('Assertion',need),ST('Question',qcap),*[ST('Assertion',x) for x in (said_before,said_bug,said_call,said_torso,said_expect,said_nimg)],ST('Assertion',show))

# 08
T=rows[8]['text']
goodq=O(T,'Good','Good question',[('Subject','question')])
guess=O(T,'guess','guess',[('Subject','speaker')])
check=O(T,['actually','check'],'I can actually check rather than guess',[('Subject','speaker'),('DirectObject','caret_path'),('SourceAnchored:rather than',guess)])
cap=CAP(T,'speaker','can',check)
wait=O(T,'waiting',"I'm just waiting on the test run",[('Subject','speaker'),('SourceAnchored:on','test_run')],tense='Present',aspect='Progressive',voice='Active',grammar=("I'm",'waiting'))
trace=O(T,'trace','trace the real caret-reconciliation path for a non-collab document',[('Subject','speaker'),('DirectObject','caret_path')])
intendtrace=ATT(T,'Intention','speaker','Let me',trace)
go=O(T,'go through','a non-collab doc even go through the async runtime-flush path',[('Subject','noncollab_doc'),('DirectObject','flush_path')])
qgo=IQ(T,'Polar','does',go)
restore=O(T,'restored','how the caret is restored after a flush',[('Subject','caret'),('SourceAnchored:how','v_method')],tense='Present',voice='Passive',grammar=('is','restored'))
flush=O(T,'flush','a flush',[('Subject','runtime_flush')])
restore_after=TEMP(T,'After','after',restore,flush)
how=IQ(T,'Wh','how',restore_after,var='v_method')
crux_questions=AND(T,'and',qgo,how)
crux=O(T,['two','things'],"The crux is two things: (1) does a non-collab doc even go through the async runtime-flush path (if it edits purely locally, there's no race), and (2) how the caret is restored after a flush",[('Subject','crux'),('Content',crux_questions)],tense='Present',voice='Active',grammar=('is',))
look=O(T,'look','look',[('Subject','speaker')])
intendlook=ATT(T,'Intention','speaker','Let me',look)
editlocal=O(T,'edits','it edits purely locally',[('Subject','noncollab_doc')],tense='Present',voice='Active',grammar=('edits',))
race=O(T,'race',"there's no race",[('Subject','v_race')])
existsrace=QUANT(T,'Exists','no','v_race',race)
norace=NEG(T,'no',existsrace)
localcond=IMP(T,'if',editlocal,norace)
waitreason=CAUSE(T,'Enables','since',wait,check)
add(8,ST('Assertion',goodq),ST('Assertion',cap),ST('Assertion',wait),ST('Assertion',waitreason),ST('Assertion',intendtrace),ST('Assertion',crux),ST('Assertion',localcond),ST('Assertion',intendlook))

# 09
T=rows[9]['text']
cut=O(T,'cut','FIX 4 cut over-extract 14→9',[('Subject','fix4'),('DirectObject','over_extract')],tense='Past',voice='Active',grammar=('cut',))
split=O(T,'splits','The residual splits into: model org-extractions (Welle/Lumen/Saenz/Porous/Today — hard), a quote-guard miss (Capitalism), and `UNESCO Water`',[('Subject','residual'),('SourceAnchored:into','residual_parts')],tense='Present',voice='Active',grammar=('splits',))
reg=O(T,'regression-tested',"I've only regression-tested against **eval-set** controls",[('Subject','speaker'),('SourceAnchored:against','eval_controls')],tense='Present',aspect='Perfect',voice='Active',grammar=("I've",'regression-tested'))
touch=O(T,['heavily','touch'],'My fixes heavily touch the 90004xxx hardballs',[('Subject','fix4'),('DirectObject','hardballs')],tense='Present',voice='Active',grammar=('touch',))
breakx=O(T,'break',"they don't break hardball cites that were already **perfect**",[('Subject','fix4'),('DirectObject','hardball_cites')],tense='Present',voice='Active',grammar=("don't",'break'))
nbreak=NEG(T,"don't",breakx)
perfect=O(T,['already','perfect'],'hardball cites that were already **perfect**',[('Subject','hardball_cites')],tense='Past',voice='Active',grammar=('were',))
confirm=O(T,'confirm',"confirm they don't break hardball cites that were already **perfect**",[('Subject','speaker'),('Content',nbreak)])
must=MOD(T,'Obligatory','must',confirm)
dump=O(T,'dump','dump a sample of those',[('Subject','speaker'),('DirectObject','hardball_sample')])
intend=ATT(T,'Intention','speaker','Let me',dump)
add(9,ST('Assertion',cut),ST('Assertion',split),ST('Assertion',reg),ST('Assertion',touch),ST('Assertion',perfect),ST('Assertion',must),ST('Assertion',intend))

# 10
T=rows[10]['text']
great=O(T,['Great','notes'],'Great notes',[('Subject','notes')])
act=O(T,'act on','I can act on',[('Subject','speaker'),('DirectObject','triage')])
cap=CAP(T,'speaker','can',act)
triage_kind=O(T,['exactly','kind','triage'],'this is exactly the kind of triage I can act on',[('Subject','notes'),('Content',cap)],tense='Present',voice='Active',grammar=('is',))
items=O(T,['questions','items'],'A bunch of these are questions or Chinmay\'s "is this still real?" items',[('Subject','notes'),('PredicateComplement','questions_or_stale_items')],tense='Present',voice='Active',grammar=('are',))
check=O(T,['actually','check'],'actually check the code rather than guess',[('Subject','speaker'),('DirectObject','code')])
want=OPGRAM(T,ATT(T,'Desire','speaker','want',check),tense='Present',voice='Active',grammar=('want',))
write=O(T,'write back','write back',[('Subject','speaker')])
before=TEMP(T,'Before','before',want,write)
invest=O(T,'investigate','investigate the stale items and the one architectural question (PERF-1) in parallel',[('Subject','speaker'),('DirectObject','stale_items_and_perf1'),('SourceAnchored:in','parallel')])
confirm=O(T,'confirm','confirm the COR-1 mechanism myself',[('Subject','speaker'),('DirectObject','cor1_mechanism')])
both=AND(T,'and',invest,confirm)
intend=ATT(T,'Intention','speaker','Let me',both)
skept=O(T,['rightly','skeptical'],"you're rightly skeptical",[('Subject','user')],tense='Present',voice='Active',grammar=("you're",'skeptical'))
reason=CAUSE(T,'Motivates','since',skept,confirm)
add(10,ST('Assertion',great),ST('Assertion',triage_kind),ST('Assertion',items),ST('Assertion',before),ST('Assertion',intend),ST('Assertion',reason))


# 11
T=rows[11]['text']
needed=O(T,'needed','what I needed',[('Subject','speaker'),('DirectObject','v_needed')],tense='Past',voice='Active',grammar=('needed',))
qneeded=IQ(T,'Wh','what',needed,var='v_needed')
asked=OPGRAM(T,SA(T,'Question','user','asked',qneeded,('speaker',)),tense='Past',voice='Active',grammar=('asked',))
need=O(T,'need',"I probably don't need it",[('Subject','speaker'),('DirectObject','it')],tense='Present',voice='Active',grammar=("don't",'need'))
notneed=NEG(T,"don't",need)
prob=MOD(T,'SourceAnchored','probably',notneed)
said=OPGRAM(T,SA(T,'Assertion','speaker','said',prob,('user',)),tense='Past',voice='Active',grammar=('said',))
proceed=O(T,'Proceeding','Proceeding.',[('Subject','speaker')])
work=O(T,'working','working it through',[('Subject','speaker'),('DirectObject','problem')])
smaller=O(T,'smaller',"the urgent part is **smaller than I'd been treating it**",[('Subject','urgent_part'),('SourceAnchored:than','prior_treatment')],tense='Present',voice='Active',grammar=('is',))
treat=O(T,'treating',"I'd been treating it",[('Subject','speaker'),('DirectObject','urgent_part')],tense='Past',aspect='PerfectProgressive',voice='Active',grammar=("I'd",'been','treating'))
claims=O(T,['always','claims'],'the rules were always claims about bits',[('Subject','rules'),('SourceAnchored:about','bits')],tense='Past',voice='Active',grammar=('were',))
perbit=O(T,['per-bit','semantics'],'under per-bit semantics',[('Subject','per_bit_semantics')])
sound=O(T,'sound','`Instance`/`Instantiate`/`Connective` become sound with *no kernel change at all*',[('Subject','three_rules'),('SourceAnchored:with','no_kernel_change')],tense='Present',voice='Active',grammar=('become',))
enable=CAUSE(T,'Enables','so',perbit,sound)
wrongsem=O(T,'wrong','What was wrong was the semantics',[('Subject','semantics')],tense='Past',voice='Active',grammar=('was',))
wrongcode=O(T,sp(T,'wrong'),'the code',[('Subject','code')]); wrongcode['source_spans']=spans_union(wrongcode['source_spans'],sp(T,'wrong')); wrongcode['ellipsis_from']='contrastive predicate: wrong'
notcode=NEG(T,'not',wrongcode)
contrast=AND(T,',',wrongsem,notcode)
make=O(T,'making','making the reference per-bit',[('Subject','speaker'),('DirectObject','reference')])
verify=O(T,'verify','verify that',[('Subject','speaker'),('SourceAnchored:that',enable)])
method=CAUSE(T,'Enables','by',make,verify)
intend=ATT(T,'Intention','speaker','Let me',method)
add(11,ST('Assertion',asked),ST('Assertion',said),ST('Assertion',proceed),ST('Assertion',work),ST('Assertion',smaller),ST('Assertion',treat),ST('Assertion',claims),ST('Assertion',enable),ST('Assertion',contrast),ST('Assertion',intend))

# 12
T=rows[12]['text']
stop=O(T,'Stop','Stop',[('Subject','speaker')])
correct=O(T,'correct','correct something first',[('Subject','speaker'),('DirectObject','something')])
necessary=MOD(T,'Necessary','need to',correct)
check=O(T,'Checking','Checking your votes',[('Subject','speaker'),('DirectObject','user_votes')])
right=O(T,['right','call'],'Checking your votes was the right call',[('Subject',check),('PredicateComplement','right_call')],tense='Past',voice='Active',grammar=('was',))
vote=O(T,'voted','you voted NO on #03 `needs_human`',[('Subject','user'),('DirectObject','NO'),('IndirectObject','item03')],tense='Past',voice='Active',grammar=('voted',))
built=O(T,'built','I built it anyway',[('Subject','speaker'),('DirectObject','item03')],tense='Past',voice='Active',grammar=('built',))
do=O(T,'do','An agent can do all of these except two-factor',[('Subject','generic_agent'),('DirectObject','all_actions'),('SourceAnchored:except','two_factor')])
cap=CAP(T,'generic_agent','can',do)
yolo_base=O(T,['YOLO','harness'],'for now artist is a YOLO harness',[('Subject','artist'),('PredicateComplement','yolo_harness')],tense='Present',voice='Active',grammar=('is',))
yolo=TEMP(T,'At','for now',yolo_base,'for now')
words_cap=SA(T,'Assertion','user','Your words',cap,('speaker',))
words_yolo=SA(T,'Assertion','user','Your words',yolo,('speaker',))
plug=O(T,'plug in','you want to plug in',[('Subject','user')])
want=OPGRAM(T,ATT(T,'Desire','user','want',plug),tense='Present',voice='Active',grammar=('want',))
restrict=O(T,'cucked',"we've arbitrarily cucked the agent",[('Subject','speaker_and_agent'),('DirectObject','agent')],tense='Present',aspect='Perfect',voice='Active',grammar=("we've",'cucked'))
reason=CAUSE(T,'Motivates','because',restrict,want)
notreason=NEG(T,'not',reason)
explicit=OPGRAM(T,SA(T,'Assertion','user',['were','explicit'],notreason,('speaker',)),tense='Past',voice='Active',grammar=('were','explicit'))
equiv=O(T,['exactly','that'],'A hard halt is exactly that',[('Subject','hard_halt'),('PredicateComplement','arbitrary_restriction')],tense='Present',voice='Active',grammar=('is',))
revert=O(T,'Reverting','Reverting it now',[('Subject','speaker'),('DirectObject','item03')])
revertnow=TEMP(T,'At','now',revert,'now')
add(12,ST('SourceAnchored:Stop',stop),ST('Assertion',necessary),ST('Assertion',right),ST('Assertion',vote),ST('Assertion',built),ST('Assertion',words_cap),ST('Assertion',words_yolo),ST('Assertion',explicit),ST('Assertion',equiv),ST('Assertion',revertnow))

# 13
T=rows[13]['text']
surface=O(T,'surfaced','Two real issues surfaced',[('Subject','two_issues')],tense='Past',voice='Active',grammar=('surfaced',))
important=O(T,'important','one is more important than the τ-floor',[('Subject','one_issue'),('SourceAnchored:than','tau_floor')],tense='Present',voice='Active',grammar=('is',))
read=O(T,'read','read the ingest script',[('Subject','speaker'),('DirectObject','ingest_script')])
intend=ATT(T,'Intention','speaker','Let me',read)
persist=O(T,'persisted','rejects ("no") appear to not be persisted at all',[('Subject','rejects')],voice='Passive',grammar=('be','persisted'))
npersist=NEG(T,'not',persist)
susp=OPGRAM(T,ATT(T,'Suspicion','speaker','appear to',npersist),tense='Present',voice='Active',grammar=('appear',))
row=O(T,'rows','only 607 rows',[('Subject','v_row')])
q607=QUANT(T,'SourceAnchored:only 607','only 607','v_row',row)
rows607=O(T,'has','gold has only 607 rows',[('Subject','gold'),('DirectObject',q607)],tense='Present',voice='Active',grammar=('has',))
origin=O(T,'from','all from yes/edit',[('Subject','v_gold_row'),('PredicateComplement','yes_edit')])
all_from_yesedit=QUANT(T,'ForAll','all','v_gold_row',origin)
discard_base=O(T,'thrown away','the reject-validation set you want for the discriminator is being thrown away every page',[('Subject','reject_validation')],tense='Present',aspect='Progressive',voice='Passive',grammar=('is','being','thrown away'))
discard_at_page=TEMP(T,'At','page',discard_base,{'kind':'variable','value':'v_page'})
discard=QUANT(T,'ForAll','every','v_page',discard_at_page)
would=IMP(T,'would mean',npersist,discard)
readreason=CAUSE(T,'Motivates','because',susp,intend)
add(13,ST('Assertion',surface),ST('Assertion',important),ST('Assertion',intend),ST('Assertion',readreason),ST('Assertion',susp),ST('Assertion',rows607),ST('Assertion',all_from_yesedit),ST('Assertion',would))

# 14
T=rows[14]['text']
inplace=O(T,'in place','The fix, instrument counter, and fail-loud test are in place',[('Subject','fix_counter_test')],tense='Present',voice='Active',grammar=('are',))
fail=O(T,['failing','quietly'],'something that could start failing quietly',[('Subject','something')])
begin=PHASE(T,'Begin','start',fail)
possible=MOD(T,'Possible','could',begin)
see=O(T,'see','you see something that could start failing quietly',[('Subject','generic_agent'),('DirectObject','something')],tense='Present',voice='Active',grammar=('see',))
condition=AND(T,'that',see,possible)
write=O(T,'write','write a test',[('Subject','generic_agent'),('DirectObject','test')])
cmd=SA(T,'Command','protocol','write',write,('generic_agent',))
rule=IMP(T,'if',condition,cmd)
protocol=OPGRAM(T,SA(T,'Assertion','protocol','says',rule,('speaker',)),tense='Present',voice='Active',grammar=('says',))
introduced=O(T,'introduced','I introduced `char_at` in P4 slice-2',[('Subject','speaker'),('DirectObject','char_at'),('SourceAnchored:in','p4_slice2')],tense='Past',voice='Active',grammar=('introduced',))
exists=O(T,'exists','the same per-element `char_at`-on-large-body footgun exists **anywhere else** in the projection/collab hot paths',[('Subject','footgun'),('SourceAnchored:anywhere else','anywhere_else'),('SourceAnchored:in','projection_collab_hot_paths')],tense='Present',voice='Active',grammar=('exists',))
whether=IQ(T,'Polar','whether',exists)
check2=O(T,'check','check whether the same per-element `char_at`-on-large-body footgun exists **anywhere else** in the projection/collab hot paths',[('Subject','speaker'),('Content',whether)])
intend2=ATT(T,'Intention','speaker','let me',check2)
add(14,ST('Assertion',inplace),ST('Assertion',protocol),ST('Assertion',introduced),ST('Assertion',intend2))

# 15
T=rows[15]['text']
running=O(T,'running','From the OS side only one is running',[('Subject','v_running_task'),('SourceAnchored:From','os_side')],tense='Present',aspect='Progressive',voice='Active',grammar=('is','running'))
only_one_running=QUANT(T,'SourceAnchored:only one','only one','v_running_task',running)
second_is_task=O(T,'task','the second is a task',[('Subject','second_task')],tense='Present',voice='Active',grammar=('is',))
reg_base=O(T,'registered','the second is a task the harness still has registered',[('Subject','harness'),('DirectObject','second_task')],tense='Present',aspect='Perfect',voice='Active',grammar=('has','registered'))
reg=PHASE(T,'Continue','still',reg_base)
reported=O(T,'reported back','that never reported back',[('Subject','second_task')],tense='Past',voice='Active',grammar=('reported',))
nreported=NEG(T,'never',reported)
ident=EQ(T,':','second_task','waiter_bhhu0ermd')
know=OPGRAM(T,ATT(T,'Knowledge','speaker','know',ident),tense='Present',voice='Active',grammar=('know',))
armed_base=O(T,'armed','a waiter I armed long ago with `until [ -x artist ] && [ cargo count -eq 0 ]`',[('Subject','speaker'),('DirectObject','waiter_bhhu0ermd'),('SourceAnchored:with','condition')],tense='Past',voice='Active',grammar=('armed',))
armed=TEMP(T,'At','long ago',armed_base,'long ago')
idle=O(T,['essentially','never','idle'],'On this box cargo is essentially never idle',[('Subject','cargo'),('SourceAnchored:On','this_box')],tense='Present',voice='Active',grammar=('is',))
become=O(T,'become',"that condition can't become true",[('Subject','condition'),('PredicateComplement','true')])
possible=MOD(T,'Possible',"can't",become)
impossible=NEG(T,"can't",possible)
prevents=CAUSE(T,'Prevents','so',idle,become)
add(15,ST('Assertion',only_one_running),ST('Assertion',second_is_task),ST('Assertion',reg),ST('Assertion',nreported),ST('Assertion',know),ST('Assertion',armed),ST('Assertion',idle),ST('Assertion',impossible),ST('Assertion',prevents))

# 16
T=rows[16]['text']
conclusion=O(T,'conclusion','this is the conclusion your own pipeline doc already reached',[('Subject','this')],tense='Present',voice='Active',grammar=('is',))
reached=O(T,['already','reached'],'your own pipeline doc already reached',[('Subject','pipeline_doc'),('DirectObject','this_conclusion')],tense='Past',voice='Active',grammar=('reached',))
polish=O(T,'polishing','polishing a monolith',[('Subject','speaker'),('DirectObject','monolith')])
drive=O(T,'driven','I should have driven toward instead of polishing a monolith',[('Subject','speaker'),('SourceAnchored:toward','this_conclusion'),('SourceAnchored:instead of',polish)],aspect='Perfect',voice='Active',grammar=('have','driven'))
should=MOD(T,'Obligatory','should',drive)
ships=O(T,'ships','Shadows of Doubt ships *separate haircut models over a base head*',[('Subject','sod'),('DirectObject','haircut_models'),('SourceAnchored:over','base_head')],tense='Present',voice='Active',grammar=('ships',))
own=O(T,'owns','the compiler owns geometry',[('Subject','compiler'),('DirectObject','geometry')],tense='Present',voice='Active',grammar=('owns',))
comes=O(T,'comes','variation comes from composition',[('Subject','variation'),('SourceAnchored:from','composition')],tense='Present',voice='Active',grammar=('comes',))
doccontent=AND(T,'and',own,comes)
docsays=OPGRAM(T,SA(T,'Assertion','pipeline_doc','says',doccontent,('speaker',)),tense='Present',voice='Active',grammar=('says',))
modular=O(T,['modular','en masse'],'A fused skull with baked-in hair and features can never be "modular en masse" no matter how many iterations it gets',[('Subject','fused_skull'),('SourceAnchored:with','baked_hair_features')])
can=MOD(T,'Possible','can',modular)
never=NEG(T,'never',can)
gets=O(T,'gets','how many iterations it gets',[('Subject','fused_skull'),('SourceAnchored:how many','v_iterations')],tense='Present',voice='Active',grammar=('gets',))
regardless=IMP(T,'no matter how many',gets,never)
limit=QUANT(T,'ForAll','no matter how many','v_iterations',regardless)
scrap=O(T,'Scrapping','Scrapping v12',[('Subject','speaker'),('DirectObject','v12')])
build=O(T,'building','building the component system from zero',[('Subject','speaker'),('DirectObject','component_system'),('SourceAnchored:from','zero')])
coord=AND(T,'and',scrap,build)
add(16,ST('Assertion',conclusion),ST('Assertion',reached),ST('Assertion',should),ST('Assertion',ships),ST('Assertion',docsays),ST('Assertion',limit),ST('Assertion',coord))


# 17
T=rows[17]['text']
hunt=O(T,['autonomously','hunt'],'You will autonomously hunt high RoI white hat bug bounties in line with the workspace set up for you here',[('Subject','agent'),('DirectObject','bounties'),('SourceAnchored:in line with','workspace')],tense='Future',voice='Active',grammar=('will','hunt'))
loop=O(T,['durably','looped'],'This task will be durably looped over hours',[('Subject','task'),('SourceAnchored:over','hours')],tense='Future',voice='Passive',grammar=('will','be','looped'))
persist=O(T,'persist','persist without my intervention',[('Subject','agent'),('SourceAnchored:without','speaker_intervention')])
cap=CAP(T,'agent','be able to',persist)
want=OPGRAM(T,ATT(T,'Desire','speaker','want',cap),tense='Present',voice='Active',grammar=('want',))
comfortable=O(T,'comfortable',"you'd be comfortable doing that",[('Subject','agent'),('DirectObject','hunt')])
would=MOD(T,'SourceAnchored',"you'd",comfortable)
qcomfort=IQ(T,'Polar','if',would)
hunt2=O(T,'hunt','hunt for RoI',[('Subject','agent'),('SourceAnchored:for','roi')])
begin=PHASE(T,'Begin','beginning',hunt2)
comfortable2=O(T,'comfortable',"you'd be comfortable beginning to hunt for RoI",[('Subject','agent'),('Content',begin)])
would2=MOD(T,'SourceAnchored',"you'd",comfortable2)
setup=O(T,'set up',"items you'd need set up",[('Subject','agent'),('DirectObject','v_items')])
need=O(T,'need',"what other items you'd need set up before you'd be comfortable beginning to hunt for RoI",[('Subject','agent'),('Content',setup),('SourceAnchored:before',would2)])
qitems=IQ(T,'Wh','what',need,var='v_items')
qs=AND(T,'/',qcomfort,qitems)
tell=O(T,'Tell',"Tell me if you'd be comfortable doing that / what other items you'd need set up before you'd be comfortable beginning to hunt for RoI",[('Subject','agent'),('IndirectObject','speaker'),('Content',qs)])
add(17,ST('Command',hunt),ST('Assertion',loop),ST('Assertion',want),ST('Request',tell))

# 18
T=rows[18]['text']
yeah=O(T,'Yeah','Yeah',[('Subject','speaker')])
check=O(T,'check','check on the failed IDs',[('Subject','speaker_and_agent'),('SourceAnchored:on','failed_ids')])
issue=O(T,'issue','disk use is the issue',[('Subject','disk_use'),('PredicateComplement','issue')],tense='Present',voice='Active',grammar=('is',))
clean=O(T,['cargo','clean'],'cargo clean my rust projects',[('Subject','agent'),('DirectObject','rust_projects')])
perm=MOD(T,'Permitted','can',clean)
cond=IMP(T,'if',issue,perm)
add(18,ST('SourceAnchored:Yeah',yeah),ST('Suggestion',check),ST('Assertion',cond))

# 19
T=rows[19]['text']
has=O(T,'has','Neon has dual-account sessions and open share/branch hypotheses',[('Subject','neon'),('DirectObject','dual_account_sessions'),('DirectObject','share_branch_hypotheses')],tense='Present',voice='Active',grammar=('has',))
run=O(T,'running',"running that as this tick's unit",[('Subject','speaker'),('DirectObject','neon_hypothesis_unit'),('SourceAnchored:as','tick_unit')])
add(19,ST('Assertion',has),ST('Assertion',run))

# 20
T=rows[20]['text']
armed=O(T,'armed','Hunt armed',[('Subject','hunt')])
signup=O(T,['signup','path'],'app signup path for API tokens',[('Subject','signup_path'),('SourceAnchored:for','api_tokens')])
unit=O(T,'Unit','Unit: Lightspark unauth payment/hash lookups + app signup path for API tokens',[('Subject','hunt'),('DirectObject','lightspark_lookup_unit'),('Content',signup)])
add(20,ST('Assertion',armed),ST('Assertion',unit))

# 21
T=rows[21]['text']
armed=O(T,'armed','Hunt armed',[('Subject','hunt')])
unit=O(T,'Unit','Unit: Kronor/Boozt payment GraphQL surface',[('Subject','hunt'),('DirectObject','kronor_graphql_unit')])
authz=O(T,'authz/IDOR','authz/IDOR',[('Subject','kronor_graphql_unit')])
roi=O(T,['high','RoI'],'high RoI',[('Subject','kronor_graphql_unit')])
cond=IMP(T,'if',authz,roi)
add(21,ST('Assertion',armed),ST('Assertion',unit),ST('Assertion',cond))

# 22
T=rows[22]['text']
armed=O(T,'armed','Hunt armed',[('Subject','hunt')])
locate=O(T,'locate','locate World App SIWE backend (treasure-map high impact) via source/docs',[('Subject','speaker'),('DirectObject','world_app_backend'),('SourceAnchored:via','source_docs')])
unit=O(T,'Unit','Unit: locate World App SIWE backend (treasure-map high impact) via source/docs',[('Subject','hunt'),('Content',locate)])
impact=O(T,['treasure-map','high','impact'],'treasure-map high impact',[('Subject','world_app_backend')])
add(22,ST('Assertion',armed),ST('Assertion',unit),ST('Assertion',impact))

# 23
T=rows[23]['text']
need=O(T,'need','Token might need exchange for some APIs',[('Subject','token'),('DirectObject','exchange'),('SourceAnchored:for','some_apis')])
might=MOD(T,'Possible','might',need)
failed=O(T,'failed','access/exchange failed with 401 earlier',[('Subject','access_exchange'),('SourceAnchored:with','401')],tense='Past',voice='Active',grammar=('failed',))
earlier=TEMP(T,'At','earlier',failed,'earlier')
missing=O(T,'missing','missing token',[('Subject','token')])
response=SA(T,'Report','response_401','saying',missing,('speaker',))
formatneed=O(T,'needs','maybe needs cookie format not Bearer',[('Subject','token'),('DirectObject','cookie_format'),('SourceAnchored:not','bearer_format')],tense='Present',voice='Active',grammar=('needs',))
maybe=MOD(T,'SourceAnchored','maybe',formatneed)
add(23,ST('Assertion',might),ST('Assertion',earlier),ST('Assertion',response),ST('Assertion',maybe))

# 24
T=rows[24]['text']
interesting=O(T,'Interesting','Interesting openapi endpoints',[('Subject','endpoints')])
on=O(T,'on','openapi endpoints on id-infra.worldcoin.dev and stage-crypto.worldcoin.org',[('Subject','endpoints'),('PredicateComplement','domains')])
scope=O(T,'in','those are in structured scope',[('Subject','endpoints'),('PredicateComplement','structured_scope')],tense='Present',voice='Active',grammar=('are',))
qscope=IQ(T,'Polar','if',scope)
check=O(T,'Check','Check if those are in structured scope',[('Subject','speaker'),('Content',qscope)])
mentioned=O(T,'mentioned','treasure map mentioned worldcoin.dev wildcards',[('Subject','treasure_map'),('DirectObject','wildcards')],tense='Past',voice='Active',grammar=('mentioned',))
add(24,ST('Assertion',interesting),ST('Assertion',on),ST('Command',check),ST('Assertion',mentioned))

# 25
T=rows[25]['text']
interesting=O(T,'Interesting','Interesting findings',[('Subject','findings')])
login=O(T,'credential/login','credential/login with wrong password for non-existent email',[('Subject','login_attempt'),('SourceAnchored:with','wrong_password'),('SourceAnchored:for','nonexistent_email')])
account=O(T,'account',"An account doesn't exist for this email",[('Subject','v_account'),('SourceAnchored:for','this_email')])
exists=QUANT(T,'Exists','An','v_account',account)
no_account=NEG(T,"doesn't",exists)
quoted=QUOTE(T,'"',no_account)
result=O(T,'→',"→ \"An account doesn't exist for this email\"",[('Subject','login_attempt'),('Content',quoted)])
enum=O(T,'email enumeration','email enumeration',[('Subject','result_message')])
add(25,ST('Assertion',interesting),ST('Assertion',login),ST('Assertion',result),ST('Assertion',enum))

# 26
T=rows[26]['text']
appear=O(T,'appears','The probe appears in the inbox',[('Subject','probe'),('SourceAnchored:in','inbox')],tense='Present',voice='Active',grammar=('appears',))
sent=O(T,'SENT','we SENT it from ahutd44 to obama9255',[('Subject','speaker_and_agent'),('DirectObject','probe'),('SourceAnchored:from','ahutd44'),('SourceAnchored:to','obama9255')],tense='Past',voice='Active',grammar=('SENT',))
why=CAUSE(T,'Explains','because',sent,appear)
insent=O(T,'in',"it's in Sent folder / conversation view",[('Subject','probe'),('PredicateComplement','sent_view')],tense='Present',voice='Active',grammar=("it's",))
result=CAUSE(T,'Causes','so',sent,insent)
worked=O(T,'worked','forwarding worked',[('Subject','forwarding')],tense='Past',voice='Active',grammar=('worked',))
prove=O(T,'prove','That does NOT prove forwarding worked',[('Subject','probe_appearance'),('Content',worked)],tense='Present',voice='Active',grammar=('does','prove'))
nprove=NEG(T,'NOT',prove)
sentcopy=O(T,'sent copy',"it's the sent copy",[('Subject','probe'),('PredicateComplement','sent_copy')],tense='Present',voice='Active',grammar=("it's",))
expl=CAUSE(T,'Explains','-',sentcopy,nprove)
add(26,ST('Assertion',why),ST('Assertion',result),ST('Assertion',nprove),ST('Assertion',sentcopy),ST('Assertion',expl))

# 27
T=rows[27]['text']
acc=O(T,'accumulate','accumulate verified high RoI bounties',[('Subject','user'),('DirectObject','verified_bounties')])
setgoal=O(T,['set','goal'],'The user has set a goal to accumulate verified high RoI bounties',[('Subject','user'),('DirectObject','goal'),('Content',acc)],tense='Present',aspect='Perfect',voice='Active',grammar=('has','set'))
read=O(T,'read','read the plan first',[('Subject','speaker'),('DirectObject','plan')])
seed=O(T,'seed','seed todos from acceptance criteria',[('Subject','speaker'),('DirectObject','todos'),('SourceAnchored:from','acceptance_criteria')])
hunt=O(T,'hunting','hunting',[('Subject','speaker')])
beginhunt=PHASE(T,'Begin','start',hunt)
coord=AND(T,[',','and'],read,seed,beginhunt)
need=MOD(T,'Necessary','need to',coord)
first_seed=TEMP(T,'Before','first',read,seed)
first_hunt=TEMP(T,'Before','first',read,beginhunt)
first=AND(T,'and',first_seed,first_hunt)
read2=O(T,'read','read the plan file',[('Subject','speaker'),('DirectObject','plan_file')])
execute=O(T,'executing','executing',[('Subject','speaker')])
beginexec=PHASE(T,'Begin','begin',execute)
coord2=AND(T,'and',read2,beginexec)
intend=ATT(T,'Intention','speaker','Let me',coord2)
add(27,ST('Assertion',setgoal),ST('Assertion',need),ST('Assertion',first),ST('Assertion',intend))


# 28
T=rows[28]['text']
write=O(T,['Write','implementation-specific'],'Write all of these details with implementation-specific vision to the markdown with the exception of the ast-bro one and the ask notification one',[('Subject','agent'),('DirectObject','details'),('SourceAnchored:with','vision'),('SourceAnchored:to','markdown'),('SourceAnchored:exception of','ast_bro'),('SourceAnchored:exception of','ask_notification')])
bang=O(T,'bang','bang those out a little',[('Subject','speaker'),('DirectObject','ast_bro_and_ask')])
want=OPGRAM(T,ATT(T,'Desire','speaker','want',bang),tense='Present',voice='Active',grammar=('want',))
mark_base=O(T,'mark','Obviously mark the precise OpenVINO agent as deferred for now',[('Subject','agent'),('DirectObject','openvino_agent'),('PredicateComplement','deferred')])
mark=MOD(T,'SourceAnchored','Obviously',mark_base)
marknow=TEMP(T,'At','for now',mark,'for now')
writevision=O(T,'write','write vision yet',[('Subject','agent'),('DirectObject','vision')])
prohibit=MOD(T,'Prohibited',"don't",writevision)
support=O(T,'supports','ChatGPT web MCP supports it as a payload',[('Subject','chatgpt_web_mcp'),('DirectObject','vision'),('SourceAnchored:as','payload')],tense='Present',voice='Active',grammar=('supports',))
whether=IQ(T,'Polar','whether',support)
check=O(T,'check','instead, check whether ChatGPT web MCP supports it as a payload',[('Subject','agent'),('Content',whether),('SourceAnchored:instead',prohibit)])
pros=O(T,['pros','cons'],'pros cons for zenity, kdialog, yad, etc?',[('Subject','ui_tools'),('PredicateComplement','v_tradeoffs')])
qpros=IQ(T,'SourceAnchored','One question',pros,var='v_tradeoffs')
add(28,ST('Command',write),ST('Assertion',want),ST('Command',marknow),ST('Command',prohibit),ST('Command',check),ST('Question',qpros))

# 29
T=rows[29]['text']
wait=O(T,'Wait','Wait',[('Subject','speaker')])
accessible=O(T,"a11y'd", "we a11y'd ts",[('Subject','speaker_and_agent'),('DirectObject','target')],tense='Past',voice='Active',grammar=("a11y'd",))
drive=O(T,'drive','you drive it yourself',[('Subject','agent'),('DirectObject','target')])
cap=CAP(T,'agent',"can't",drive)
negcap=NEG(T,"can't",cap)
qdrive=IQ(T,'Polar',"can't",negcap)
askreason=CAUSE(T,'Motivates','since',accessible,qdrive)
issue=O(T,'issues','a number of issues',[('Subject','v_issue')])
exist=QUANT(T,'SourceAnchored:a number of',["There's",'a number of'],'v_issue',issue)
describe=O(T,'describing','describing',[('Subject','speaker'),('DirectObject','issues')])
explain=O(T,'explaining','explaining',[('Subject','speaker'),('DirectObject','issues')])
classify=O(T,'classifying','classifying',[('Subject','speaker'),('DirectObject','issues')])
coord=AND(T,'and',describe,explain,classify)
hard=O(T,'hard time','I have a hard time describing, explaining, and classifying',[('Subject','speaker'),('Content',coord)],tense='Present',voice='Active',grammar=('have',))
run=O(T,'run','you just run a conversation with the model (configured to be Luna ultracheap, dwai)',[('Subject','agent'),('DirectObject','conversation'),('SourceAnchored:with','model')],tense='Present',voice='Active',grammar=('run',))
notice=O(T,'notice',"you'll probably notice them all",[('Subject','agent'),('DirectObject','issues')],tense='Future',voice='Active',grammar=("you'll",'notice'))
prob=MOD(T,'SourceAnchored','probably',notice)
cond=IMP(T,'if',run,prob)
take=O(T,'take','take screenshots of the harness',[('Subject','agent'),('DirectObject','screenshots'),('SourceAnchored:of','harness')])
view=O(T,'view','view them',[('Subject','agent'),('DirectObject','screenshots')])
actions=AND(T,'and',take,view)
can_actions=MOD(T,'SourceAnchored','can',actions)
like=OPGRAM(T,ATT(T,'Preference','agent','like',actions),tense='Present',voice='Active',grammar=('like',))
optional=IMP(T,'if',like,can_actions)
can_ambiguity={'kind':'Lexical','cue':'can','alternatives':['Capability','Permitted'],'note':'The source does not force ability vs permission/option.'}
add(29,ST('SourceAnchored:Wait',wait),ST('Assertion',askreason),ST('Question',qdrive),ST('Assertion',exist),ST('Assertion',hard),ST('Assertion',cond),ST('Assertion',optional),ambiguities=(can_ambiguity,))

# 30
T=rows[30]['text']
real=O(T,'real','the bottleneck is real',[('Subject','bottleneck')],tense='Present',voice='Active',grammar=('is',))
artifact=O(T,'artifact','an artifact of our workspace',[('Subject','bottleneck'),('SourceAnchored:of','workspace')])
nartifact=NEG(T,'not',artifact)
content=AND(T,',',real,nartifact)
confirm=O(T,'confirms','The literature check confirms the bottleneck is real, not an artifact of our workspace',[('Subject','literature_check'),('Content',content)],tense='Present',voice='Active',grammar=('confirms',))
available=O(T,'available','the rescaled initial data are “available upon request,”',[('Subject','data'),('SourceAnchored:upon','request')],tense='Present',voice='Active',grammar=('are',))
says_base=OPGRAM(T,SA(T,'Assertion','paper','says',available,('speaker',)),tense='Present',voice='Active',grammar=('says',))
says=PHASE(T,'Continue','still',says_base)
record=O(T,['linked','public','code/data','record'],'linked public code/data record',[('Subject','v_record')])
exists=QUANT(T,'Exists','no','v_record',record)
no_record=NEG(T,'no',exists)
found=O(T,'found','I found no linked public code/data record',[('Subject','speaker'),('Content',no_record)],tense='Past',voice='Active',grammar=('found',))
freeze=O(T,'freezing','I’m therefore freezing the early-state branch',[('Subject','speaker'),('DirectObject','branch')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','freezing'))
prepare=O(T,'preparing','preparing a precise acquisition request plus a pre-registered acceptance gate',[('Subject','speaker'),('DirectObject','request_and_gate')],tense='Present',aspect='Progressive',voice='Active',grammar=('preparing',))
actions=AND(T,'and',freeze,prepare)
evidence_bundle=AND(T,[':', 'and'],confirm,says,found)
reason=CAUSE(T,'Motivates','therefore',evidence_bundle,actions)
produce=O(T,['immediately','produces'],'obtaining the data immediately produces a yes/no research decision rather than another open-ended reproduction',[('Subject','obtaining_data'),('DirectObject','yes_no_decision'),('SourceAnchored:rather than','open_ended_reproduction')],tense='Present',voice='Active',grammar=('produces',))
purpose=CAUSE(T,'Purpose','so that',actions,produce)
add(30,ST('Assertion',confirm),ST('Assertion',says),ST('Assertion',found),ST('Assertion',reason),ST('Assertion',purpose))

# 31
T=rows[31]['text']
circuit=O(T,'twelve-mode circuit','twelve-mode circuit',[('Subject','v_circuit')])
exists=QUANT(T,'Exists','no','v_circuit',circuit)
no_circuit=NEG(T,'no',exists)
quoted=QUOTE(T,'“',no_circuit)
narrow=O(T,'narrower','The failure is narrower than “no twelve-mode circuit”',[('Subject','failure'),('SourceAnchored:than',quoted)],tense='Present',voice='Active',grammar=('is',))
test=O(T,'existence test','simple renormalization iteration is not an existence test',[('Subject','iteration'),('PredicateComplement','existence_test')],tense='Present',voice='Active',grammar=('is',))
ntest=NEG(T,'not',test)
attract=O(T,'attracted','it can be attracted to the wrong fixed point',[('Subject','iteration'),('SourceAnchored:to','wrong_fixed_point')],voice='Passive',grammar=('be','attracted'))
possible_attract=MOD(T,'Possible','can',attract)
explain=CAUSE(T,'Explains','because',possible_attract,ntest)
form_base=O(T,'formulating','I’m now formulating the exact Leray-projected Fourier map on the invariant sine-polarization subspace',[('Subject','speaker'),('DirectObject','fourier_map'),('SourceAnchored:on','subspace')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','formulating'))
form=TEMP(T,'At','now',form_base,'now')
search=O(T,'searching','searching its coherence/energy-retention Pareto frontier',[('Subject','speaker'),('DirectObject','frontier')],tense='Present',aspect='Progressive',voice='Active',grammar=(sp(T,'I’m'),sp_in(T,'searching',sp(T,'searching its coherence/energy-retention Pareto frontier')))); search['source_spans']=spans_union(search['source_spans'],sp(T,'I’m')); search['ellipsis_from']='shared progressive auxiliary: I’m'
actions=AND(T,'and',form,search)
coh=O(T,'near','three-stage shape coherence near 1',[('Subject','shape_coherence'),('PredicateComplement','1')])
ret=O(T,'above','retention product above \\(1/2\\)',[('Subject','retention_product'),('PredicateComplement','1/2')])
both=AND(T,'and',coh,ret)
threshold=EQ(T,sp_in(T,'is',sp(T,'The decisive threshold is simultaneous three-stage shape coherence near 1 and retention product above \\(1/2\\)')),'decisive_threshold',both)
add(31,ST('Assertion',narrow),ST('Assertion',ntest),ST('Assertion',explain),ST('Assertion',actions),ST('Assertion',threshold))

# 32
T=rows[32]['text']
make=O(T,'making','making',[('Subject','speaker'),('DirectObject','hardening_change')])
worth=O(T,'worth','One more hardening change is worth making',[('Subject','hardening_change'),('Content',make)],tense='Present',voice='Active',grammar=('is',))
here=O(T,'here','I’m here',[('Subject','speaker')],tense='Present',voice='Active',grammar=('I’m',))
during=TEMP(T,'During','while',worth,here)
enabled=O(T,'enabled','the extension bridge is enabled',[('Subject','bridge')],tense='Present',voice='Passive',grammar=('is','enabled'))
disc=O(T,'disconnected','disconnected',[('Subject','bridge')])
ant=AND(T,'but',enabled,disc)
fallback=O(T,['silently','fall back'],'the adapter should not silently fall back to HTTP replay',[('Subject','adapter'),('SourceAnchored:to','http_replay')])
prohib=MOD(T,'Prohibited','should not',fallback)
cond=IMP(T,'if',ant,prohib)
saw=O(T,'saw','you saw `ThreadNotAccessible`',[('Subject','user'),('DirectObject','thread_error')],tense='Past',voice='Active',grammar=('saw',))
why=CAUSE(T,'Causes',['exactly','why'],fallback,saw)
connected=O(T,'connected','Zen bridge not connected',[('Subject','zen_bridge')])
nconnected=NEG(T,'not',connected)
quote=QUOTE(T,'“',nconnected)
eq=EQ(T,sp_in(T,'is',sp(T,'which is “Zen bridge not connected.”')),'problem',quote)
hides=O(T,'hides','it hides the real problem, which is “Zen bridge not connected.”',[('Subject','fallback'),('DirectObject','problem'),('Content',eq)],tense='Present',voice='Active',grammar=('hides',))
add(32,ST('Assertion',during),ST('Assertion',cond),ST('Assertion',why),ST('Assertion',hides))

# 33
T=rows[33]['text']
fails=O(T,['fails','locally'],'The open-family frame also fails locally',[('Subject','frame')],tense='Present',voice='Active',grammar=('fails',))
cancel=O(T,'cancel','neighboring decompositions can cancel to order \\(1/K\\) at one output',[('Subject','decompositions'),('SourceAnchored:to order','1/K'),('SourceAnchored:at','output')])
cap=MOD(T,'Possible','can',cancel)
crucial=O(T,'crucial','The crucial remaining question is global',[('Subject','remaining_question')],tense='Present',voice='Active',grammar=('is',))
globalq=O(T,'global','The crucial remaining question is global',[('Subject','remaining_question')],tense='Present',voice='Active',grammar=('is',))
creates=O(T,['automatically','creates'],'a real velocity containing those four modes automatically creates many other sums and differences',[('Subject','velocity'),('SourceAnchored:containing','four_modes'),('DirectObject','other_sums_differences')],tense='Present',voice='Active',grammar=('creates',))
explain=CAUSE(T,'Explains','because',creates,globalq)
compute_base=O(T,['computing','exactly'],'I’m now computing that full convolution exactly',[('Subject','speaker'),('DirectObject','convolution')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','computing'))
compute=TEMP(T,'At','now',compute_base,'now')
export=O(T,'exports','the near-collision necessarily exports order-one interaction elsewhere',[('Subject','near_collision'),('DirectObject','order_one_interaction'),('SourceAnchored:elsewhere','elsewhere')],tense='Present',voice='Active',grammar=('exports',))
necessary=MOD(T,'Necessary','necessarily',export)
becomes=O(T,'becomes','it becomes the sought “no cancellation without leakage” pattern in a more general form',[('Subject','near_collision'),('PredicateComplement','pattern'),('SourceAnchored:in','more_general_form')],tense='Present',voice='Active',grammar=('becomes',))
cond=IMP(T,'if',necessary,becomes)
add(33,ST('Assertion',fails),ST('Assertion',cap),ST('Assertion',crucial),ST('Assertion',globalq),ST('Assertion',explain),ST('Assertion',compute),ST('Assertion',cond))

# 34
T=rows[34]['text']
different=O(T,['different','rendering','problem'],'The stage side is a different rendering problem from WEF',[('Subject','stage_side'),('SourceAnchored:from','wef')],tense='Present',voice='Active',grammar=('is',))
exports=O(T,['already','exports'],'Artist’s compositor already exports double-buffered DMA-BUF targets',[('Subject','compositor'),('DirectObject','dma_targets')],tense='Present',voice='Active',grammar=('exports',))
imports=O(T,'imports','its standalone viewer imports them without readback',[('Subject','viewer'),('DirectObject','dma_targets'),('SourceAnchored:without','readback')],tense='Present',voice='Active',grammar=('imports',))
expose=O(T,'expose','GPUI does not obviously expose a stable public “paint this external DMA-BUF” element',[('Subject','gpui'),('DirectObject','external_dma_element')],tense='Present',voice='Active',grammar=('does','expose'))
obvious=MOD(T,'SourceAnchored','obviously',expose)
notobvious=NEG(T,'not',obvious)
check_base=O(T,'checking','I’m checking the pinned GPUI renderer surface now',[('Subject','speaker'),('DirectObject','renderer_surface')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','checking'))
check=TEMP(T,'At','now',check_base,'now')
private=O(T,'private','that API is private',[('Subject','api')],tense='Present',voice='Active',grammar=('is',))
dist=O(T,'distinguish','the implementation plan should distinguish a safe first integration from a zero-copy optimization',[('Subject','plan'),('DirectObject','safe_integration'),('SourceAnchored:from','zero_copy')])
obl=MOD(T,'Obligatory','should',dist)
cond=IMP(T,'if',private,obl)
add(34,ST('Assertion',different),ST('Assertion',exports),ST('Assertion',imports),ST('Assertion',notobvious),ST('Assertion',check),ST('Assertion',cond))

# 35
T=rows[35]['text']
harder=O(T,['substantially','harder'],'The enlarged fixed-distance solve is substantially harder',[('Subject','solve')],tense='Present',voice='Active',grammar=('is',))
degree=O(T,['degrees','freedom'],'28 degrees of freedom',[('Subject','v_degree')])
q28=QUANT(T,'SourceAnchored:28','28','v_degree',degree)
dof=O(T,'has','the tail direction has 28 degrees of freedom',[('Subject','tail_direction'),('Content',q28)],tense='Present',voice='Active',grammar=('has',))
why=CAUSE(T,'Explains','because',dof,harder)
onbase=O(T,'on','It is still on the first continuation point',[('Subject','solve'),('PredicateComplement','first_point')],tense='Present',voice='Active',grammar=('is',))
on=PHASE(T,'Continue','still',onbase)
give=O(T,'giving','I’m giving it one more interval',[('Subject','speaker'),('IndirectObject','solve'),('DirectObject','one_more_interval')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','giving'))
cap=O(T,'cap',"I’ll cap it",[('Subject','speaker'),('DirectObject','solve')],tense='Future',voice='Active',grammar=("I’ll",'cap'))
necessary=MOD(T,'Necessary','necessary',cap)
cond=IMP(T,'if',necessary,cap)
after=TEMP(T,'After','then',cond,give)
strict=O(T,'strict','strict',[('Subject','acceptance_test')])
remain=OPGRAM(T,PHASE(T,'Continue','remains',strict),tense='Present',voice='Active',grammar=('remains',))
resid=O(T,'o(\\delta)',r'the residual is \(o(\delta)\)',[('Subject','residual'),('PredicateComplement','little_o_delta')],tense='Present',voice='Active',grammar=('is',))
ret=O(T,'above','retention above \\(1/2\\)',[('Subject','retention'),('PredicateComplement','1/2')])
irrel=O(T,'irrelevant','retention above \\(1/2\\) is irrelevant',[('Subject','retention_above_half')],tense='Present',voice='Active',grammar=('is',))
nresid=NEG(T,'unless',resid)
unless=IMP(T,'unless',nresid,irrel)
repro=O(T,['actually','reproduces'],'the tail actually reproduces at leading order',[('Subject','tail'),('SourceAnchored:at','leading_order')],tense='Present',voice='Active',grammar=('reproduces',))
means=O(T,'meaning','meaning the tail actually reproduces at leading order',[('Subject','little_o_residual'),('Content',repro)])
add(35,ST('Assertion',why),ST('Assertion',on),ST('Assertion',give),ST('Assertion',after),ST('Assertion',remain),ST('Assertion',unless),ST('Assertion',means))

# 36
T=rows[36]['text']
heavier=O(T,['computationally','heavier'],'The packet run is computationally heavier',[('Subject','packet_run')],tense='Present',voice='Active',grammar=('is',))
resolves_base=O(T,'resolves','it now resolves ordinary vertical Fourier modes rather than the compressed single-carrier lattice',[('Subject','packet_run'),('DirectObject','vertical_modes'),('SourceAnchored:rather than','compressed_lattice')],tense='Present',voice='Active',grammar=('resolves',))
resolves=TEMP(T,'At','now',resolves_base,'now')
why=CAUSE(T,'Explains','because',resolves,heavier)
intentional=O(T,'intentional','that is intentional',[('Subject','resolving_choice')],tense='Present',voice='Active',grammar=('is',))
appear=O(T,'appear','frequencies \\(s-t\\) and \\(2n+s+t\\) must be allowed to appear as genuine dynamics',[('Subject','frequencies'),('SourceAnchored:as','genuine_dynamics')])
allowed=O(T,'allowed','frequencies \\(s-t\\) and \\(2n+s+t\\) must be allowed to appear as genuine dynamics',[('Subject','frequencies'),('Content',appear)],voice='Passive',grammar=('be','allowed'))
must=MOD(T,'Necessary','must',allowed)
prepclean=O(T,'clean','The exact preparation and tests are clean',[('Subject','preparation')],tense='Present',voice='Active',grammar=('are',))
testclean=O(T,'clean','The exact preparation and tests are clean',[('Subject','tests')],tense='Present',voice='Active',grammar=('are',))
clean=AND(T,'and',prepclean,testclean)
wait=O(T,'waiting','I’m waiting for the first nonlinear bandwidth comparison',[('Subject','speaker'),('SourceAnchored:for','comparison')],tense='Present',aspect='Progressive',voice='Active',grammar=('I’m','waiting'))
preserve=O(T,'preserves','longitudinal localization preserves or destroys the coherent margin',[('Subject','localization'),('DirectObject','margin')],tense='Present',voice='Active',grammar=('preserves',))
destroy=O(T,'destroys','destroys the coherent margin',[('Subject','localization'),('DirectObject','margin')],tense='Present',voice='Active',grammar=('destroys',)); destroy['ellipsis_from']='coordination subject: longitudinal localization'
alts=OR(T,'or',preserve,destroy)
whether=IQ(T,'Alternative','whether',alts)
decide=O(T,'deciding','deciding whether longitudinal localization preserves or destroys the coherent margin',[('Subject','speaker'),('Content',whether)])
before=TEMP(T,'Before','before',wait,decide)
intent_reason=CAUSE(T,'Explains','since',must,intentional)
add(36,ST('Assertion',why),ST('Assertion',intentional),ST('Assertion',must),ST('Assertion',intent_reason),ST('Assertion',clean),ST('Assertion',before))

# Remaining samples are specified below in a second generated block.

# serialization/verification will run after all 36 are present.

def span_valid(text,s):
    b=text.encode(); frag=b[s['start']:s['end']].decode('utf-8')
    return frag==s['text']

VALID_ROLES={'Subject','DirectObject','IndirectObject','PredicateComplement','Agent','Patient','Theme','Experiencer','Content','Source','Goal','Recipient','Instrument','Location','Manner','Beneficiary','Stimulus','Topic','Possessor','Attribute','Value'}
VALID_MODES={'Assertion','Question','Command','Request','Suggestion','Promise','Quotation','Mention','SourceAnchored'}

def walk(node,text,errs,path='root',bound=None):
    bound=set() if bound is None else set(bound)
    if not isinstance(node,dict): return
    for s in node.get('source_spans',[]):
        if not span_valid(text,s): errs.append(f'{path}: bad source span {s}')
    for s in node.get('operator_spans',[]):
        if not span_valid(text,s): errs.append(f'{path}: bad operator span {s}')
    typ=node.get('type')
    if typ=='occurrence':
        if not node.get('lexical_anchor'): errs.append(f'{path}: missing lexical anchor')
        for s in node.get('lexical_anchor',[]):
            if not span_valid(text,s): errs.append(f'{path}: bad lexical anchor')
        if 'predicate' in node or 'lemma' in node: errs.append(f'{path}: forbidden invented lexical identity')
        if not node.get('ontology_type'): errs.append(f'{path}: missing ontology type')
        # Full ancestry is checked by the v4 verifier; this builder check only catches accidental empty typing.
        hasgram=node.get('tense_aspect') is not None or node.get('voice') is not None
        if hasgram != bool(node.get('grammatical_spans')): errs.append(f'{path}: grammar/evidence mismatch')
        for r in node.get('roles',[]):
            role=r['role']
            if role.startswith('SourceAnchored:'):
                cue=role.split(':',1)[1]
                if cue not in text: errs.append(f'{path}: source role cue missing {cue!r}')
            elif role not in VALID_ROLES: errs.append(f'{path}: unknown role {role}')
            v=r['value']
            if isinstance(v,str) and v.startswith('v_') and v not in bound: errs.append(f'{path}: unbound role var {v}')
        return
    opgram=node.get('operator_tense_aspect') is not None or node.get('operator_voice') is not None
    opgsp=node.get('operator_grammatical_spans',[])
    if opgram and not opgsp: errs.append(f'{path}: operator grammar without cue')
    if (not opgram) and opgsp: errs.append(f'{path}: operator grammar cue without profile')
    if opgram and typ not in {'modal','capability','phase','attitude','speech_act'}: errs.append(f'{path}: operator grammar unsupported')
    for s in opgsp:
        if not span_valid(text,s): errs.append(f'{path}: bad operator grammar span')
    structural={'negation','modal','capability','attitude','phase','interrogative','speech_act','implication','causal','temporal','conjunction','disjunction','quotation'}
    if typ in structural and not node.get('operator_spans'): errs.append(f'{path}: structural node missing operator cue')
    if typ=='quantified':
        v=node.get('variable'); walk(node['body'],text,errs,path+'.body',bound|{v}); return
    if typ=='equality': return
    if typ=='interrogative':
        v=node.get('variable'); nb=bound|({v} if v else set())
        walk(node['body'],text,errs,path+'.body',nb); return
    if typ in {'negation','modal','capability','attitude','phase','quotation'}:
        walk(node['content'],text,errs,path+'.content',bound); return
    if typ=='speech_act': walk(node['content'],text,errs,path+'.content',bound); return
    if typ in {'implication','causal'}:
        walk(node['left'],text,errs,path+'.left',bound); walk(node['right'],text,errs,path+'.right',bound); return
    if typ in {'conjunction','disjunction'}:
        for j,m in enumerate(node['members']): walk(m,text,errs,f'{path}.m{j}',bound)
        return
    if typ=='temporal': walk(node['subject'],text,errs,path+'.subject',bound); return

def verify_complete():
    errs=[]
    missing=[i for i in range(1,37) if i not in spec]
    if missing: errs.append(f'missing samples {missing}')
    for i,x in spec.items():
        text=rows[i]['text']
        for j,st in enumerate(x['statements']):
            mode=st['mode'].split(':',1)[0]
            if mode not in VALID_MODES: errs.append(f'S{i}.{j}: legacy/unknown mode {st["mode"]}')
            if mode=='SourceAnchored':
                cue=st['mode'].split(':',1)[1] if ':' in st['mode'] else ''
                if not cue or cue not in text: errs.append(f'S{i}.{j}: source-anchored mode missing cue')
            if st['mode']=='Question' and st['expression'].get('type')!='interrogative': errs.append(f'S{i}.{j}: question root is not interrogative')
            walk(st['expression'],text,errs,f'S{i}.{j}')
    return errs

def collect_referents(node,out):
    if not isinstance(node,dict): return
    typ=node.get('type')
    if typ=='occurrence':
        for role in node.get('roles',[]):
            value=role.get('value')
            if isinstance(value,str) and not value.startswith('v_'): out.add(value)
            elif isinstance(value,dict): collect_referents(value,out)
    for key in ('bearer','holder','speaker','left_term','right_term'):
        value=node.get(key)
        if isinstance(value,str) and not value.startswith('v_'): out.add(value)
    for value in node.get('addressees',[]):
        if isinstance(value,str) and not value.startswith('v_'): out.add(value)
    for key,value in node.items():
        if key in {'roles','bearer','holder','speaker','left_term','right_term','addressees','source_spans','operator_spans','operator_grammatical_spans','lexical_anchor','grammatical_spans'}: continue
        if isinstance(value,dict): collect_referents(value,out)
        elif isinstance(value,list):
            for child in value:
                if isinstance(child,dict): collect_referents(child,out)

def referent_inventory(statements):
    refs={'speaker'}
    for statement in statements: collect_referents(statement.get('expression'),refs)
    return {name:concept_symbol(infer_referent_type(name)) for name in sorted(refs)}

def write_outputs():
    errs=verify_complete()
    if errs:
        print('\n'.join(errs[:100])); raise SystemExit(f'{len(errs)} errors')
    out=OUT/'v4_strict_labels.jsonl'
    with out.open('w') as f:
        for i in range(1,37):
            row=rows[i]
            payload={k:row[k] for k in ['sample_index','provider','session_id','source_file','source_line','message_id','run_id','speaker_role','channel','record_id','text']}
            payload.update(spec[i]); payload['referent_types']=referent_inventory(payload['statements']); f.write(json.dumps(payload,ensure_ascii=False,sort_keys=True)+'\n')
    print('wrote',out)

if __name__=='__main__': write_outputs()
