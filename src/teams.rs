//! Team configuration from ~/.gl-mcp/teams.json
//!
//! Not committed to repo — each user configures their own teams.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub username: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    #[serde(default)]
    pub members: Vec<TeamMember>,
    #[serde(default)]
    pub projects: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Teams {
    teams: BTreeMap<String, Team>,
}

impl Teams {
    /// Load from ~/.gl-mcp/teams.json. Returns empty if file doesn't exist.
    pub fn load() -> Self {
        let path = Self::config_path();
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return Self::default(),
        };
        let teams: BTreeMap<String, Team> = match serde_json::from_str(&content) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("Warning: failed to parse teams.json: {e}");
                return Self::default();
            }
        };
        Self { teams }
    }

    /// Get a team by key.
    pub fn get(&self, key: &str) -> Option<&Team> {
        self.teams.get(key)
    }

    /// List all team keys.
    pub fn list(&self) -> Vec<(&String, &Team)> {
        self.teams.iter().collect()
    }

    /// Get usernames for a team.
    pub fn usernames(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|t| t.members.iter().map(|m| m.username.clone()).collect())
            .unwrap_or_default()
    }

    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".gl-mcp").join("teams.json")
    }

    /// Save teams to ~/.gl-mcp/teams.json
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.teams)?;
        std::fs::write(&path, content)
    }

    /// Add or update a team.
    pub fn set(&mut self, key: String, team: Team) {
        self.teams.insert(key, team);
    }
}
