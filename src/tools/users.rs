//! Shared user-resolution and access-level helpers.
//!
//! One implementation of "username → id" for the whole tool surface, with two
//! deliberate flavors:
//!
//! - [`resolve_user_id`] — **hard**: errors when the user doesn't exist. For
//!   operations whose whole point is the user (member grants), where silently
//!   proceeding would be wrong.
//! - [`lookup_user_id`] — **soft**: `Ok(None)` when the user doesn't exist. For
//!   optional enrichments (assignee, reviewers) where the write should proceed
//!   without the field rather than fail.

use serde_json::Value;

use crate::client::GitLabClient;
use crate::error::{Error, Result};

/// Resolve a user given as a numeric id or a username (leading `@` optional),
/// returning `(user id, username)`. Errors (as user input) if not found.
pub(crate) async fn resolve_user_id(client: &GitLabClient, user: &str) -> Result<(u64, String)> {
    let u = user.trim().trim_start_matches('@');
    if !u.is_empty() && u.chars().all(|c| c.is_ascii_digit()) {
        let usr: Value = client.get(&format!("/users/{u}"), &[]).await?;
        let id = usr["id"].as_u64().unwrap_or_else(|| u.parse().unwrap_or(0));
        let name = usr["username"].as_str().unwrap_or(u).to_string();
        return Ok((id, name));
    }
    let users: Vec<Value> = client.get("/users", &[("username", u)]).await?;
    let usr = users
        .into_iter()
        .next()
        .ok_or_else(|| Error::UserInput(format!("User '@{u}' not found")))?;
    let id = usr["id"]
        .as_u64()
        .ok_or_else(|| Error::UserInput(format!("User '@{u}' has no id")))?;
    let name = usr["username"].as_str().unwrap_or(u).to_string();
    Ok((id, name))
}

/// Look up a username → id, returning `Ok(None)` when the user doesn't exist.
/// Transport/API failures still propagate as errors.
pub(crate) async fn lookup_user_id(client: &GitLabClient, username: &str) -> Result<Option<u64>> {
    let u = username.trim().trim_start_matches('@');
    let users: Vec<Value> = client.get("/users", &[("username", u)]).await?;
    Ok(users.first().and_then(|usr| usr["id"].as_u64()))
}

/// Map a friendly access-level name (or numeric string) to GitLab's numeric level.
pub(crate) fn parse_access_level(level: &str) -> Result<u32> {
    Ok(match level.trim().to_lowercase().as_str() {
        "guest" | "10" => 10,
        "planner" | "15" => 15,
        "reporter" | "20" => 20,
        "developer" | "dev" | "30" => 30,
        "maintainer" | "40" => 40,
        "owner" | "50" => 50,
        other => {
            return Err(Error::UserInput(format!(
                "Unknown access level '{other}'. Use one of: guest, reporter, developer, maintainer, owner (or 10/20/30/40/50)."
            )))
        }
    })
}

/// Human-readable name for a GitLab access level number.
pub(crate) fn access_level_name(n: u64) -> &'static str {
    match n {
        10 => "Guest",
        15 => "Planner",
        20 => "Reporter",
        30 => "Developer",
        40 => "Maintainer",
        50 => "Owner",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_access_level_names_and_numbers() {
        assert_eq!(parse_access_level("developer").unwrap(), 30);
        assert_eq!(parse_access_level(" Maintainer ").unwrap(), 40);
        assert_eq!(parse_access_level("50").unwrap(), 50);
        let err = parse_access_level("boss").unwrap_err();
        assert!(err.is_user_error(), "unknown role must classify as user error");
    }

    #[test]
    fn access_level_names_round_trip() {
        for (n, name) in [(10, "Guest"), (30, "Developer"), (50, "Owner")] {
            assert_eq!(access_level_name(n), name);
            assert_eq!(parse_access_level(&n.to_string()).unwrap() as u64, n);
        }
        assert_eq!(access_level_name(99), "?");
    }
}
