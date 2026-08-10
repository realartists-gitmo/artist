#!/usr/bin/env python3
"""Toolchain-independent source-shape checks for semantic-label v6.

This is intentionally strict about the model-facing boundary and the deterministic
safety path. It does not substitute for Rust compilation.
"""
from __future__ import annotations
import re, sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
errors=[]
def need(text, marker, where):
    if marker not in text: errors.append(f'{where}: missing {marker}')
def no(text, marker, where):
    if marker in text: errors.append(f'{where}: forbidden marker {marker}')

def strip_rust(text:str)->str:
    out=list(text); i=0; state='code'; depth=0
    while i<len(text):
        c=text[i]; n=text[i+1] if i+1<len(text) else ''
        if state=='code':
            if c=='/' and n=='/': out[i]=out[i+1]=' '; i+=2; state='line'; continue
            if c=='/' and n=='*': out[i]=out[i+1]=' '; i+=2; state='block'; depth=1; continue
            if c=='"': out[i]=' '; i+=1; state='str'; continue
            if c=="'" and i+2<len(text):
                # char literal only; lifetimes are left intact
                j=i+1
                if text[j]=='\\': j+=2
                else: j+=1
                if j<len(text) and text[j]=="'":
                    for k in range(i,j+1): out[k]=' '
                    i=j+1; continue
            i+=1
        elif state=='line':
            if c=='\n': state='code'
            else: out[i]=' '
            i+=1
        elif state=='block':
            out[i]=' '
            if c=='/' and n=='*': out[i+1]=' '; depth+=1; i+=2
            elif c=='*' and n=='/': out[i+1]=' '; depth-=1; i+=2; state='code' if depth==0 else 'block'
            else: i+=1
        else:
            out[i]=' '
            if c=='\\' and i+1<len(text): out[i+1]=' '; i+=2
            elif c=='"': state='code'; i+=1
            else: i+=1
    return ''.join(out)

def duplicate_struct_fields(path:Path, struct_names:set[str]):
    raw=path.read_text(); clean=strip_rust(raw)
    pat=re.compile(r'\b('+'|'.join(map(re.escape,sorted(struct_names,key=len,reverse=True)))+r')\s*\{')
    for m in pat.finditer(clean):
        name=m.group(1); start=m.end(); depth=1; i=start
        while i<len(clean) and depth:
            if clean[i]=='{': depth+=1
            elif clean[i]=='}': depth-=1
            i+=1
        if depth: continue
        body=clean[start:i-1]
        fields=[]; j=0; local_depth=0
        while j<len(body):
            c=body[j]
            if c in '({[': local_depth+=1; j+=1; continue
            if c in ')}]': local_depth=max(0,local_depth-1); j+=1; continue
            if local_depth==0:
                fm=re.match(r'\s*([A-Za-z_][A-Za-z0-9_]*)\s*:(?!:)',body[j:])
                if fm:
                    fields.append(fm.group(1)); j+=fm.end(); continue
            j+=1
        dup=sorted({x for x in fields if fields.count(x)>1})
        if dup: errors.append(f'{path.relative_to(ROOT)}: duplicate fields in {name} literal: {dup}')

learn=(ROOT/'crates/muse-training/src/semantic_v6.rs').read_text()
compile=(ROOT/'crates/muse-training/src/v6_compile.rs').read_text()
training=(ROOT/'crates/muse-training/src/lib.rs').read_text()
occ=(ROOT/'crates/muse-occurrence/src/lib.rs').read_text()
lowering=(ROOT/'crates/muse-superstrate/src/lib.rs').read_text()
tooling=(ROOT/'crates/muse-tooling/src/lib.rs').read_text()
artist=(ROOT/'crates/muse-artist-adapter/src/lib.rs').read_text()
event=(ROOT/'crates/muse-artist-adapter/src/event_formalizer.rs').read_text()
gen=(ROOT/'scripts/generate_foundation_packages.py').read_text()
cargo=(ROOT/'Cargo.toml').read_text()

for marker in [
    'pub struct LearnedSemanticTarget','pub struct LearnedReferent','pub sort: ConceptId',
    'pub struct LearnedOccurrence','pub struct LearnedVariable','pub enum ContextTerm',
    'pub enum LearnedArithmeticOperator','pub enum LearnedLiteral','pub enum LearnedPropositionExpr','pub enum LearnedContentBody',
    'pub struct LearnedAmbiguity','pub enum LearnedRoot','pub enum LearnedReading',
    'Counterfactual','Unless','GeneralizedQuantified','ScopedOperator','Focus','Presuppositional',
    'Ratio','Percentage','Approximate','Interval','PluralScale','Arithmetic','Measurement',
]: need(learn,marker,'learned-v6')
for marker in ['Possible { content: PropositionId }','Necessary { content: PropositionId }','Generic { content: PropositionId }','Perfect { content: PropositionId }','Progressive { content: PropositionId }']:
    no(learn,marker,'learned-v6')

for marker in ['SourceSpanId','pub spans: BTreeMap<SourceSpanId']:
    no(learn,marker,'learned-v6 span grounding')

