//! Config loading tests.
//! Note: env var manipulation is unsafe in Rust 2024 edition.
//! Tests must run single-threaded: cargo test -- --test-threads=1

use std::env;

macro_rules! set_env {
    ($key:expr, $val:expr) => { unsafe { env::set_var($key, $val) } };
}
macro_rules! rm_env {
    ($key:expr) => { unsafe { env::remove_var($key) } };
}

fn clean_env() {
    rm_env!("GITLAB_INSTANCES");
    rm_env!("GITLAB_URL");
    rm_env!("GITLAB_TOKEN");
    rm_env!("GITLAB_READ_ONLY");
    rm_env!("GITLAB_COMPACT");
    rm_env!("DISABLED_TOOLS");
    rm_env!("GITLAB_ALLOW_HTTP");
    rm_env!("GITLAB_MAIN_URL");
    rm_env!("GITLAB_MAIN_TOKEN");
    rm_env!("GITLAB_STAGING_URL");
    rm_env!("GITLAB_STAGING_TOKEN");
}

#[test]
fn test_single_instance() {
    clean_env();
    set_env!("GITLAB_URL", "https://gitlab.example.com");
    set_env!("GITLAB_TOKEN", "glpat-test");

    let c = gl_mcp::config::Config::from_env().unwrap();
    assert_eq!(c.instances.len(), 1);
    assert_eq!(c.instances[0].name, "default");
    assert_eq!(c.instances[0].url, "https://gitlab.example.com");
    assert!(!c.read_only);
    assert!(!c.compact);
    clean_env();
}

#[test]
fn test_trailing_slash() {
    clean_env();
    set_env!("GITLAB_URL", "https://gitlab.example.com/");
    set_env!("GITLAB_TOKEN", "t");

    let c = gl_mcp::config::Config::from_env().unwrap();
    assert_eq!(c.instances[0].url, "https://gitlab.example.com");
    clean_env();
}

#[test]
fn test_read_only() {
    clean_env();
    set_env!("GITLAB_URL", "https://gitlab.example.com");
    set_env!("GITLAB_TOKEN", "t");
    set_env!("GITLAB_READ_ONLY", "true");

    assert!(gl_mcp::config::Config::from_env().unwrap().read_only);
    clean_env();
}

#[test]
fn test_compact() {
    clean_env();
    set_env!("GITLAB_URL", "https://gitlab.example.com");
    set_env!("GITLAB_TOKEN", "t");
    set_env!("GITLAB_COMPACT", "1");

    assert!(gl_mcp::config::Config::from_env().unwrap().compact);
    clean_env();
}

#[test]
fn test_disabled_tools() {
    clean_env();
    set_env!("GITLAB_URL", "https://gitlab.example.com");
    set_env!("GITLAB_TOKEN", "t");
    set_env!("DISABLED_TOOLS", "create_issue, retry-pipeline");

    let c = gl_mcp::config::Config::from_env().unwrap();
    assert_eq!(c.disabled_tools, vec!["create_issue", "retry_pipeline"]);
    clean_env();
}

#[test]
fn test_missing_url() {
    clean_env();
    set_env!("GITLAB_TOKEN", "t");

    assert!(gl_mcp::config::Config::from_env().is_err());
    clean_env();
}

#[test]
fn test_http_rejected() {
    clean_env();
    set_env!("GITLAB_URL", "http://gitlab.example.com");
    set_env!("GITLAB_TOKEN", "t");

    assert!(gl_mcp::config::Config::from_env().is_err());
    clean_env();
}

#[test]
fn test_localhost_http_ok() {
    clean_env();
    set_env!("GITLAB_URL", "http://localhost:8080");
    set_env!("GITLAB_TOKEN", "t");

    let c = gl_mcp::config::Config::from_env().unwrap();
    assert_eq!(c.instances[0].url, "http://localhost:8080");
    clean_env();
}

#[test]
fn test_multi_instance() {
    clean_env();
    set_env!("GITLAB_INSTANCES", "main,staging");
    set_env!("GITLAB_MAIN_URL", "https://gitlab.example.com");
    set_env!("GITLAB_MAIN_TOKEN", "t1");
    set_env!("GITLAB_STAGING_URL", "https://staging.example.com");
    set_env!("GITLAB_STAGING_TOKEN", "t2");

    let c = gl_mcp::config::Config::from_env().unwrap();
    assert_eq!(c.instances.len(), 2);
    assert_eq!(c.instances[0].name, "main");
    assert_eq!(c.instances[1].name, "staging");
    clean_env();
}

#[test]
fn test_domain_map() {
    clean_env();
    set_env!("GITLAB_INSTANCES", "main,staging");
    set_env!("GITLAB_MAIN_URL", "https://gitlab.example.com");
    set_env!("GITLAB_MAIN_TOKEN", "t1");
    set_env!("GITLAB_STAGING_URL", "https://staging.example.com");
    set_env!("GITLAB_STAGING_TOKEN", "t2");

    let c = gl_mcp::config::Config::from_env().unwrap();
    let dm = c.domain_map();
    assert_eq!(dm.get("gitlab.example.com"), Some(&"main".to_string()));
    assert_eq!(dm.get("staging.example.com"), Some(&"staging".to_string()));
    clean_env();
}
