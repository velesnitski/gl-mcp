//! Structured logging and analytics.
//!
//! Mirrors yt-mcp patterns:
//! - Error logging to stderr (JSON)
//! - Analytics logging to ~/.gl-mcp/analytics.log
//! - Persistent instance_id
//! - Safe param extraction (whitelist)

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;
use uuid::Uuid;

/// Sentry guard — must be held alive for the lifetime of the process.
static SENTRY_GUARD: OnceLock<Option<sentry::ClientInitGuard>> = OnceLock::new();

/// Persistent instance ID (8-char UUID).
static INSTANCE_ID: OnceLock<String> = OnceLock::new();

/// Directory for gl-mcp data.
fn data_dir() -> PathBuf {
    let home = dirs_next().unwrap_or_else(|| PathBuf::from("."));
    home.join(".gl-mcp")
}

fn dirs_next() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Get or create persistent instance ID.
pub fn instance_id() -> &'static str {
    INSTANCE_ID.get_or_init(|| {
        let dir = data_dir();
        let id_file = dir.join("instance_id");

        if let Ok(id) = fs::read_to_string(&id_file) {
            let id = id.trim().to_string();
            if !id.is_empty() {
                return id;
            }
        }

        let id = Uuid::new_v4().to_string()[..8].to_string();
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(&id_file, &id);
        id
    })
}

/// Initialize tracing subscriber (JSON to stderr).
pub fn setup_logging() {
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("gl_mcp=info"));

    fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

/// Initialize Sentry if SENTRY_DSN is set.
pub fn setup_sentry() {
    SENTRY_GUARD.get_or_init(|| {
        let dsn = std::env::var("SENTRY_DSN").ok()?;
        if dsn.is_empty() {
            return None;
        }

        let guard = sentry::init(sentry::ClientOptions {
            dsn: dsn.parse().ok(),
            release: Some(format!("gl-mcp@{}", env!("CARGO_PKG_VERSION")).into()),
            environment: std::env::var("SENTRY_ENVIRONMENT")
                .ok()
                .map(Into::into)
                .or(Some("production".into())),
            sample_rate: 1.0,
            traces_sample_rate: 0.0,
            send_default_pii: false,
            before_send: Some(std::sync::Arc::new(|mut event| {
                // Scrub any token-like strings from exception messages
                for e in event.exception.values.iter_mut() {
                    if let Some(ref val) = e.value {
                        e.value = Some(scrub_tokens(val));
                    }
                }
                Some(event)
            })),
            ..Default::default()
        });

        sentry::configure_scope(|scope| {
            scope.set_tag("instance_id", instance_id());
        });

        eprintln!("gl-mcp: Sentry enabled");
        Some(guard)
    });
}

/// Scrub GitLab tokens and other secrets from strings.
fn scrub_tokens(s: &str) -> String {
    let re = regex::Regex::new(r"glpat-[A-Za-z0-9_\-\.]+").unwrap();
    re.replace_all(s, "glpat-[REDACTED]").to_string()
}

/// Add a Sentry breadcrumb for a tool call.
fn add_sentry_breadcrumb(tool: &str, duration_ms: u128, status: &str, error: Option<&str>) {
    if !sentry::Hub::current().client().is_some() {
        return;
    }
    let mut data = std::collections::BTreeMap::new();
    data.insert("tool".to_string(), sentry::protocol::Value::from(tool));
    data.insert("duration_ms".to_string(), sentry::protocol::Value::from(duration_ms as f64));
    data.insert("status".to_string(), sentry::protocol::Value::from(status));
    if let Some(err) = error {
        data.insert("error".to_string(), sentry::protocol::Value::from(err));
    }

    sentry::add_breadcrumb(sentry::Breadcrumb {
        ty: "default".into(),
        category: Some("tool_call".into()),
        message: Some(format!("{tool}: {status} ({duration_ms}ms)")),
        data,
        level: if status == "error" {
            sentry::Level::Error
        } else {
            sentry::Level::Info
        },
        ..Default::default()
    });

    // Capture errors as Sentry events
    if status == "error" {
        if let Some(err) = error {
            sentry::capture_message(
                &format!("Tool error: {tool}: {err}"),
                sentry::Level::Error,
            );
        }
    }
}

/// Parameters safe to log (no tokens, no secrets).
#[allow(dead_code)]
const SAFE_PARAMS: &[&str] = &[
    "project_id", "query", "search", "instance", "state",
    "scope", "per_page", "page", "ref_name", "branch",
    "source_branch", "target_branch", "milestone",
];

/// Extract safe parameters from a JSON value for analytics.
#[allow(dead_code)]
pub fn extract_safe_params(params: &serde_json::Value) -> serde_json::Value {
    if let Some(obj) = params.as_object() {
        let safe: serde_json::Map<String, serde_json::Value> = obj
            .iter()
            .filter(|(k, _)| SAFE_PARAMS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        serde_json::Value::Object(safe)
    } else {
        serde_json::Value::Null
    }
}

/// Hash parameters for privacy-safe analytics.
#[allow(dead_code)]
pub fn hash_params(params: &serde_json::Value) -> String {
    let bytes = serde_json::to_vec(params).unwrap_or_default();
    let hash = Sha256::digest(&bytes);
    format!("{:x}", hash)[..16].to_string()
}

/// Analytics event for tool calls.
#[derive(serde::Serialize)]
pub struct AnalyticsEvent {
    pub ts: String,
    pub tool: String,
    pub duration_ms: u128,
    pub status: String,
    pub instance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "is_zero")]
    pub response_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// Log an analytics event to ~/.gl-mcp/analytics.log
pub fn log_analytics(event: &AnalyticsEvent) {
    let dir = data_dir();
    let log_file = std::env::var("GITLAB_ANALYTICS_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dir.join("analytics.log"));

    let _ = fs::create_dir_all(log_file.parent().unwrap_or(&dir));

    if let Ok(line) = serde_json::to_string(event) {
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// Helper to measure and log a tool call.
pub struct ToolTimer {
    pub tool_name: String,
    pub start: Instant,
    pub params: Option<serde_json::Value>,
}

impl ToolTimer {
    pub fn start(tool_name: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            start: Instant::now(),
            params,
        }
    }

    pub fn finish(&self, status: &str, response_size: usize, error: Option<String>) {
        let duration_ms = self.start.elapsed().as_millis();
        let event = AnalyticsEvent {
            ts: chrono::Utc::now().to_rfc3339(),
            tool: self.tool_name.clone(),
            duration_ms,
            status: status.to_string(),
            instance: instance_id().to_string(),
            params: self.params.clone(),
            response_size,
            error: error.clone(),
        };
        log_analytics(&event);
        add_sentry_breadcrumb(&self.tool_name, duration_ms, status, error.as_deref());
    }
}
