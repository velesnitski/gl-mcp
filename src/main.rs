//! gl-mcp — GitLab MCP server for Claude Code.
//!
//! Usage:
//!   GITLAB_URL=https://gitlab.com GITLAB_TOKEN=glpat-xxx gl-mcp

mod client;
mod config;
mod error;
mod logging;
mod resolver;
mod server;
mod teams;
mod tools;

use rmcp::ServiceExt;
use rmcp::transport::stdio;

use crate::config::Config;
use crate::server::GlMcpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logging to stderr (stdout is reserved for JSON-RPC)
    logging::setup_logging();
    logging::setup_sentry();
    let instance_id = logging::instance_id();
    eprintln!("gl-mcp v{} starting (instance: {instance_id})", env!("CARGO_PKG_VERSION"));

    let config = Config::from_env().unwrap_or_else(|e| {
        eprintln!("Configuration error: {e}");
        eprintln!("Set GITLAB_URL and GITLAB_TOKEN environment variables.");
        std::process::exit(1);
    });

    eprintln!(
        "Configured {} instance(s), read_only={}",
        config.instances.len(),
        config.read_only,
    );

    let server = GlMcpServer::new(config);
    let service = server.serve(stdio()).await?;

    eprintln!("gl-mcp: serving via stdio");
    service.waiting().await?;

    Ok(())
}
