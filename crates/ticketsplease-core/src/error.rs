//! Error type and the stable process exit-code contract (R12).

/// Convenience alias used throughout the core crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Core error type. Each variant maps to a stable process exit code via
/// [`Error::exit_code`] — exit codes are part of the CLI's public contract.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Malformed input: invalid ticket, bad config, failed validation.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A referenced ticket does not exist.
    #[error("ticket not found: {0}")]
    NotFound(String),
    /// The dependency graph contains a cycle.
    #[error("dependency cycle: {0}")]
    Cycle(String),
    /// The guard found a scope under-declaration or collision.
    #[error("conflict: {0}")]
    Conflict(String),
    /// A `watch` gave up before the ticket reached its target status.
    #[error("timed out: {0}")]
    Timeout(String),
    /// An unexpected internal failure.
    #[error("internal error: {0}")]
    Internal(String),
    /// Underlying I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The process exit code for this error — the stable CLI API.
    ///
    /// `0` ok · `1` internal · `2` usage (clap) · `3` invalid/dirty ·
    /// `4` not found · `5` cycle · `6` conflict · `7` watch timeout.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Invalid(_) => 3,
            Error::NotFound(_) => 4,
            Error::Cycle(_) => 5,
            Error::Conflict(_) => 6,
            Error::Timeout(_) => 7,
            Error::Internal(_) => 1,
            Error::Io(_) => 1,
        }
    }

    /// A stable machine-readable kind for the JSON error envelope.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid",
            Error::NotFound(_) => "not-found",
            Error::Cycle(_) => "cycle",
            Error::Conflict(_) => "conflict",
            Error::Timeout(_) => "timeout",
            Error::Internal(_) => "internal",
            Error::Io(_) => "io",
        }
    }

    /// The inner message without the `Display` kind prefix — for adding context
    /// (e.g. a file path) without double-prefixing `invalid input: ... invalid input: ...`.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Error::Invalid(s)
            | Error::NotFound(s)
            | Error::Cycle(s)
            | Error::Conflict(s)
            | Error::Timeout(s)
            | Error::Internal(s) => s.clone(),
            Error::Io(e) => e.to_string(),
        }
    }
}
