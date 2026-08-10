#!/usr/bin/env python3
"""Toolchain-independent source-shape checks for semantic-label v5."""
from __future__ import annotations
import re, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
errors=[]
def need(text, marker, where):
    if marker not in text: errors.append(f'{where}: missing {marker}')
def no(text, marker, where):
    if marker in text: errors.append(f'{where}: forbidden marker {marker}')
occ=(ROOT/'crates/muse-occurrence/src/lib.rs').read_text()
canon=(ROOT/'crates/muse-occurrence/src/canonical.rs').read_text()
training=(ROOT/'crates/muse-training/src/lib.rs').read_text()
lowering=(ROOT/'crates/muse-superstrate/src/lib.rs').read_text()
gen=(ROOT/'scripts/generate_foundation_packages.py').read_text()
spec=(ROOT/'docs/SEMANTIC_LABEL_SPEC.md').read_text()

for marker in [
 'pub const SCHEMA_VERSION: &str = "muse-occurrence-5"','pub enum PresentationMode','Record,',
 'Counterfactual {','Exactly(Literal)','pub enum ParticipantRole','pub struct LexicalAnchor',
 'pub enum DiscourseRole','Tool,',"if !field_path.is_empty() && !field_path.starts_with('/') && !field_path.starts_with('@')",
]: need(occ,marker,'occurrence')
for marker in ['pub enum OccurrenceKind','pub kind: OccurrenceKind','pub kind_spans:','LexicalPredicate']:
    no(occ,marker,'occurrence')
for marker in ['muse-occurrence-training-canonical-6','UnusedInterrogativeVariable','PropositionExpr::Counterfactual']:
    need(canon,marker,'canonical')

for marker in [
 'muse-semantic-label-5','muse-corpus-3','muse-label-conformance-5','ToolCallUpdate','StructuredChannel::ToolCallUpdate',
 'pub enum SpeakerRole { User, Agent, System, Harness, Tool, Unknown }','StructuredFieldValue::Opaque','alias_paths','container_paths',
 'StructuredContextField','build_training_windows','muse_window_actor','muse_window_recorder','@tool_name','@invocation_id','@record',
 'validate_structured_semantic_shell','concept_set_has_subtype','relation_is_subrelation','index.is_subtype',
 'NonRecordModeInStructuredWindow','RecordModeInProse','MissingReferentOntologyType','MissingOccurrenceOntologyType',
 'QualifiedOntologyConceptInLearnedLabel','QualifiedOntologyRelationInLearnedLabel','LabelBounce','BounceReason',
 'source_start_byte','source_end_byte','InvalidStructuredAlias','UnknownOpaqueField',
]: need(training,marker,'training')
for forbidden in [
 'ty.as_str()=="ToolInvocation"','ty.as_str()=="ToolResult"','ty.as_str()=="ToolInvocationUpdate"',
 'validate_prose_v4','muse-prose-label-4.1','muse-corpus-2','muse-label-conformance-4.1'
]: no(training,forbidden,'training')

for marker in [
 'muse-superstrate-lowering-7','lower_and_kernel_check','resolve_ontology_symbols','index.resolve_concept','index.resolve_relation',
 'Term::Proposition(id) => document.propositions.contains_key(id).then(||','ConceptId::from("ufo:Proposition")',
 'typed_instance','check_relation_constraints','muse.counterfactual','muse.interrogative','muse.operator_grounding',
]: need(lowering,marker,'superstrate')
for marker in ['muse.occurrence.kind/','check_occurrence_kind_type']:
    no(lowering,marker,'superstrate')

for marker in [
 'agent:QuestionPrompt','agent:ChoiceOption','agent:ToolInvocationUpdate','agent:MessageDelivery',
 'agent:questionPromptHasOption','agent:choiceOptionExpressesProposition','agent:toolInvocationUpdateStatus',
 'agent:toolInvocationUpdateCarriesResult','agent:deliversMessage','agent:toolResultOutcome',
]: need(gen,marker,'foundation generator')

for marker in [
 'Status: normative for `muse-semantic-label-5`','model-visible','ontology-ancestry checks','cumulative UI snapshots',
 '`toolResultOutcome','status` is provider/tool-schema dependent','BOUNCE: FORMALISM','ACCEPT WITH AMBIGUITY',
]: need(spec,marker,'semantic spec')

# Every explicit production Occurrence literal must carry one ontology type field and no legacy kind.
for path in [ROOT/'crates/muse-tooling/src/lib.rs',ROOT/'crates/muse-artist-adapter/src/lib.rs',ROOT/'crates/muse-training/src/lib.rs']:
    text=path.read_text().split('#[cfg(test)]',1)[0]
    for m in re.finditer(r'Occurrence\s*\{',text):
        i=m.end(); depth=1
        while i<len(text) and depth:
            if text[i]=='{': depth+=1
            elif text[i]=='}': depth-=1
            i+=1
        block=text[m.start():i]
        if 'id:' not in block: continue
        if not re.search(r'\btypes(?:\s*:|\s*,)',block): errors.append(f'{path.relative_to(ROOT)}: Occurrence literal missing ontology types')
        if re.search(r'\bkind\s*:',block): errors.append(f'{path.relative_to(ROOT)}: Occurrence literal contains removed kind')

if errors:
    print('semantic-v5 source verification FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print('semantic-v5 source verification passed')
