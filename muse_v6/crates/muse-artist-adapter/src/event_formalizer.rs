#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use muse_core::{
    ConceptId, OccurrenceDocumentId, PropositionId, ReferentId, RelationId, SourceSpanId,
    StatementId,
};
use muse_occurrence::{
    Derivation, DiscourseRole, Literal, OccurrenceDocument, PresentationMode, Proposition,
    PropositionExpr, Referent, SCHEMA_VERSION, SourceSpan, Statement, StatementBasis, Term,
};
use muse_registry::RegistrySnapshot;
use serde_json::Value;

use crate::{ARTIST_EVENT_FORMALIZER_VERSION, ArtistAdapterError, ArtistStructuredEvent};

/// Deterministic total formalizer for known Artist event envelopes.
#[derive(Clone, Debug)]
pub struct ArtistEventFormalizer {
    pub ontology: RegistrySnapshot,
}

impl ArtistEventFormalizer {
    pub fn formalize(
        &self,
        event: &ArtistStructuredEvent,
    ) -> Result<OccurrenceDocument, ArtistAdapterError> {
        let seed = format!("{}:{}", event.session, event.sequence);
        let span_id = SourceSpanId::from(format!("artist-event-span:{seed}"));
        let recorder_id = ReferentId::from(format!("artist-event-recorder:{}", event.session));
        let session_id = ReferentId::from(format!("artist-session:{}", event.session));
        let lineage_id = ReferentId::from(format!(
            "artist-lineage:{}:{}",
            event.session, event.lineage
        ));
        let record_id = ReferentId::from(format!("artist-event-record:{seed}"));
        let run_id = event.run.as_ref().map(|run| {
            ReferentId::from(format!(
                "artist-run:{}:{}:{run}",
                event.session, event.lineage
            ))
        });

        let mut document = OccurrenceDocument {
            schema_version: SCHEMA_VERSION.into(),
            id: OccurrenceDocumentId::from(format!("artist-event-document:{seed}")),
            ontology: self.ontology.clone(),
            derivation: Derivation::DeterministicStructured {
                adapter: "artist-event".into(),
                version: ARTIST_EVENT_FORMALIZER_VERSION.into(),
            },
            source_spans: BTreeMap::from([(
                span_id.clone(),
                SourceSpan {
                    id: span_id.clone(),
                    source: event.source.source.clone(),
                    run: event.run.clone(),
                    turn: None,
                    message: Some(event.source.record.clone()),
                    block: None,
                    bytes: None,
                    // RFC-6901's root pointer is the empty string. Use a
                    // non-root, valid pointer to identify the event payload.
                    field_path: Some("/payload".into()),
                },
            )]),
            referents: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            variables: BTreeMap::new(),
            propositions: BTreeMap::new(),
            statements: BTreeMap::new(),
            ambiguities: BTreeMap::new(),
            statement_order: Vec::new(),
        };

        document.referents.insert(
            recorder_id.clone(),
            referent(
                recorder_id.clone(),
                "artist:ArtistHarness",
                [DiscourseRole::Harness],
                &span_id,
                [("artist_recorder", "artist-session-recorder")],
            ),
        );
        document.referents.insert(
            session_id.clone(),
            referent(
                session_id.clone(),
                "artist:ArtistSession",
                [DiscourseRole::External],
                &span_id,
                [("artist_session", event.session.as_str())],
            ),
        );
        document.referents.insert(
            lineage_id.clone(),
            referent(
                lineage_id.clone(),
                "artist:ArtistLineage",
                [DiscourseRole::External],
                &span_id,
                [("artist_lineage", event.lineage.as_str())],
            ),
        );
        let record_sort = event_record_type(event).unwrap_or("artist:ArtistSessionEventRecord");
        document.referents.insert(
            record_id.clone(),
            Referent {
                id: record_id.clone(),
                types: BTreeSet::from([ConceptId::from(record_sort)]),
                lexical_anchor: None,
                labels: BTreeSet::from([event.kind.clone()]),
                external_ids: BTreeMap::from([(
                    "artist_event_sequence".into(),
                    event.sequence.to_string(),
                )]),
                discourse_roles: BTreeSet::from([DiscourseRole::External]),
                source_spans: BTreeSet::from([span_id.clone()]),
            },
        );
        if let Some(run_id) = &run_id {
            document.referents.insert(
                run_id.clone(),
                referent(
                    run_id.clone(),
                    "harness:CodingRun",
                    [DiscourseRole::External],
                    &span_id,
                    [("artist_run", event.run.as_deref().unwrap_or(""))],
                ),
            );
        }

        let mut ordinal = 0usize;
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordSession",
            vec![
                Term::Referent(record_id.clone()),
                Term::Referent(session_id.clone()),
            ],
            &span_id,
        );
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordLineage",
            vec![
                Term::Referent(record_id.clone()),
                Term::Referent(lineage_id.clone()),
            ],
            &span_id,
        );
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:sessionLineage",
            vec![Term::Referent(session_id), Term::Referent(lineage_id)],
            &span_id,
        );
        if let Some(run_id) = run_id {
            relation_statement(
                &mut document,
                &mut ordinal,
                &recorder_id,
                "harness:recordBelongsToRun",
                vec![Term::Referent(record_id.clone()), Term::Referent(run_id)],
                &span_id,
            );
        }
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordKind",
            vec![
                Term::Referent(record_id.clone()),
                Term::Literal(Literal::String(event.kind.clone())),
            ],
            &span_id,
        );
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordPayload",
            vec![
                Term::Referent(record_id.clone()),
                Term::Literal(Literal::Json(canonical_json(&event.payload))),
            ],
            &span_id,
        );
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordSequence",
            vec![
                Term::Referent(record_id.clone()),
                Term::Literal(Literal::Integer(event.sequence.to_string())),
            ],
            &span_id,
        );
        relation_statement(
            &mut document,
            &mut ordinal,
            &recorder_id,
            "artist:eventRecordTimestampMillis",
            vec![
                Term::Referent(record_id.clone()),
                Term::Literal(Literal::Integer(event.timestamp_millis.to_string())),
            ],
            &span_id,
        );
        add_event_specific_semantics(
            &mut document,
            &mut ordinal,
            &recorder_id,
            &record_id,
            event,
            &span_id,
        );

        document.validate()?;
        Ok(document)
    }
}

