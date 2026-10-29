//! Unified error type for gl-mcp.

use reqwest::StatusCode;

/// All errors in gl-mcp flow through this type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GitLab API error ({status}): {message}")]
    GitLab { status: StatusCode, message: String },

    /// The root cause leads: reqwest's own text ("error sending request for url
    /// (…)") says nothing about why — DNS, refused, timeout, TLS — and analytics
    /// truncate long messages, so the cause must come first to survive.
    #[error("HTTP error: {}", http_message(.0))]
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

fn http_message(e: &reqwest::Error) -> String {
    let mut causes = Vec::new();
    let mut src = std::error::Error::source(e);
    while let Some(c) = src {
        causes.push(c.to_string());
        src = c.source();
    }
    chain_message(&e.to_string(), &causes)
}

/// "{deepest cause} — {top}"; repeated links (hyper often wraps one error twice)
/// are dropped. Pure, for testing without a network.
pub(crate) fn chain_message(top: &str, causes: &[String]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for c in causes.iter().rev() {
        let c = c.trim();
        if !c.is_empty() && !seen.iter().any(|s| s.contains(c) || c.contains(*s)) && !top.contains(c) {
            seen.push(c);
        }
    }
    match seen.first() {
        Some(root) => format!("{root} — {top}"),
        None => top.to_string(),
    }
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

/// Best-effort fallback for optional data, without hiding the failure.
///
/// Reports enrich their output from many secondary API calls; one failing should not
/// fail the whole report. `unwrap_or_default()` achieved that by discarding the error,
/// so an outage and a genuinely empty result rendered identically. This keeps the
/// fallback but logs where it happened: 4xx at debug (a missing README is expected),
/// anything else at warn.
pub trait ResultExt<T> {
    fn or_default_logged(self) -> T;
}

impl<T: Default> ResultExt<T> for Result<T> {
    #[track_caller]
    fn or_default_logged(self) -> T {
        self.unwrap_or_else(|e| {
            let at = std::panic::Location::caller();
            if e.is_user_error() {
                tracing::debug!(%at, "optional fetch failed, using empty value: {e}");
            } else {
                tracing::warn!(%at, "optional fetch failed, using empty value: {e}");
            }
            T::default()
        })
    }
}

/// Outcome tracker for a fan-out's primary lookups.
///
/// Fan-out tools (team activity, org dashboards, group sweeps) used to drop each
/// failed lookup and report on what was left. With GitLab down nothing was left, and
/// the tool answered with a clean empty result — "no users", a table of zeros, an
/// all-clear audit — indistinguishable from a genuinely quiet team.
///
/// A lookup that fails with a caller-side error (404, 403, unknown user) is an honest
/// "not there" and counts as answered. Anything else — connection refused, 5xx — is a
/// failure: if every lookup failed, the tool fails; if some did, it says how many.
#[derive(Default)]
pub struct FanOut {
    answered: usize,
    failed: Vec<Error>,
}

impl FanOut {
    /// Record one lookup; the value when it succeeded.
    pub fn take<T>(&mut self, result: Result<T>) -> Option<T> {
        match result {
            Ok(v) => {
                self.answered += 1;
                Some(v)
            }
            Err(e) if e.is_user_error() => {
                self.answered += 1;
                None
            }
            Err(e) => {
                self.failed.push(e);
                None
            }
        }
    }

    /// Merge the outcomes of a fan-out run in another task.
    pub fn absorb(&mut self, other: FanOut) {
        self.answered += other.answered;
        self.failed.extend(other.failed);
    }

    /// `Err` when every lookup failed; otherwise a warning line when some did.
    pub fn finish(self, what: &str) -> Result<Option<String>> {
        let failed = self.failed.len();
        let total = failed + self.answered;
        match self.failed.into_iter().next() {
            None => Ok(None),
            Some(first) if self.answered == 0 => Err(Error::other(format!(
                "all {total} {what} failed — GitLab did not answer ({first}). No result is shown rather than an empty one."
            ))),
            Some(first) => Ok(Some(format!(
                "⚠️ {failed} of {total} {what} failed ({first}) — the result below is partial.\n"
            ))),
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

    #[test]
    fn fan_out_fails_only_when_nothing_answered() {
        let refused = || Error::other("connection refused");
        let mut all_down = FanOut::default();
        assert!(all_down.take::<u8>(Err(refused())).is_none());
        assert!(all_down.take::<u8>(Err(refused())).is_none());
        let err = all_down.finish("user lookups").unwrap_err();
        assert!(err.to_string().contains("all 2 user lookups failed"));
        assert!(!err.is_user_error(), "an outage is a real error, not the caller's");

        let mut partial = FanOut::default();
        assert_eq!(partial.take(Ok(7)), Some(7));
        partial.take::<u8>(Err(refused()));
        let warning = partial.finish("group lookups").unwrap().unwrap();
        assert!(warning.contains("1 of 2 group lookups failed"));

        let mut merged = FanOut::default();
        let mut other = FanOut::default();
        other.take::<u8>(Err(refused()));
        merged.absorb(other);
        assert!(merged.finish("x").is_err(), "absorbed failures count");

        let mut answered = FanOut::default();
        let mut other = FanOut::default();
        other.take(Ok::<u8, Error>(1));
        other.take(Ok::<u8, Error>(2));
        answered.absorb(other);
        answered.take::<u8>(Err(refused()));
        let w = answered.finish("x").unwrap().expect("partial");
        assert!(w.contains("1 of 3 x failed"), "absorbed successes count too: {w}");

        // "Not found" is an answer, not an outage.
        let mut missing = FanOut::default();
        missing.take::<u8>(Err(gitlab(404)));
        assert_eq!(missing.finish("x").unwrap(), None);
        assert_eq!(FanOut::default().finish("x").unwrap(), None, "no lookups, nothing to report");
    }

    #[test]
    fn or_default_logged_keeps_values_and_defaults_errors() {
        assert_eq!(Ok::<_, Error>(vec![1]).or_default_logged(), vec![1]);
        assert_eq!(
            Err::<Vec<u8>, _>(gitlab(500)).or_default_logged(),
            Vec::<u8>::new()
        );
        assert_eq!(Err::<String, _>(gitlab(404)).or_default_logged(), "");
    }
}

#[cfg(test)]
mod chain_tests {
    use super::chain_message;

    #[test]
    fn root_cause_leads_and_repeats_collapse() {
        let top = "error sending request for url (https://gitlab.example.com/api/v4/version)";
        let causes = vec![
            "client error (Connect)".to_string(),
            "dns error".to_string(),
            "dns error: failed to lookup address information".to_string(),
        ];
        assert_eq!(
            chain_message(top, &causes),
            format!("dns error: failed to lookup address information — {top}")
        );
        assert_eq!(chain_message(top, &[]), top);
    }
}

