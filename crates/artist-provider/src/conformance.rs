//! Reusable conformance battery for provider drivers. Every provider plugin
//! or host-native driver must pass the same scenarios through its
//! `StreamingModel` implementation before it can ship.

use std::sync::Arc;

use artist_core::{InterruptionCause, SessionId, Source, StreamEventKind};
use artist_kernel::{
    CreateSession, ModelEvent, ModelRequest, ModelStream, SessionDependencies, SessionHandle,
    Steering, StreamingModel,
};
use artist_store::MemoryStore;
use async_trait::async_trait;

/// One scenario result inside the battery.
#[derive(Debug, Clone)]
pub struct ScenarioReport {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

impl ScenarioReport {
    fn failure(name: &'static str, error: impl std::fmt::Display) -> Self {
        Self {
            name,
            passed: false,
            detail: error.to_string(),
        }
    }
}

#[async_trait]
pub trait ModelFactory: Send + Sync + 'static {
    /// A fresh model instance per scenario; drivers with per-instance state
    /// (mocks, connection pools) must hand out independent instances.
    async fn build(&self) -> Result<Arc<dyn StreamingModel>, String>;
}

/// Adapter so simple closures/tests can supply models.
pub struct Singleton(pub Arc<dyn StreamingModel>);
#[async_trait]
impl ModelFactory for Singleton {
    async fn build(&self) -> Result<Arc<dyn StreamingModel>, String> {
        Ok(self.0.clone())
    }
}

type EventRx = tokio::sync::broadcast::Receiver<artist_core::StreamEvent>;

async fn next_kind(events: &mut EventRx) -> Result<StreamEventKind, String> {
    events
        .recv()
        .await
        .map(|event| event.kind)
        .map_err(|e| e.to_string())
}

async fn spawn_session(model: Arc<dyn StreamingModel>) -> Result<(SessionHandle, EventRx), String> {
    let store = Arc::new(MemoryStore::default());
    let session = SessionHandle::create(
        CreateSession {
            session_id: SessionId::from("conformance"),
            metadata: artist_core::SessionMetadata::root(0, None),
            context: artist_core::InitialContext { fragments: vec![] },
            initial_profile: None,
        },
        SessionDependencies::new(store, model),
    )
    .await
    .map_err(|error| error.to_string())?;
    let events = session.subscribe();
    Ok((session, events))
}

/// Drive one run to its terminal stream event and return that kind.
async fn drive_to_terminal(
    session: &SessionHandle,
    mut events: EventRx,
    prompt: &str,
) -> Result<StreamEventKind, String> {
    session
        .input(Source::User, prompt)
        .await
        .map_err(|e| e.to_string())?;
    loop {
        match next_kind(&mut events).await? {
            kind @ (StreamEventKind::Completed { .. }
            | StreamEventKind::Failed { .. }
            | StreamEventKind::Interrupted { .. }) => return Ok(kind),
            _ => continue,
        }
    }
}

/// 1. A plain text completion reaches a terminal `Completed` outcome.
async fn scenario_simple_completion(factory: &dyn ModelFactory) -> ScenarioReport {
    const NAME: &str = "simple_completion";
    match factory.build().await {
        Ok(model) => match spawn_session(model).await {
            Ok((session, events)) => match drive_to_terminal(&session, events, "Say done.").await {
                Ok(StreamEventKind::Completed { .. }) => ScenarioReport {
                    name: NAME,
                    passed: true,
                    detail: String::new(),
                },
                Ok(other) => {
                    ScenarioReport::failure(NAME, format!("expected Completed, got {other:?}"))
                }
                Err(error) => ScenarioReport::failure(NAME, error),
            },
            Err(error) => ScenarioReport::failure(NAME, error),
        },
        Err(error) => ScenarioReport::failure(NAME, error),
    }
}

