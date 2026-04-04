//! Unified error type for gl-mcp.

use reqwest::StatusCode;

/// All errors in gl-mcp flow through this type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitLab API error ({status}): {message}")]
    GitLab {
        status: StatusCode,
        message: String,
    },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

/// Shorthand Result type.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Truncated display for analytics logging (UTF-8 safe).
    pub fn short_message(&self) -> String {
        let msg = self.to_string();
        if msg.len() > 200 {
            let truncated: String = msg.chars().take(200).collect();
            format!("{truncated}...")
        } else {
            msg
        }
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Error::Config(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }
}