fn referent<const R: usize, const E: usize>(
    id: ReferentId,
    sort: &str,
    roles: [DiscourseRole; R],
    span: &SourceSpanId,
    external: [(&str, &str); E],
) -> Referent {
    Referent {
        id,
        types: BTreeSet::from([ConceptId::from(sort)]),
        lexical_anchor: None,
        labels: BTreeSet::new(),
        external_ids: external
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        discourse_roles: roles.into_iter().collect(),
        source_spans: BTreeSet::from([span.clone()]),
    }
}

fn relation_statement(
    document: &mut OccurrenceDocument,
    ordinal: &mut usize,
    presenter: &ReferentId,
    relation: &str,
    arguments: Vec<Term>,
    span: &SourceSpanId,
) {
    add_statement(
        document,
        ordinal,
        presenter,
        PropositionExpr::Relation {
            relation: RelationId::from(relation),
            arguments,
        },
        span,
    );
}

fn add_statement(
    document: &mut OccurrenceDocument,
    ordinal: &mut usize,
    presenter: &ReferentId,
    expression: PropositionExpr,
    span: &SourceSpanId,
) {
    let proposition_id = PropositionId::from(format!("artist-event-proposition:{ordinal}"));
    let statement_id = StatementId::from(format!("artist-event-statement:{ordinal}"));
    *ordinal += 1;
    document.propositions.insert(
        proposition_id.clone(),
        Proposition {
            id: proposition_id.clone(),
            expression,
            operator_spans: BTreeSet::new(),
            operator_tense_aspect: None,
            operator_voice: None,
            operator_grammatical_spans: BTreeSet::new(),
            source_spans: BTreeSet::from([span.clone()]),
            evidence: BTreeSet::new(),
        },
    );
    document.statements.insert(
        statement_id.clone(),
        Statement {
            id: statement_id.clone(),
            presenter: Some(presenter.clone()),
            addressees: BTreeSet::new(),
            mode: PresentationMode::Assertion,
            basis: StatementBasis::Expressed,
            content: proposition_id,
            source_spans: BTreeSet::from([span.clone()]),
            evidence: BTreeSet::new(),
        },
    );
    document.statement_order.push(statement_id);
}

