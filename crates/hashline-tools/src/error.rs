/// Error codes relevant to file/anchor operations (shell-specific codes stripped).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashlineErrorCode {
    StaleAnchor,
    ContentChanged,
    AmbiguousAnchor,
    ConfirmationRequired,
    PathOutsideWorkspace,
    AlreadyExists,
    InvalidInput,
    Io,
    Internal,
}

#[derive(Debug, Clone)]
pub struct HashlineError {
    pub code: HashlineErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl HashlineError {
    pub fn new(code: HashlineErrorCode, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(HashlineErrorCode::InvalidInput, message, false)
    }

    /// Best-effort classification from a free-form message (handy when wrapping
    /// `anyhow` errors from the manager layer).
    pub fn from_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let lower = message.to_ascii_lowercase();
        let (code, retryable) = if lower.contains("content hash mismatch") {
            (HashlineErrorCode::ContentChanged, true)
        } else if lower.contains("confirmation_required")
            || lower.contains("stale hash")
            || lower.contains("does not match any line")
            || lower.contains("stale anchor")
        {
            (HashlineErrorCode::StaleAnchor, true)
        } else if lower.contains("ambiguous") {
            (HashlineErrorCode::AmbiguousAnchor, true)
        } else if lower.contains("outside workspace") || lower.contains("workspace root") {
            (HashlineErrorCode::PathOutsideWorkspace, false)
        } else if lower.contains("already exists") {
            (HashlineErrorCode::AlreadyExists, false)
        } else if lower.contains("failed to read")
            || lower.contains("failed to write")
            || lower.contains("no such file")
        {
            (HashlineErrorCode::Io, true)
        } else if lower.contains("must") || lower.contains("cannot") || lower.contains("invalid") {
            (HashlineErrorCode::InvalidInput, false)
        } else {
            (HashlineErrorCode::Internal, false)
        };
        Self::new(code, message, retryable)
    }
}

impl std::fmt::Display for HashlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HashlineError {}
