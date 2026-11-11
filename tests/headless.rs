//! Headless mode end to end: the real command path (`gl_mcp::headless::run`) over
//! the full server, with GitLab pointed at a closed localhost port so nothing
//! leaves the machine. `HOME` is redirected before anything starts, so the
//! analytics log and teams file never touch the real one.

use gl_mcp::config::{Config, GitLabInstance};
use gl_mcp::headless::{run, EXIT_OK, EXIT_TOOL_ERROR, EXIT_USAGE};
use serde_json::Value;

fn isolate_home() {
    let dir = std::env::temp_dir().join(format!("gl-mcp-headless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // SAFETY: set before any thread that reads the environment is started.
    unsafe {
        std::env::set_var("HOME", &dir);
        std::env::remove_var("SENTRY_DSN");
        std::env::remove_var("GITLAB_ANALYTICS_FILE");
    }
}

fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

fn config(read_only: bool, disabled: &[&str]) -> Config {
    Config {
        instances: vec![GitLabInstance {
            name: "default".into(),
            url: format!("http://127.0.0.1:{}", closed_port()),
            token: "test-token".into(),
        }],
        read_only,
        disabled_tools: disabled.iter().map(|s| s.to_string()).collect(),
        compact: false,
        toolset: "full".into(),
    }
}

/// (exit code, stdout, stderr)
async fn cli(args: &[&str], cfg: Config, stdin: &str) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&args, cfg, &mut std::io::Cursor::new(stdin.to_string()), &mut out, &mut err).await;
    (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
}

#[tokio::test(flavor = "multi_thread")]
async fn headless_lists_calls_types_and_gates_like_a_client() {
    isolate_home();

    // tools: every registered tool, sorted, one per line.
    let (code, out, _) = cli(&["tools"], config(false, &[]), "").await;
    assert_eq!(code, EXIT_OK);
    let names: Vec<&str> = out.lines().map(|l| l.split_whitespace().next().unwrap()).collect();
    assert!(names.len() >= 100, "full toolset listed (got {})", names.len());
    assert!(names.windows(2).all(|w| w[0] <= w[1]), "sorted");
    assert!(names.contains(&"delete_project") && names.contains(&"list_rules"));

    // tools --json: full definitions with input schemas.
    let (code, out, _) = cli(&["tools", "--json"], config(false, &[]), "").await;
    assert_eq!(code, EXIT_OK);
    let defs: Vec<Value> = serde_json::from_str(&out).unwrap();
    assert!(defs.iter().all(|d| d["inputSchema"].is_object()));

    // A tool that needs no GitLab succeeds; --json has the stable shape.
    let (code, out, _) = cli(&["run", "list_rules", "--json"], config(false, &[]), "").await;
    assert_eq!(code, EXIT_OK);
    let rec: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(rec["tool"], "list_rules");
    assert_eq!(rec["ok"], true);
    assert!(rec["text"].as_str().unwrap().len() > 20);

    // GitLab unreachable: the tool runs and reports an error — exit 1, not 2.
    // The integer argument went over the wire as an integer (else: usage error).
    let (code, out, _) = cli(
        &["run", "get_pipeline", "project_id=g/p", "--json", "pipeline_id=12"],
        config(false, &[]),
        "",
    )
    .await;
    assert_eq!(code, EXIT_TOOL_ERROR, "{out}");
    let rec: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(rec["ok"], false);
    assert_eq!(rec["args"]["pipeline_id"], 12);

    // --args from stdin, pairs override it.
    let (code, out, _) = cli(
        &["run", "get_pipeline", "--args", "-", "pipeline_id=7", "--json"],
        config(false, &[]),
        r#"{"project_id": "g/p", "pipeline_id": 1}"#,
    )
    .await;
    assert_eq!(code, EXIT_TOOL_ERROR);
    assert_eq!(serde_json::from_str::<Value>(out.trim()).unwrap()["args"]["pipeline_id"], 7);

    // Usage errors: exit 2 with a message on stderr, nothing on stdout.
    for (args, needle) in [
        (vec!["run", "no_such_tool"], "unknown tool"),
        (vec!["run", "get_pipeline", "projct_id=g/p"], "it takes:"),
        (vec!["run", "get_pipeline", "project_id=g/p"], "needs pipeline_id"),
        (vec!["run", "get_pipeline", "project_id=g/p", "pipeline_id=x"], "whole number"),
        (vec!["run"], "usage:"),
        (vec!["tools", "--verbose"], "usage:"),
    ] {
        let (code, out, err) = cli(&args, config(false, &[]), "").await;
        assert_eq!(code, EXIT_USAGE, "{args:?}: {err}");
        assert!(out.is_empty(), "{args:?} printed to stdout: {out}");
        assert!(err.contains(needle), "{args:?}: {err}");
    }

    // Gating is the server's: under read-only a write tool does not exist, and a
    // disabled tool is gone too.
    let (code, _, err) = cli(&["run", "delete_project", "project_id=g/p"], config(true, &[]), "").await;
    assert_eq!(code, EXIT_USAGE);
    assert!(err.contains("GITLAB_READ_ONLY"));
    let (_, out, _) = cli(&["tools"], config(true, &["list_rules"]), "").await;
    assert!(!out.contains("delete_project") && !out.lines().any(|l| l.starts_with("list_rules ")));
}
