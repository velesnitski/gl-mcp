//! Who may reach the HTTP transport.
//!
//! Every tool runs with the server's GitLab token, write tools included, so an HTTP
//! listener is as powerful as that token. The rules:
//!
//! - Bind loopback unless told otherwise.
//! - Off loopback, a bearer token is mandatory: the server refuses to start without
//!   one rather than serve the network unauthenticated.
//! - On loopback, rmcp's Host check (loopback names only) stops DNS rebinding; off
//!   loopback the token does that job, and `GL_MCP_ALLOWED_HOSTS` can narrow Host
//!   further.
//!
//! Pure decisions live here so they are tested without opening a socket.

use std::net::IpAddr;

/// Env var holding the bearer token clients must send.
pub const TOKEN_ENV: &str = "GL_MCP_HTTP_TOKEN";
/// Env var with comma-separated Host values to accept when bound off loopback.
pub const ALLOWED_HOSTS_ENV: &str = "GL_MCP_ALLOWED_HOSTS";

/// What the HTTP transport will enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpPolicy {
    pub bind: IpAddr,
    /// Required bearer token; `None` only on loopback.
    pub token: Option<String>,
    /// `None`: rmcp's default loopback Host list. `Some(empty)`: no Host check.
    pub allowed_hosts: Option<Vec<String>>,
}

/// Decide the policy from the `--host` value and the environment, or explain why the
/// server must not start.
pub fn policy(host: &str, token: Option<&str>, allowed_hosts: Option<&str>) -> Result<HttpPolicy, String> {
    let bind: IpAddr = host
        .parse()
        .map_err(|_| format!("--host '{host}' is not an IP address (use 127.0.0.1, 0.0.0.0, ::1 …)"))?;
    let token = token.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    if token.as_ref().is_some_and(|t| t.len() < 24) {
        return Err(format!("{TOKEN_ENV} is too short: use at least 24 random characters."));
    }
    if bind.is_loopback() {
        return Ok(HttpPolicy { bind, token, allowed_hosts: None });
    }
    if token.is_none() {
        return Err(format!(
            "refusing to serve {bind} without authentication: every tool runs with the GitLab token. \
             Set {TOKEN_ENV} (clients send `Authorization: Bearer <token>`), or bind 127.0.0.1."
        ));
    }
    let hosts = allowed_hosts
        .map(|h| h.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect())
        .unwrap_or_default();
    Ok(HttpPolicy { bind, token, allowed_hosts: Some(hosts) })
}

/// Does this `Authorization` header carry the token? Compared in constant time, so
/// response timing does not reveal how much of a guess was right.
pub fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(presented) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (presented.trim().as_bytes(), token.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    // Assembled at runtime: no token-shaped literal sits in the source.
    fn token() -> String {
        ["t", &"x".repeat(30)].concat()
    }

    #[test]
    fn loopback_is_the_default_and_needs_no_token() {
        let p = policy("127.0.0.1", None, None).unwrap();
        assert!(p.bind.is_loopback());
        assert_eq!(p.token, None);
        assert_eq!(p.allowed_hosts, None, "rmcp's loopback Host list applies");
        assert!(policy("::1", None, None).is_ok());
    }

    #[test]
    fn off_loopback_without_a_token_refuses_to_start() {
        let err = policy("0.0.0.0", None, None).unwrap_err();
        assert!(err.contains(TOKEN_ENV));
        assert!(policy("0.0.0.0", Some("   "), None).is_err(), "blank is no token");
        assert!(policy("10.0.0.5", None, None).is_err());
    }

    #[test]
    fn off_loopback_with_a_token_serves_and_hosts_can_be_narrowed() {
        let t = token();
        let p = policy("0.0.0.0", Some(&t), None).unwrap();
        assert_eq!(p.token.as_deref(), Some(t.as_str()));
        assert_eq!(p.allowed_hosts, Some(vec![]), "the token guards; no Host list by default");
        let p = policy("0.0.0.0", Some(&t), Some("mcp.example.com, mcp.example.com:8000,")).unwrap();
        assert_eq!(p.allowed_hosts, Some(vec!["mcp.example.com".into(), "mcp.example.com:8000".into()]));
    }

    #[test]
    fn weak_tokens_and_bad_hosts_are_rejected() {
        assert!(policy("0.0.0.0", Some("short"), None).unwrap_err().contains("too short"));
        assert!(policy("localhost", None, None).unwrap_err().contains("not an IP"));
    }

    #[test]
    fn only_the_exact_bearer_token_is_authorized() {
        let t = token();
        assert!(authorized(Some(&format!("Bearer {t}")), &t));
        assert!(!authorized(None, &t));
        assert!(!authorized(Some(&t), &t), "the Bearer scheme is required");
        assert!(!authorized(Some(&format!("Basic {t}")), &t));
        assert!(!authorized(Some(&format!("Bearer {t}x")), &t));
        assert!(!authorized(Some(&format!("Bearer {}", &t[1..])), &t));
        assert!(!authorized(Some("Bearer "), &t));
    }
}
