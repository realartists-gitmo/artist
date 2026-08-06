mod command;
mod context;
mod integration;
#[cfg(test)]
mod integration_tests;
mod state;

pub use context::{AGENT, HerdrContext, LIFECYCLE_SOURCE};
pub use integration::{HerdrHandle, HerdrIntegration};
pub use state::{Activity, TurnActivity};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HerdrState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

impl HerdrState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}
