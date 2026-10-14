//! Plumbing that needs no GitLab at all: instance resolution, client construction
//! and its failure modes, the teams file, and the analytics log. Runs in its own
//! process with `HOME` pointed at a temporary directory.

use gl_mcp::client::GitLabClient;
use gl_mcp::config::{Config, GitLabInstance};
use gl_mcp::resolver::Resolver;
use gl_mcp::teams::{Team, TeamMember, Teams};

fn instance(name: &str, url: &str, token: &str) -> GitLabInstance {
    GitLabInstance { name: name.into(), url: url.into(), token: token.into() }
}

fn config(instances: Vec<GitLabInstance>) -> Config {
    Config { instances, read_only: false, disabled_tools: vec![], compact: false, toolset: "full".into() }
}

fn temp_home(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gl-mcp-plumbing-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: these tests run single-threaded (`--test-threads=1`, as CI does).
    unsafe { std::env::set_var("HOME", &dir) };
    dir
}

#[test]
fn resolver_prefers_explicit_name_then_url_domain_then_default() {
    let r = Resolver::new(&config(vec![
        instance("main", "https://gitlab.example.com", "t1"),
        instance("other", "https://code.example.org", "t2"),
    ]));
    assert_eq!(r.instance_count(), 2);
    assert_eq!(r.resolve("other", "").unwrap().name, "other");
    assert_eq!(r.resolve("", "https://code.example.org/g/p/-/merge_requests/1").unwrap().name, "other");
    assert_eq!(r.resolve("", "g/p").unwrap().name, "main", "no URL: the first instance");
    assert_eq!(r.resolve("", "https://unknown.example.net/g/p").unwrap().name, "main");
    let err = r.resolve("missing", "").err().expect("unknown instance is an error");
    assert!(err.is_user_error(), "an unknown instance name is the caller's mistake");
    assert_eq!(r.all_clients().len(), 2);
}

#[test]
fn an_instance_with_an_unusable_token_is_skipped_not_fatal() {
    // A token with a newline cannot be an HTTP header value.
    assert!(GitLabClient::new(&instance("bad", "https://gitlab.example.com", "a\nb")).is_err());
    let r = Resolver::new(&config(vec![
        instance("bad", "https://gitlab.example.com", "a\nb"),
        instance("good", "https://code.example.org", "t"),
    ]));
    assert_eq!(r.instance_count(), 1);
    // The default was the skipped instance, so there is no default left to fall back to.
    assert!(r.resolve("", "g/p").is_err());
    assert_eq!(r.resolve("good", "").unwrap().name, "good");
}

#[tokio::test]
async fn an_unreachable_gitlab_is_a_real_error_and_is_not_retried() {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let client = GitLabClient::new(&instance("x", &format!("http://127.0.0.1:{port}"), "t")).unwrap();
    let started = std::time::Instant::now();
    let err = client.get::<serde_json::Value>("/projects", &[]).await.unwrap_err();
    assert!(!err.is_user_error(), "an outage must not be classified as the caller's mistake");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "connection errors are not retried");
    assert!(client.get_text("/x", &[]).await.is_err());
    assert!(client.get_all_pages::<serde_json::Value>("/x", &[], 3).await.is_err());
    assert!(client.post::<serde_json::Value>("/x", &serde_json::json!({})).await.is_err());
    assert!(client.put::<serde_json::Value>("/x", &serde_json::json!({})).await.is_err());
    assert!(client.delete("/x").await.is_err());
    assert!(client.get_cached::<serde_json::Value>("k", "/x", &[], 60).await.is_err());
}

#[tokio::test]
async fn teams_round_trip_through_the_file_and_survive_corruption() {
    let home = temp_home("teams");
    assert!(Teams::load().list().is_empty(), "no file: no teams");

    let mut teams = Teams::default();
    teams.set(
        "core".into(),
        Team {
            name: "Core".into(),
            members: vec![TeamMember { username: "ada".into(), name: String::new(), instance: None }],
            projects: vec!["g/p".into()],
        },
    );
    teams.save().await.unwrap();
    let loaded = Teams::load();
    assert_eq!(loaded.get("core").unwrap().name, "Core");
    assert_eq!(loaded.usernames("core"), vec!["ada".to_string()]);
    assert!(loaded.usernames("nobody").is_empty());

    std::fs::write(home.join(".gl-mcp/teams.json"), "{ not json").unwrap();
    assert!(Teams::load().list().is_empty(), "a corrupt file degrades to no teams, not a crash");
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn analytics_events_are_appended_as_json_lines() {
    let home = temp_home("analytics");
    let file = home.join("events.log");
    // SAFETY: single-threaded test run.
    unsafe { std::env::set_var("GITLAB_ANALYTICS_FILE", &file) };
    let timer = gl_mcp::logging::ToolTimer::start("list_projects", None);
    timer.finish("ok", 42, None);
    timer.finish("error", 0, Some("boom".into()));
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["tool"], "list_projects");
    assert_eq!(lines[0]["response_size"], 42);
    assert!(lines[1].get("response_size").is_none(), "zero sizes are omitted");
    assert_eq!(lines[1]["error"], "boom");
    unsafe { std::env::remove_var("GITLAB_ANALYTICS_FILE") };
    let _ = std::fs::remove_dir_all(home);
}