fn add_event_specific_semantics(
    document: &mut OccurrenceDocument,
    ordinal: &mut usize,
    presenter: &ReferentId,
    record: &ReferentId,
    event: &ArtistStructuredEvent,
    span: &SourceSpanId,
) {
    match event.kind.as_str() {
        "task.started" | "task.updated" | "task.finished" => {
            let Some(task) = event.payload.get("task").and_then(Value::as_str) else {
                return;
            };
            let task_id = ReferentId::from(format!(
                "artist-task:{}:{}:{}",
                event.session, event.lineage, task
            ));
            document
                .referents
                .entry(task_id.clone())
                .or_insert_with(|| {
                    referent(
                        task_id.clone(),
                        "artist:ArtistTask",
                        [DiscourseRole::External],
                        span,
                        [("artist_task", task)],
                    )
                });
            relation_statement(
                document,
                ordinal,
                presenter,
                "artist:taskRecordTask",
                vec![Term::Referent(record.clone()), Term::Referent(task_id)],
                span,
            );
            match event.kind.as_str() {
                "task.started" => {
                    if let Some(command) = event.payload.get("command").and_then(Value::as_str) {
                        literal_relation(
                            document,
                            ordinal,
                            presenter,
                            record,
                            "artist:taskCommand",
                            Literal::String(command.to_owned()),
                            span,
                        );
                    }
                    if let Some(persistent) =
                        event.payload.get("persistent").and_then(Value::as_bool)
                    {
                        literal_relation(
                            document,
                            ordinal,
                            presenter,
                            record,
                            "artist:taskPersistent",
                            Literal::Boolean(persistent),
                            span,
                        );
                    }
                }
                "task.updated" => {
                    if let Some(output) = event.payload.get("output").and_then(Value::as_str) {
                        literal_relation(
                            document,
                            ordinal,
                            presenter,
                            record,
                            "artist:taskOutput",
                            Literal::String(output.to_owned()),
                            span,
                        );
                    }
                }
                "task.finished" => {
                    if let Some(exit_code) = event.payload.get("exit_code").and_then(Value::as_i64)
                    {
                        literal_relation(
                            document,
                            ordinal,
                            presenter,
                            record,
                            "artist:taskExitCode",
                            Literal::Integer(exit_code.to_string()),
                            span,
                        );
                    }
                    if let Some(interrupted) =
                        event.payload.get("interrupted").and_then(Value::as_bool)
                    {
                        literal_relation(
                            document,
                            ordinal,
                            presenter,
                            record,
                            "artist:taskInterrupted",
                            Literal::Boolean(interrupted),
                            span,
                        );
                    }
                }
                _ => {}
            }
        }
        "computer.acted" => {
            let Some(steps) = event.payload.get("steps").and_then(Value::as_array) else {
                return;
            };
            for (index, step) in steps.iter().enumerate() {
                let step_id =
                    ReferentId::from(format!("artist-computer-step:{}:{}", event.sequence, index));
                document.referents.insert(
                    step_id.clone(),
                    Referent {
                        id: step_id.clone(),
                        types: BTreeSet::from([ConceptId::from("artist:ArtistComputerStep")]),
                        lexical_anchor: None,
                        labels: BTreeSet::new(),
                        external_ids: BTreeMap::from([(
                            "artist_computer_step".into(),
                            index.to_string(),
                        )]),
                        discourse_roles: BTreeSet::from([DiscourseRole::External]),
                        source_spans: BTreeSet::from([span.clone()]),
                    },
                );
                relation_statement(
                    document,
                    ordinal,
                    presenter,
                    "artist:computerActionStep",
                    vec![
                        Term::Referent(record.clone()),
                        Term::Referent(step_id.clone()),
                    ],
                    span,
                );
                literal_relation(
                    document,
                    ordinal,
                    presenter,
                    &step_id,
                    "artist:computerStepIndex",
                    Literal::Integer(index.to_string()),
                    span,
                );
                if let Some(action) = step.get("action").and_then(Value::as_str) {
                    literal_relation(
                        document,
                        ordinal,
                        presenter,
                        &step_id,
                        "artist:computerStepAction",
                        Literal::String(action.to_owned()),
                        span,
                    );
                }
                if let Some(label) = step.get("label").and_then(Value::as_str) {
                    literal_relation(
                        document,
                        ordinal,
                        presenter,
                        &step_id,
                        "artist:computerClaimedTargetLabel",
                        Literal::String(label.to_owned()),
                        span,
                    );
                }
                if let Some(resolved) = step.get("resolved_name").and_then(Value::as_str) {
                    literal_relation(
                        document,
                        ordinal,
                        presenter,
                        &step_id,
                        "artist:computerResolvedTargetName",
                        Literal::String(resolved.to_owned()),
                        span,
                    );
                }
                if let Some(payload) = step.get("payload").and_then(Value::as_str) {
                    literal_relation(
                        document,
                        ordinal,
                        presenter,
                        &step_id,
                        "artist:computerStepPayload",
                        Literal::String(payload.to_owned()),
                        span,
                    );
                }
                if let Some(outcome) = step.get("outcome").and_then(Value::as_str) {
                    literal_relation(
                        document,
                        ordinal,
                        presenter,
                        &step_id,
                        "artist:computerStepOutcome",
                        Literal::String(outcome.to_owned()),
                        span,
                    );
                }
            }
        }
        _ => {}
    }
}

