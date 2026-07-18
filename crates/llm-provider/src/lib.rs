//! Saved provider configuration and authentication primitives.
//!
//! Secrets are serializable because callers need to persist them, but are always
//! redacted from `Debug`. Persist [`SavedProvider`] values in an OS keychain or an
//! equivalently protected encrypted store, never in plaintext configuration.

mod chatgpt;
mod error;
mod provider;
mod secret;

pub use chatgpt::{
    CHATGPT_CODEX_BASE_URL, CODEX_CLIENT_ID, ChatGptOAuth, LoginRequest, PendingLogin,
    RefreshOutcome,
};
pub use error::{Error, Result};
pub use provider::{Auth, ProviderId, ProviderKind, RequestAuth, SavedProvider};
pub use secret::Secret;
