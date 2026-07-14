#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid provider configuration: {0}")]
    InvalidConfig(String),
    #[error("login state did not match; the callback may belong to another login")]
    StateMismatch,
    #[error("OAuth response did not include {0}")]
    MissingToken(&'static str),
    #[error("invalid identity token: {0}")]
    InvalidIdentityToken(String),
    #[error("authentication request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid URL: {0}")]
    Url(#[from] url::ParseError),
}

pub type Result<T> = std::result::Result<T, Error>;
