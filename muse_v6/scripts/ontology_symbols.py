#!/usr/bin/env python3
"""Canonical namespaceless learned-label ontology symbols.

Authoritative package IDs remain qualified internally. Learned labels use local
names except for the small, explicit semantic rename table below. Any future
collision not covered here is a hard error.
"""
from __future__ import annotations
from collections import defaultdict

CONCEPT_RENAMES = {
    'ufo:Delegation': 'DelegationRelation',
    'agent:Delegation': 'DelegationAction',
    'mlt:Individual': 'MLTIndividual',
    'ufo:Individual': 'Individual',
    'mlt:Type': 'MLTType',
    'ufo:Type': 'Type',
}
RELATION_RENAMES = {
    'mlt:instantiates': 'mltInstantiates',
    'ufo:instantiates': 'instantiates',
    'mlt:specializes': 'mltSpecializes',
    'ufo:specializes': 'specializes',
    'service:fulfillsCommitment': 'serviceDeliveryFulfillsCommitment',
    'ufo:fulfillsCommitment': 'fulfillsCommitment',
    'service:violatesCommitment': 'serviceDeliveryViolatesCommitment',
    'ufo:violatesCommitment': 'violatesCommitment',
}

def _canonical(value: str, overrides: dict[str, str]) -> str:
    return overrides.get(value, value.split(':', 1)[-1])

def concept_symbol(value: str) -> str:
    return _canonical(value, CONCEPT_RENAMES)

def relation_symbol(value: str) -> str:
    return _canonical(value, RELATION_RENAMES)

def build_symbol_maps(concepts: set[str], relations: set[str]):
    concept_map = {concept_symbol(value): value for value in concepts}
    relation_map = {relation_symbol(value): value for value in relations}
    concept_groups = defaultdict(list)
    relation_groups = defaultdict(list)
    for value in concepts:
        concept_groups[concept_symbol(value)].append(value)
    for value in relations:
        relation_groups[relation_symbol(value)].append(value)
    concept_collisions = {k: sorted(v) for k, v in concept_groups.items() if len(v) > 1}
    relation_collisions = {k: sorted(v) for k, v in relation_groups.items() if len(v) > 1}
    cross_collisions = sorted(set(concept_map) & set(relation_map))
    if concept_collisions or relation_collisions or cross_collisions:
        raise ValueError(f'unresolved canonical ontology collisions: concepts={concept_collisions}, relations={relation_collisions}, cross={cross_collisions}')
    if any(':' in value for value in concept_map | relation_map):
        raise ValueError('canonical learned ontology symbols must be namespaceless')
    return concept_map, relation_map