fn literal_relation(
    document: &mut OccurrenceDocument,
    ordinal: &mut usize,
    presenter: &ReferentId,
    subject: &ReferentId,
    relation: &str,
    value: Literal,
    span: &SourceSpanId,
) {
    relation_statement(
        document,
        ordinal,
        presenter,
        relation,
        vec![Term::Referent(subject.clone()), Term::Literal(value)],
        span,
    );
}

fn event_record_type(event: &ArtistStructuredEvent) -> Option<&'static str> {
    Some(match event.kind.as_str() {
        "model.turn" => "artist:ArtistModelTurnRecord",
        "tool.result" => "artist:ArtistToolResultRecord",
        "tool.result.images" => "artist:ArtistToolResultImagesRecord",
        "change.recorded" => "artist:ArtistChangeRecord",
        "task.started" | "task.updated" | "task.finished" => "artist:ArtistTaskRecord",
        "rule.fired" => "artist:ArtistRuleFiringRecord",
        "handoff.performed" => "artist:ArtistHandoffRecord",
        "computer.stage_opened" | "computer.stage_closed" => "artist:ArtistComputerStageRecord",
        "computer.launched" => "artist:ArtistComputerLaunchRecord",
        "computer.observed" => "artist:ArtistComputerObservationRecord",
        "computer.acted" => "artist:ArtistComputerActionRecord",
        "computer.elided" => "artist:ArtistComputerElisionRecord",
        _ => return None,
    })
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into()),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            let body = entries
                .into_iter()
                .map(|(key, value)| {
                    let key = serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into());
                    format!("{key}:{}", canonical_json(value))
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use muse_core::{ContentDigest, PackageId, PackageRef, PackageVersion, canonical_digest};

    fn snapshot() -> RegistrySnapshot {
        let package = PackageRef {
            id: PackageId::new("muse.artist"),
            version: PackageVersion::new("1.0.0"),
            digest: ContentDigest::sha256_bytes(b"test"),
        };
        let packages = BTreeSet::from([package]);
        RegistrySnapshot {
            identity: canonical_digest(&packages).unwrap(),
            packages,
        }
    }

    #[test]
    fn every_known_event_kind_has_a_total_classification() {
        for kind in crate::ARTIST_KNOWN_EVENT_KINDS {
            assert!(crate::is_known_event_kind(kind), "{kind}");
        }
    }

    #[test]
    fn provider_private_payload_stays_exact_json() {
        let event = ArtistStructuredEvent {
            source: crate::source_for(
                &crate::ArtistEnvelope {
                    v: 1,
                    seq: 1,
                    ts: 2,
                    session: "s".into(),
                    run: Some("r".into()),
                    lineage: "main".into(),
                    kind: "provider.context.v1".into(),
                    payload: serde_json::json!({"opaque":{"z":1,"a":2}}),
                },
                None,
            )
            .unwrap(),
            session: "s".into(),
            lineage: "main".into(),
            run: Some("r".into()),
            kind: "provider.context.v1".into(),
            sequence: 1,
            timestamp_millis: 2,
            payload: serde_json::json!({"opaque":{"z":1,"a":2}}),
        };
        let document = ArtistEventFormalizer {
            ontology: snapshot(),
        }
        .formalize(&event)
        .unwrap();
        assert!(document.propositions.values().any(|proposition| matches!(
            &proposition.expression,
            PropositionExpr::Relation { relation, arguments }
                if relation.as_str() == "artist:eventRecordPayload"
                    && matches!(arguments.get(1), Some(Term::Literal(Literal::Json(value))) if value.contains("\"a\":2"))
        )));
    }
}
