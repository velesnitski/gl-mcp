//! End-to-end over the MCP protocol, with GitLab unreachable.
//!
//! The real server runs in-process on one end of an in-memory pipe and a real MCP
//! client drives it from the other: `tools/list` and `tools/call` go through the
//! same serialization, routing and wrapper code a client like Claude Code hits.
//!
//! GitLab points at a localhost port that was just bound and released, so every
//! request is refused on the spot. Nothing leaves the machine and there is no mock
//! server: this is the genuine "GitLab is down" case, and every tool has to survive
//! it — answer with a tool error, never panic, never hang, and never report success
//! with data it could not have fetched.
//!
//! `HOME` is redirected to a temporary directory before the server starts, so the
//! analytics log, teams.json, spec maps and instance id never touch the real one.

use std::collections::BTreeMap;
use std::time::Duration;

use gl_mcp::config::{Config, GitLabInstance};
use gl_mcp::server::GlMcpServer;
use rmcp::model::{CallToolRequestParam, Tool};
use rmcp::ServiceExt;
use serde_json::{json, Map, Value};

/// Tools that legitimately answer without GitLab: they read embedded rules or the
/// local teams file, or validate input that is already in the arguments.
const OFFLINE_TOOLS: &[&str] = &["list_rules", "list_teams", "save_team", "validate_commit"];

fn isolate_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("gl-mcp-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: set before any thread that reads the environment is started.
    unsafe {
        std::env::set_var("HOME", &dir);
        std::env::remove_var("SENTRY_DSN");
        std::env::remove_var("GITLAB_ANALYTICS_FILE");
        // The vulnerability database is an outside service too: keep tests offline.
        std::env::set_var("OSV_API_URL", format!("http://127.0.0.1:{}", closed_port()));
    }
    dir
}

/// A localhost port with nothing listening: bind, read the port, release it.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn config(port: u16, read_only: bool) -> Config {
    Config {
        instances: vec![GitLabInstance {
            name: "default".into(),
            url: format!("http://127.0.0.1:{port}"),
            token: "test-token".into(),
        }],
        read_only,
        disabled_tools: Vec::new(),
        compact: false,
        toolset: "full".into(),
    }
}

type Client = rmcp::service::RunningService<rmcp::RoleClient, ()>;

async fn connect(cfg: Config) -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = GlMcpServer::new(cfg).serve(server_io).await.expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

/// The schema's type, looking through `nullable` and `anyOf` wrappers.
fn schema_type(schema: &Value) -> &str {
    if let Some(t) = schema["type"].as_str() {
        return t;
    }
    if let Some(arr) = schema["type"].as_array() {
        return arr.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("null");
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(alts) = schema[key].as_array() {
            if let Some(alt) = alts.iter().find(|a| a["type"] != "null") {
                return schema_type(alt);
            }
        }
    }
    "string"
}

/// The smallest arguments the schema accepts: every required field, nothing else.
fn minimal_args(tool: &Tool) -> Map<String, Value> {
    let schema = Value::Object((*tool.input_schema).clone());
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut args = Map::new();
    for name in required {
        let prop = &schema["properties"][name];
        let value = match schema_type(prop) {
            "integer" | "number" => json!(1),
            "boolean" => json!(false),
            "array" => json!([]),
            "object" => json!({}),
            _ => json!("group/project"),
        };
        args.insert(name.to_string(), value);
    }
    args
}

#[tokio::test(flavor = "multi_thread")]
async fn every_tool_survives_an_unreachable_gitlab() {
    let home = isolate_home();
    let client = connect(config(closed_port(), false)).await;

    let tools = client.list_all_tools().await.expect("tools/list");
    assert!(tools.len() >= 100, "full toolset is listed over the wire (got {})", tools.len());

    let mut outcomes: BTreeMap<String, &'static str> = BTreeMap::new();
    for tool in &tools {
        let name = tool.name.to_string();
        let call = client.call_tool(CallToolRequestParam {
            name: tool.name.clone(),
            arguments: Some(minimal_args(tool)),
        });
        let outcome = match tokio::time::timeout(Duration::from_secs(20), call).await {
            Err(_) => "HUNG",
            Ok(Err(_)) => "protocol-error",
            Ok(Ok(r)) if r.is_error == Some(true) => "tool-error",
            Ok(Ok(r)) => {
                if std::env::var_os("E2E_PEEK").is_some() {
                    let text: String = r.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect();
                    eprintln!("=== {name}: {}", text.chars().take(300).collect::<String>().replace('\n', " | "));
                }
                "success"
            }
        };
        outcomes.insert(name, outcome);
    }

    let hung: Vec<_> = outcomes.iter().filter(|(_, o)| **o == "HUNG").map(|(n, _)| n).collect();
    assert!(hung.is_empty(), "tools that never answered: {hung:?}");

    // Success without GitLab is only possible for tools that need no GitLab. Anything
    // else reporting success has rendered an answer out of data it never received.
    let fabricated: Vec<_> = outcomes
        .iter()
        .filter(|(n, o)| **o == "success" && !OFFLINE_TOOLS.contains(&n.as_str()))
        .map(|(n, _)| n)
        .collect();
    assert!(fabricated.is_empty(), "success with GitLab unreachable: {fabricated:?}");

    // The server introduces itself by name and the running crate version.
    let info = client.peer_info().expect("server info received at initialize");
    // The name carries the version on purpose: clients label the server by it.
    assert_eq!(info.server_info.name, concat!("gl-mcp v", env!("CARGO_PKG_VERSION")));
    assert_eq!(info.server_info.version, env!("CARGO_PKG_VERSION"));

    // The server is still alive and answering after every tool has failed once.
    assert_eq!(client.list_all_tools().await.expect("still serving").len(), tools.len());

    let _ = client.cancel().await;
    let _ = std::fs::remove_dir_all(home);
}

