use artist_agent::SteeringHandle;
use artist_extensions::{ControlFuture, HostControl};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Default)]
pub struct ExtensionControl {
    steering: Arc<Mutex<Option<SteeringHandle>>>,
    prompts: Arc<Mutex<VecDeque<String>>>,
    stop: Arc<AtomicBool>,
}

impl ExtensionControl {
    pub fn set_steering(&self, steering: Option<SteeringHandle>) {
        *self
            .steering
            .lock()
            .expect("extension steering lock poisoned") = steering;
    }

    pub fn take_prompts(&self) -> Vec<String> {
        self.prompts
            .lock()
            .expect("extension prompt lock poisoned")
            .drain(..)
            .collect()
    }

    pub fn take_stop(&self) -> bool {
        self.stop.swap(false, Ordering::AcqRel)
    }
}

impl HostControl for ExtensionControl {
    fn steer(&self, message: String) -> ControlFuture<'_> {
        Box::pin(async move {
            if let Some(handle) = self
                .steering
                .lock()
                .expect("extension steering lock poisoned")
                .clone()
            {
                handle.enqueue(message);
            }
        })
    }

    fn prompt_after(&self, message: String) -> ControlFuture<'_> {
        Box::pin(async move {
            self.prompts
                .lock()
                .expect("extension prompt lock poisoned")
                .push_back(message);
        })
    }

    fn stop(&self) -> ControlFuture<'_> {
        Box::pin(async move { self.stop.store(true, Ordering::Release) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queues_followups_and_stop_requests() {
        let control = ExtensionControl::default();
        control.prompt_after("next".into()).await;
        control.stop().await;
        assert_eq!(control.take_prompts(), ["next"]);
        assert!(control.take_stop());
        assert!(!control.take_stop());
    }
}
