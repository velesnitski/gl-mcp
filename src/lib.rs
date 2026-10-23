//! gl-mcp library — reusable components for GitLab MCP server.

pub mod client;
pub mod config;
pub mod error;
pub mod http_guard;
pub mod logging;
pub mod params;
pub mod resolver;
pub mod server;
pub mod teams;
pub mod tools;

#[cfg(test)]
mod golden;
#[cfg(test)]
mod proptests;
#[cfg(test)]
mod render_goldens;
