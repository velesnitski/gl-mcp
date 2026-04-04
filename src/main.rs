//! gl-mcp — GitLab MCP server.
//!
//! Usage:
//!   gl-mcp                              # stdio transport (default)
//!   gl-mcp --transport http --port 8000 # HTTP/SSE transport for n8n, etc.

mod client;
mod config;
mod error;
mod logging;
mod params;
mod resolver;
mod server;
mod teams;
mod tools;

use crate::config::Config;
use crate::server::GlMcpServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    let args: Vec<String> = std::env::args().collect();
    let transport = args.iter()
        .position(|a| a == "--transport")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("stdio");

    let port: u16 = args.iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(8000);

    match transport {
        "http" | "sse" | "streamable-http" => {
            serve_http(config, port).await
        }
        "stdio" | _ => {
            serve_stdio(config).await
        }
    }
}

async fn serve_stdio(config: Config) -> anyhow::Result<()> {
    use rmcp::ServiceExt;
    use rmcp::transport::stdio;

    let server = GlMcpServer::new(config);
    let service = server.serve(stdio()).await?;
    eprintln!("gl-mcp: serving via stdio");
    service.waiting().await?;
    Ok(())
}

async fn serve_http(config: Config, port: u16) -> anyhow::Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpService, StreamableHttpServerConfig,
        session::local::LocalSessionManager,
    };
    use std::sync::Arc;
    use std::net::SocketAddr;

    let session_manager = Arc::new(LocalSessionManager::default());
    let http_config = StreamableHttpServerConfig::default();

    let mcp_service = StreamableHttpService::new(
        move || Ok(GlMcpServer::new(config.clone())),
        session_manager,
        http_config,
    );

    let app = axum::Router::new()
        .fallback_service(mcp_service);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("gl-mcp: serving HTTP on http://0.0.0.0:{port}/mcp");

    axum::serve(listener, app).await?;
    Ok(())
}