for marker in [
    'pub schema_version:','pub ontology:','pub derivation:','pub evidence:','pub external_ids:','pub discourse_roles:',
    'ParticipantRole','PresentationMode','StatementBasis','SourceAnchored','Json {','pub types:',
    'pub anchor: LearnedAnchor,\n    pub body: LearnedContentBody',
]: no(learn,marker,'learned-v6')
# Question must be content, never a proposition variant.
prop_section=learn.split('pub enum LearnedPropositionExpr',1)[1].split('pub struct LearnedProposition',1)[0]
if re.search(r'\bQuestion\s*\{',prop_section): errors.append('learned-v6: Question appears inside proposition grammar')
arith_section=learn.split('pub enum LearnedArithmeticOperator',1)[1].split('pub enum LearnedLiteral',1)[0]
if 'Divide' in arith_section: errors.append('learned-v6: Divide remains in learned arithmetic; numeric / must be Ratio')
need(cargo,'features = ["arbitrary_precision"]','workspace serde_json')

for marker in [
    'validate_compiled_v6_contract','validate_source_coverage','compile_ambiguity',
    'StructuredSourceValue::String','questionAnswerVariable','toolResultOfInvocation',
    'toolInvocationUpdateFor','toolInvocationPrincipal','invokesTool',
    'alternatives do not cover the same source footprint','semantic objects unreachable from any root',
    'arguments.len() != 2','operator cue overlaps scoped semantic material','contentPresenter','contentAddressee',
]: need(compile,marker,'v6-compile')
for marker in ['LearnedPropositionExpr::Interrogative','LearnedPropositionExpr::Attitude','LearnedPropositionExpr::SpeechAct']:
    no(compile,marker,'v6-compile')
for marker in ['PropositionExpr::Modal { .. }','PropositionExpr::Generic { .. }','PropositionExpr::Perfect { .. }','PropositionExpr::Progressive { .. }']:
    need(compile,marker,'v6-compatibility-firewall')

for marker in [
    'pub const CORPUS_SCHEMA_VERSION: &str = "muse-corpus-5"',
    'pub enum LearnedVisibility {\n    ModelVisible,\n}','pub source_adapter: String','pub source_adapter_version: String',
    'pub aliases: Vec<FieldAliasRule>','pub opaque_fields: Vec<OpaqueFieldRule>','pub context_dependencies: Vec<ContextDependencyRule>',
    'OpaqueEncoding::Base64','duplicate JSON object key','normalization_contracts: BTreeMap',
    'exact_json_numbers_do_not_round_through_f64',
]: need(training,marker,'training-normalization')

for marker in [
    'muse.occurrence.instance/','muse.tense/{tense_name}','muse.quantifier/{kind}','muse.generalized_quantifier','muse.scoped_operator/',
    'muse.generic','muse.focus','muse.aspect/perfect','muse.aspect/progressive','muse.presuppositional',
    'muse.literal/ratio','muse.literal/percentage','muse.literal/approximate','muse.literal/interval','muse.literal/measurement',
]: need(lowering,marker,'superstrate')
for marker in [
    'muse.occurrence.lexical_anchor/','muse.occurrence.type/','muse.occurrence.grounding/',
]: no(lowering,marker,'superstrate')

for marker in [
    'pub const TOOL_SCHEMA_VERSION: &str = "muse-tool-record-3"','pub invocation_sort: ConceptId',
    'agent:toolInvocationRequestedEffect','agent:toolInvocationObservedEffect','agent:toolResultOutcome',
    'InconsistentEffectStateEvidence',
]: need(tooling,marker,'tooling')
for marker in ['pub enum PrincipalKind','pub enum ToolFamily','pub enum ArtifactKind','pub enum EffectKind','pub enum ArtifactStateRole','ParticipantRole']:
    no(tooling,marker,'tooling')

for marker in ['pub struct ArtistStructuredEvent','fn is_known_event_kind','invocation_sort: tool_invocation_sort(&name)']:
    need(artist,marker,'artist-adapter')
for marker in ['pub enum ArtistEventClass','pub class: ArtistEventClass']:
    no(artist,marker,'artist-adapter')
if 'let mut record_types' in event: errors.append('artist-event-formalizer: multi-type record construction remains')
if 'type_statement(' in event: errors.append('artist-event-formalizer: redundant explicit type assertion remains')

for marker in [
    'agent:toolInvocationRequestedEffect','agent:toolInvocationObservedEffect','agent:toolResultOutcome',
    'sem:questionAnswerVariable','sem:PropositionalOperator','sem:ModalOperator','sem:CapabilityOperator','sem:OptimizationOperator','sem:MaximizationOperator','sem:MinimizationOperator','sem:optimizationTarget','sem:ExclusiveFocusOperator','sem:agentParticipant','sem:surfaceSubjectParticipant','sem:explains','comp:NumericValue','comp:RatioValue','comp:PercentageValue',
    'comp:ApproximateNumericValue','comp:NumericInterval','comp:MeasurementValue',
]: need(gen,marker,'foundation-generator')

# Target/safety-path struct literals must not contain duplicate named fields.
structs={'Referent','Occurrence','SemanticVariable','Proposition','Statement','ToolInvocationRecord','Principal','ToolDescriptor','ArtifactDescriptor','ArtifactStateDescriptor','ToolEffect','NormalizationContract'}
for path in [ROOT/'crates/muse-training/src/v6_compile.rs',ROOT/'crates/muse-tooling/src/lib.rs',ROOT/'crates/muse-artist-adapter/src/lib.rs',ROOT/'crates/muse-artist-adapter/src/event_formalizer.rs']:
    duplicate_struct_fields(path,structs)

if errors:
    print('semantic-v6 source verification FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print('semantic-v6 source verification passed')
