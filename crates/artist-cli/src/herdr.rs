use artist_agent::{LifecycleEmitter, LifecycleEvent};
use artist_herdr::{HerdrHandle, HerdrIntegration, TurnActivity};

const AUTH_BLOCK: &str = "authentication required";

#[derive(Clone, Default)]
pub(crate) struct Lifecycle {
    handle: Option<HerdrHandle>,
}

impl Lifecycle {
    pub(crate) fn claim_idle(&self) {
        if let Some(handle) = &self.handle {
            handle.claim_idle();
        }
    }

    pub(crate) fn report_session(&self, session_id: &str) {
        if let Some(handle) = &self.handle {
            handle.report_session(session_id);
        }
    }

    pub(crate) fn set_auth_blocked(&self, blocked: bool) {
        if let Some(handle) = &self.handle {
            handle.set_blocked(AUTH_BLOCK, blocked);
        }
    }

    pub(crate) fn start_turn(&self) -> Turn {
        Turn {
            activity: self.handle.as_ref().map(HerdrHandle::start_turn),
            closed: false,
        }
    }
}

pub(crate) struct Turn {
    activity: Option<TurnActivity>,
    closed: bool,
}

impl Turn {
    pub(crate) fn emitter(&self) -> LifecycleEmitter {
        let Some(activity) = self.activity.clone() else {
            return LifecycleEmitter::default();
        };
        LifecycleEmitter::new(move |event| match event {
            LifecycleEvent::ToolStarted(id) => activity.tool_started(id),
            LifecycleEvent::ToolFinished(id) => activity.tool_finished(&id),
        })
    }

    pub(crate) fn finish(mut self, cancelled: bool) {
        self.close(cancelled);
    }

    fn close(&mut self, cancelled: bool) {
        if self.closed {
            return;
        }
        if let Some(activity) = &self.activity {
            if cancelled {
                activity.cancel();
            } else {
                activity.finish();
            }
        }
        self.closed = true;
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.close(true);
    }
}

pub(crate) struct Runtime(Option<HerdrIntegration>);

impl Runtime {
    pub(crate) fn detect() -> Self {
        Self(HerdrIntegration::detect())
    }

    pub(crate) fn lifecycle(&self) -> Lifecycle {
        Lifecycle {
            handle: self.0.as_ref().map(HerdrIntegration::handle),
        }
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(integration) = self.0.take() {
            integration.shutdown().await;
        }
    }
}
