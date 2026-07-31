use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    ToolStarted(String),
    ToolFinished(String),
    SubagentStarted(String),
    SubagentFinished(String),
}

#[derive(Clone, Default)]
pub struct LifecycleEmitter {
    callback: Option<Arc<dyn Fn(LifecycleEvent) + Send + Sync>>,
}

impl LifecycleEmitter {
    pub fn new(callback: impl Fn(LifecycleEvent) + Send + Sync + 'static) -> Self {
        Self {
            callback: Some(Arc::new(callback)),
        }
    }

    pub(crate) fn emit(&self, event: LifecycleEvent) {
        if let Some(callback) = &self.callback {
            callback(event);
        }
    }
}