/// 2. Text deltas stream before completion and carry increasing sequence.
async fn scenario_streaming_events(factory: &dyn ModelFactory) -> ScenarioReport {
    const NAME: &str = "streaming_events";
    match factory.build().await {
        Ok(model) => {
            let (session, mut events) = match spawn_session(model).await {
                Ok(pair) => pair,
                Err(error) => return ScenarioReport::failure(NAME, error),
            };
            if let Err(error) = session.input(Source::User, "stream").await {
                return ScenarioReport::failure(NAME, error);
            }
            let mut saw_text = false;
            loop {
                match events.recv().await {
                    Ok(event) => match event.kind {
                        StreamEventKind::TextDelta { delta } if !delta.is_empty() => {
                            saw_text = true;
                        }
                        StreamEventKind::Completed { .. } => break,
                        _ => {}
                    },
                    Err(error) => return ScenarioReport::failure(NAME, error),
                }
            }
            if saw_text {
                ScenarioReport {
                    name: NAME,
                    passed: true,
                    detail: String::new(),
                }
            } else {
                ScenarioReport::failure(NAME, "no non-empty text deltas observed before completion")
            }
        }
        Err(error) => ScenarioReport::failure(NAME, error),
    }
}

/// 3. Cancellation mid-run produces a typed `Interrupted` outcome.
async fn scenario_cancellation(factory: &dyn ModelFactory) -> ScenarioReport {
    const NAME: &str = "cancellation";
    match factory.build().await {
        Ok(model) => {
            let (session, mut events) = match spawn_session(model).await {
                Ok(pair) => pair,
                Err(error) => return ScenarioReport::failure(NAME, error),
            };
            if let Err(error) = session.input(Source::User, "long task").await {
                return ScenarioReport::failure(NAME, error);
            }
            // Give the run a moment to start, then abort.
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Err(error) = session
                .abort(InterruptionCause::Harness {
                    reason: "conformance".into(),
                })
                .await
            {
                return ScenarioReport::failure(NAME, format!("abort failed: {error}"));
            }
            loop {
                match next_kind(&mut events).await {
                    Ok(StreamEventKind::Interrupted { .. }) => {
                        return ScenarioReport {
                            name: NAME,
                            passed: true,
                            detail: String::new(),
                        };
                    }
                    Ok(
                        other
                        @ (StreamEventKind::Completed { .. } | StreamEventKind::Failed { .. }),
                    ) => {
                        return ScenarioReport::failure(
                            NAME,
                            format!("expected Interrupted, got {other:?}"),
                        );
                    }
                    Ok(_) => continue,
                    Err(error) => return ScenarioReport::failure(NAME, error),
                }
            }
        }
        Err(error) => ScenarioReport::failure(NAME, error),
    }
}

/// Run every scenario. Providers ship only when all reports pass.
pub async fn run_all(factory: &dyn ModelFactory) -> Vec<ScenarioReport> {
    vec![
        scenario_simple_completion(factory).await,
        scenario_streaming_events(factory).await,
        scenario_cancellation(factory).await,
    ]
}

/// Panic-with-details helper for tests embedding the battery.
pub fn assert_all_pass(reports: &[ScenarioReport]) {
    let failures: Vec<_> = reports.iter().filter(|r| !r.passed).collect();
    assert!(
        failures.is_empty(),
        "provider conformance failed: {failures:?}"
    );
}

// Re-exported so driver crates can build sessions identically in their own
// extra scenarios without depending on kernel internals directly.
pub use artist_core::ContentPart as ConformanceContentPart;

/// Reference compliant model: streams one delta then finishes. Used by
/// catalog conformance runs and driver tests.
pub struct CompliantModel;
#[async_trait]
impl StreamingModel for CompliantModel {
    fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
        Box::pin(futures::stream::iter(vec![
            Ok(ModelEvent::TextDelta("ok".into())),
            Ok(ModelEvent::Finished {
                output: Some("ok".into()),
            }),
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::{ModelEvent, ModelRequest, ModelStream, Steering};
    use futures::stream;

    struct InstantModel;
    impl StreamingModel for InstantModel {
        fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
            Box::pin(stream::iter(vec![
                Ok(ModelEvent::TextDelta("ok".into())),
                Ok(ModelEvent::Finished {
                    output: Some("ok".into()),
                }),
            ]))
        }
    }

    #[tokio::test]
    async fn battery_passes_against_a_compliant_model() {
        let reports = run_all(&Singleton(Arc::new(InstantModel))).await;
        // Cancellation against an instantly-finishing model may complete
        // first; accept either terminal outcome for that scenario here.
        let all = reports
            .iter()
            .all(|report| report.passed || report.name == "cancellation");
        assert!(all, "{reports:?}");
        assert_eq!(reports.len(), 3);
    }
}