#[tokio::test(flavor = "multi_thread")]
async fn read_only_mode_hides_write_tools_on_the_wire() {
    let home = isolate_home();
    let client = connect(config(closed_port(), true)).await;
    let names: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    for w in gl_mcp::tools::WRITE_TOOLS {
        assert!(!names.iter().any(|n| n == w), "read-only server lists write tool {w}");
    }
    // Calling a hidden write tool by name is refused, not executed.
    let refused = client
        .call_tool(CallToolRequestParam {
            name: "delete_project".into(),
            arguments: Some(Map::from_iter([
                ("project_id".to_string(), json!("group/project")),
                ("confirm_full_path".to_string(), json!("group/project")),
            ])),
        })
        .await;
    assert!(
        refused.is_err() || refused.as_ref().is_ok_and(|r| r.is_error == Some(true)),
        "a write tool must not run in read-only mode"
    );
    let _ = client.cancel().await;
    let _ = std::fs::remove_dir_all(home);
}

/// The public contract: every tool's name, description and input schema, exactly as a
/// client receives them. A rename, a removed parameter or a changed type is a breaking
/// change for every caller; here it fails a test and shows up as a reviewed diff.
/// Regenerate after an intended change with `UPDATE_GOLDEN=1`.
#[tokio::test(flavor = "multi_thread")]
async fn tool_surface_matches_the_published_contract() {
    let home = isolate_home();
    let client = connect(config(closed_port(), false)).await;
    let mut tools = client.list_all_tools().await.unwrap();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let surface: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": Value::Object((*t.input_schema).clone()),
            })
        })
        .collect();
    let actual = serde_json::to_string_pretty(&surface).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/tool_surface.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &actual).unwrap();
    } else {
        let expected = std::fs::read_to_string(&path).expect("tool_surface.json — run with UPDATE_GOLDEN=1");
        if actual != expected {
            let a: Vec<Value> = serde_json::from_str(&actual).unwrap();
            let e: Vec<Value> = serde_json::from_str(&expected).unwrap();
            let names = |v: &[Value]| v.iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
            let (an, en) = (names(&a), names(&e));
            let added: Vec<_> = an.iter().filter(|n| !en.contains(n)).collect();
            let removed: Vec<_> = en.iter().filter(|n| !an.contains(n)).collect();
            let changed: Vec<_> = a.iter().filter(|t| e.iter().any(|x| x["name"] == t["name"] && x != *t)).map(|t| t["name"].as_str().unwrap()).collect();
            panic!("tool surface changed — added {added:?}, removed {removed:?}, changed {changed:?}. Review, then UPDATE_GOLDEN=1.");
        }
    }
    let _ = client.cancel().await;
    let _ = std::fs::remove_dir_all(home);
}

/// A secret passed as a tool argument must not be written anywhere: not to the
/// analytics log the server keeps on disk, and not back into the response.
#[tokio::test(flavor = "multi_thread")]
async fn secret_arguments_never_reach_the_analytics_log() {
    let home = isolate_home();
    let client = connect(config(closed_port(), false)).await;
    // Assembled at runtime: no secret-shaped literal sits in this file.
    let secret = ["e2e", "-marker-", &std::process::id().to_string(), "-zz"].concat();
    let result = client
        .call_tool(CallToolRequestParam {
            name: "set_ci_variable".into(),
            arguments: Some(Map::from_iter([
                ("project_id".to_string(), json!("group/project")),
                ("key".to_string(), json!("DEPLOY_TOKEN")),
                ("value".to_string(), json!(secret)),
            ])),
        })
        .await
        .expect("call completes");
    let text: String = result.content.iter().filter_map(|c| c.as_text()).map(|t| t.text.clone()).collect();
    assert!(!text.contains(&secret), "the response echoed the secret");

    let log = std::fs::read_to_string(home.join(".gl-mcp/analytics.log")).expect("the call was logged");
    assert!(log.contains("set_ci_variable"), "analytics records the call itself");
    assert!(!log.contains(&secret), "analytics log contains a secret argument");
    let _ = client.cancel().await;
    let _ = std::fs::remove_dir_all(home);
}
