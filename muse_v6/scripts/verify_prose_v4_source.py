#!/usr/bin/env python3
"""HISTORICAL prose-v4 source-shape checker. Not an active readiness gate under semantic-v6."""
from __future__ import annotations
import re, sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
errors=[]
def need(text, marker, where):
    if marker not in text: errors.append(f"{where}: missing {marker}")
def no(text, marker, where):
    if marker in text: errors.append(f"{where}: forbidden legacy marker {marker}")
occ=(ROOT/'crates/muse-occurrence/src/lib.rs').read_text()
canon=(ROOT/'crates/muse-occurrence/src/canonical.rs').read_text()
training=(ROOT/'crates/muse-training/src/lib.rs').read_text()
lowering=(ROOT/'crates/muse-superstrate/src/lib.rs').read_text()
for marker in [
    'pub const SCHEMA_VERSION: &str = "muse-occurrence-4"','pub struct LexicalAnchor','pub enum ParticipantRole',
    'Agent, Patient, Theme, Experiencer','Knowledge, Recollection','pub enum Quantifier','Exactly(Literal)','AtLeast(Literal)','AtMost(Literal)','MoreThan(Literal)','FewerThan(Literal)',
    'Counterfactual {','pub enum InterrogativeKind','pub enum PhaseKind','Purpose,','pub enum GrammaticalTense','pub enum GrammaticalAspect','pub enum GrammaticalVoice',
    'pub operator_spans:','pub operator_tense_aspect:','pub operator_voice:','pub operator_grammatical_spans:','InvalidCardinalityLiteral',
]: need(occ,marker,'occurrence')
no(occ,'LexicalPredicate','occurrence'); no(canon,'LexicalPredicate','canonical')
no(occ,'pub enum OccurrenceKind','occurrence'); no(occ,'pub kind: OccurrenceKind','occurrence'); no(occ,'pub kind_spans:','occurrence')
no(lowering,'muse.occurrence.kind/','superstrate'); no(lowering,'check_occurrence_kind_type','superstrate')
for marker in ['muse-occurrence-training-canonical-5','UnusedInterrogativeVariable','Quantifier::Exactly(value)','PropositionExpr::Counterfactual','kind":"counterfactual"','proposition.operator_spans = map_set']:
    need(canon,marker,'canonical')
for marker in [
    'muse-prose-label-4.1','muse-label-conformance-4.1','validate_prose_v4','MissingReferentOntologyType','NonMinimalReferentOntologyTyping','MissingOccurrenceOntologyType','NonMinimalOccurrenceOntologyTyping',
    'MissingVariableOntologyDomain','RoleAnchorOverlapsLexicalPredicate','QuestionWithoutInterrogative','FactiveLeak','OperatorRequiresEventuality','UngroundedReferent','PropositionExpr::Counterfactual',
    'QualifiedOntologyConceptInProse','QualifiedOntologyRelationInProse','require_namespaceless_concept','require_namespaceless_relation',
]: need(training,marker,'training')
for forbidden in ['HiddenOntologyTypingInProse','SemanticRoleForbiddenInProse','validate_prose_v3']:
    no(training,forbidden,'training')
for marker in [
    'muse-superstrate-lowering-6','MissingOntologyType','MissingVariableType','InvalidOccurrenceOntologyType','check_occurrence_type','typed_instance','variable_object','exact_type_for_concepts',
    'muse.instance/','muse.counterfactual','muse.quantified/','muse.interrogative/','muse.occurrence.participant/','participant_role_class','ontology/{}','muse.occurrence.attribute/{relation_class}/','check_relation_constraints(document, &arguments, declaration, index)?','muse.operator_grounding','muse.operator_grammar','lower_and_kernel_check',
    'resolve_ontology_symbols','index.resolve_concept','index.resolve_relation','validate_resolved_ontology_references',
]: need(lowering,marker,'superstrate')
# Semantic occurrence targets in temporal/causal structure must be the typed occurrence object, not a proposition wrapper.
semantic_target = lowering.split('fn semantic_target_typed', 1)[1].split('fn temporal_anchor_typed', 1)[0]
need(semantic_target, 'self.occurrence_objects.get(id)', 'superstrate semantic target')
no(semantic_target, 'occurrence_proposition', 'superstrate semantic target')
# Ensure formal object roots are typed instances rather than direct quote-only identities.
for marker in ['builder.typed_instance(&format!("referent/','builder.typed_instance(&format!("occurrence/']:
    need(lowering,marker,'superstrate')
# Every explicit Occurrence literal in non-test production code must mention v3 structural fields.
for path in [ROOT/'crates/muse-tooling/src/lib.rs',ROOT/'crates/muse-artist-adapter/src/lib.rs',ROOT/'crates/muse-training/src/lib.rs']:
    text=path.read_text().split('#[cfg(test)]',1)[0]
    for m in re.finditer(r'Occurrence\s*\{', text):
        i=m.end(); depth=1
        while i<len(text) and depth:
            if text[i]=='{': depth+=1
            elif text[i]=='}': depth-=1
            i+=1
        block=text[m.start():i]
        if 'id:' not in block: continue
        for field in ['lexical_anchor:','tense_aspect:','grammatical_voice:','grammatical_spans:']:
            if field not in block: errors.append(f'{path.relative_to(ROOT)}: Occurrence literal missing {field}')
        if not re.search(r'\btypes(?:\s*:|\s*,)',block): errors.append(f'{path.relative_to(ROOT)}: Occurrence literal missing ontology types field')
if errors:
    print('prose-v4 source verification FAILED')
    for e in errors: print('ERROR:',e)
    sys.exit(1)
print('prose-v4 source verification passed')
