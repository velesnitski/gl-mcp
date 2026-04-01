//! Multi-instance resolver.
//!
//! Priority:
//! 1. Explicit `instance` parameter
//! 2. Auto-detect from URL domain in identifier
//! 3. Default (first configured instance)

use std::collections::HashMap;

use crate::client::GitLabClient;
use crate::error::{Error, Result};
use crate::config::Config;

/// Resolves which GitLab instance to use for a given request.
pub struct Resolver {
    clients: HashMap<String, GitLabClient>,
    domain_map: HashMap<String, String>,
    default_name: String,
}

impl Resolver {
    pub fn new(config: &Config) -> Self {
        let mut clients = HashMap::new();
        for inst in &config.instances {
            match GitLabClient::new(inst) {
                Ok(client) => { clients.insert(inst.name.clone(), client); }
                Err(e) => { eprintln!("Warning: skipping instance '{}': {e}", inst.name); }
            }
        }

        let default_name = config
            .instances
            .first()
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "default".to_string());

        Self {
            clients,
            domain_map: config.domain_map(),
            default_name,
        }
    }

    /// Resolve the client to use.
    ///
    /// - If `instance` is non-empty, use that instance by name.
    /// - If `identifier` contains a URL, extract domain and look up instance.
    /// - Otherwise, use the default instance.
    pub fn resolve(&self, instance: &str, identifier: &str) -> Result<&GitLabClient> {
        // 1. Explicit instance
        if !instance.is_empty() {
            return self
                .clients
                .get(instance)
                .ok_or_else(|| Error::NotFound(format!("Unknown instance: {instance}")));
        }

        // 2. Auto-detect from URL
        if identifier.contains("://") {
            if let Ok(url) = url::Url::parse(identifier) {
                if let Some(host) = url.host_str() {
                    if let Some(name) = self.domain_map.get(host) {
                        if let Some(client) = self.clients.get(name) {
                            return Ok(client);
                        }
                    }
                }
            }
        }

        // 3. Default
        self.clients
            .get(&self.default_name)
            .ok_or_else(|| Error::Config("No default instance configured".into()))
    }

}
