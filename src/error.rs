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

    /// Expected user-input/validation error (bad username, unknown role,
    /// mismatched confirmation, no-op update, …). Reported to the caller but
    /// never alerted on — classified by type, not by message wording.
    #[error("{0}")]
    UserInput(String),

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

    pub fn user_input(msg: impl Into<String>) -> Self {
        Error::UserInput(msg.into())
    }

    /// True for errors caused by the caller's input rather than a defect:
    /// validation failures, not-found lookups, and GitLab 4xx responses
    /// (except 408 timeout and 429 rate-limit, which are environmental).
    /// Used to keep expected errors out of Sentry by construction.
    pub fn is_user_error(&self) -> bool {
        match self {
            Error::UserInput(_) | Error::NotFound(_) => true,
            Error::GitLab { status, .. } => {
                status.is_client_error()
                    && *status != StatusCode::REQUEST_TIMEOUT
                    && *status != StatusCode::TOO_MANY_REQUESTS
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gitlab(code: u16) -> Error {
        Error::GitLab {
            status: StatusCode::from_u16(code).unwrap(),
            message: "m".into(),
        }
    }

    #[test]
    fn user_error_classification() {
        // Typed user errors.
        assert!(Error::UserInput("bad role".into()).is_user_error());
        assert!(Error::NotFound("user".into()).is_user_error());
        // GitLab 4xx are user errors — except the environmental pair.
        assert!(gitlab(404).is_user_error());
        assert!(gitlab(403).is_user_error());
        assert!(!gitlab(408).is_user_error());
        assert!(!gitlab(429).is_user_error());
        // Server-side and internal failures are real errors.
        assert!(!gitlab(500).is_user_error());
        assert!(!Error::Other("boom".into()).is_user_error());
        assert!(!Error::Config("bad env".into()).is_user_error());
    }
}
