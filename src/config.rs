//! Configuration from environment variables.
//!
//! Mirrors yt-mcp patterns: frozen config, multi-instance, read-only mode.
//!
//! Env vars:
//!   GITLAB_URL          — default instance URL (e.g., https://gitlab.com)
//!   GITLAB_TOKEN        — default instance token
//!   GITLAB_INSTANCES    — comma-separated instance names for multi-instance
//!   GITLAB_{NAME}_URL   — per-instance URL
//!   GITLAB_{NAME}_TOKEN — per-instance token
//!   GITLAB_READ_ONLY    — disable write tools (true/1/yes)
//!   DISABLED_TOOLS      — comma-separated tool names to disable
//!   GITLAB_COMPACT      — strip markdown for token savings (true/1/yes)

use std::collections::HashMap;
use crate::error::{Error, Result};
use std::env;

/// Single GitLab instance configuration (immutable after creation).
#[derive(Debug, Clone)]
pub struct GitLabInstance {
    pub name: String,
    pub url: String,
    pub token: String,
}

/// Application-wide configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub instances: Vec<GitLabInstance>,
    pub read_only: bool,
    pub disabled_tools: Vec<String>,
    pub compact: bool,
}

fn is_truthy(val: &str) -> bool {
    matches!(val.to_lowercase().as_str(), "true" | "1" | "yes")
}

impl Config {
    /// Load configuration from environment variables.
    pub fn from_env() -> Result<Self> {
        let read_only = env::var("GITLAB_READ_ONLY")
            .map(|v| is_truthy(&v))
            .unwrap_or(false);

        let compact = env::var("GITLAB_COMPACT")
            .map(|v| is_truthy(&v))
            .unwrap_or(false);

        let disabled_tools = env::var("DISABLED_TOOLS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_lowercase().replace('-', "_"))
            .filter(|s| !s.is_empty())
            .collect();

        // Multi-instance support
        let instances = if let Ok(names) = env::var("GITLAB_INSTANCES") {
            let mut result = Vec::new();
            for name in names.split(',').map(|s| s.trim()) {
                if name.is_empty() {
                    continue;
                }
                let upper = name.to_uppercase();
                let url = env::var(format!("GITLAB_{upper}_URL"))
                    .map_err(|_| Error::Config(format!("GITLAB_{upper}_URL not set")))?;
                let token = env::var(format!("GITLAB_{upper}_TOKEN"))
                    .map_err(|_| Error::Config(format!("GITLAB_{upper}_TOKEN not set")))?;

                validate_url(&url)?;
                result.push(GitLabInstance {
                    name: name.to_string(),
                    url: url.trim_end_matches('/').to_string(),
                    token,
                });
            }
            if result.is_empty() {
                return Err(Error::Config("GITLAB_INSTANCES is set but no valid instances configured".into()));
            }
            result
        } else {
            // Single instance mode
            let url = env::var("GITLAB_URL")
                .map_err(|_| Error::Config("GITLAB_URL not set".into()))?;
            let token = env::var("GITLAB_TOKEN")
                .map_err(|_| Error::Config("GITLAB_TOKEN not set".into()))?;

            validate_url(&url)?;
            vec![GitLabInstance {
                name: "default".to_string(),
                url: url.trim_end_matches('/').to_string(),
                token,
            }]
        };

        Ok(Config {
            instances,
            read_only,
            disabled_tools,
            compact,
        })
    }

    /// Build a domain → instance name map for URL-based resolution.
    pub fn domain_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for inst in &self.instances {
            if let Ok(url) = url::Url::parse(&inst.url) {
                if let Some(host) = url.host_str() {
                    map.insert(host.to_string(), inst.name.clone());
                }
            }
        }
        map
    }
}

fn validate_url(url: &str) -> Result<()> {
    if url.starts_with("https://") {
        return Ok(());
    }
    if url.starts_with("http://localhost") || url.starts_with("http://127.0.0.1") {
        return Ok(());
    }
    if env::var("GITLAB_ALLOW_HTTP").map(|v| is_truthy(&v)).unwrap_or(false) {
        return Ok(());
    }
    Err(Error::Config(format!(
        "URL must use HTTPS: {url}. Set GITLAB_ALLOW_HTTP=1 to allow HTTP."
    )))
}
