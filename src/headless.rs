//! Headless mode (ADR 076): `gl-mcp tools` and `gl-mcp run <tool> [key=value ...]`.
//!
//! A scheduler (cron, launchd, a CI job, another bot) gets exactly what an MCP
//! client gets: the real server runs in-process on one end of an in-memory pipe and
//! a real MCP client drives it from the other, so every call goes through the same
//! serialization, routing, gating (`GITLAB_READ_ONLY`, `DISABLED_TOOLS`,
//! `GITLAB_TOOLSET`) and wrapper code as a session. Nothing is wired per tool: a
//! tool is reachable here the day it is registered. Same interface as slk-mcp's
//! headless mode, so one scheduler drives both.

use std::io::{Read, Write};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::ServiceExt;
use serde_json::{Map, Value};

use crate::config::Config;
use crate::server::GlMcpServer;

/// Exit codes a scheduler can branch on.
pub const EXIT_OK: i32 = 0;
/// The tool ran and reported an error (GitLab refused, not found, ...).
pub const EXIT_TOOL_ERROR: i32 = 1;
/// Unknown tool, bad arguments, bad configuration.
pub const EXIT_USAGE: i32 = 2;

const RUN_USAGE: &str =
    "usage: gl-mcp run <tool> [key=value ...] [--args JSON|-] [--json] [--timeout 5m]";

/// Whether the command line asks for headless mode (`args` without argv[0]).
pub fn is_headless(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("run" | "tools"))
}

/// A mistake in the command line, not in GitLab.
struct Usage(String);

/// Run one headless command and return the process exit code.
pub async fn run(
    args: &[String],
    config: Config,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let (server_io, client_io) = tokio::io::duplex(1 << 22);
    let server = tokio::spawn(async move {
        if let Ok(running) = GlMcpServer::new(config).serve(server_io).await {
            let _ = running.waiting().await;
        }
    });
    let client = match ().serve(client_io).await {
        Ok(c) => c,
        Err(e) => {
            let _ = writeln!(stderr, "gl-mcp: could not start the in-process server: {e}");
            return EXIT_USAGE;
        }
    };
    let result = match args[0].as_str() {
        "tools" => tools_cmd(&client, &args[1..], stdout).await.map(|()| EXIT_OK),
        _ => run_cmd(&client, &args[1..], stdin, stdout).await,
    };
    let _ = client.cancel().await;
    server.abort();
    match result {
        Ok(code) => code,
        Err(Usage(msg)) => {
            let _ = writeln!(stderr, "gl-mcp: {msg}");
            EXIT_USAGE
        }
    }
}

type Client = rmcp::service::RunningService<rmcp::RoleClient, ()>;

async fn list_tools(client: &Client) -> Result<Vec<Tool>, Usage> {
    let mut tools = client
        .list_all_tools()
        .await
        .map_err(|e| Usage(format!("tools/list failed: {e}")))?;
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(tools)
}

/// One tool per line with the first sentence of its description, or the full
/// definitions (input schemas included) under `--json`.
async fn tools_cmd(client: &Client, args: &[String], out: &mut dyn Write) -> Result<(), Usage> {
    let json = match args {
        [] => false,
        [f] if f == "--json" => true,
        _ => return Err(Usage("usage: gl-mcp tools [--json]".into())),
    };
    let tools = list_tools(client).await?;
    if json {
        let text = serde_json::to_string_pretty(&tools).map_err(|e| Usage(e.to_string()))?;
        let _ = writeln!(out, "{text}");
    } else {
        for t in &tools {
            let desc = t.description.as_deref().unwrap_or("");
            let first = desc.split(". ").next().unwrap_or(desc);
            let _ = writeln!(out, "{:<34} {first}", t.name);
        }
    }
    Ok(())
}

/// Parsed `run` command line. Flags may sit anywhere among the pairs.
#[derive(Debug, PartialEq)]
pub(crate) struct RunArgs {
    pub tool: String,
    pub pairs: Vec<(String, String)>,
    pub args_json: Option<String>,
    pub json: bool,
    pub timeout: Duration,
}

pub(crate) fn parse_run(args: &[String]) -> Result<RunArgs, String> {
    let Some(tool) = args.first().filter(|t| !t.starts_with('-')) else {
        return Err(RUN_USAGE.into());
    };
    let mut out = RunArgs {
        tool: tool.clone(),
        pairs: Vec::new(),
        args_json: None,
        json: false,
        timeout: Duration::from_secs(300),
    };
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        // `--flag value` and `--flag=value` both work.
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--json" => out.json = true,
            "--args" => out.args_json = Some(value("--args")?),
            "--timeout" => out.timeout = parse_duration(&value("--timeout")?)?,
            f if f.starts_with("--") => return Err(format!("unknown flag {f}; {RUN_USAGE}")),
            _ => match a.split_once('=') {
                Some((k, v)) if !k.is_empty() => out.pairs.push((k.to_string(), v.to_string())),
                _ => return Err(format!("argument {a:?} is not key=value")),
            },
        }
    }
    Ok(out)
}

/// `90`, `90s`, `5m`, `1h`.
fn parse_duration(s: &str) -> Result<Duration, String> {
    let (num, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s, "s"),
    };
    let n: u64 = num.parse().map_err(|_| format!("--timeout {s:?} is not a duration (e.g. 90s, 5m)"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("--timeout {s:?} is not a duration (e.g. 90s, 5m)")),
    };
    Ok(Duration::from_secs(secs))
}

/// The JSON type a schema property declares, looking through nullable unions
/// (`["integer", "null"]`) and `anyOf`/`oneOf` wrappers.
fn schema_type(schema: &Value) -> &str {
    if let Some(t) = schema["type"].as_str() {
        return t;
    }
    if let Some(arr) = schema["type"].as_array() {
        return arr.iter().filter_map(Value::as_str).find(|t| *t != "null").unwrap_or("string");
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(alt) = schema[key].as_array().and_then(|a| a.iter().find(|x| x["type"] != "null")) {
            return schema_type(alt);
        }
    }
    if schema.get("$ref").is_some() || schema.get("properties").is_some() {
        return "object";
    }
    "string"
}

/// A command-line string as the type the schema declares. Integers stay
/// integers: the parameter types reject `12.0` where they expect `12`.
fn typed(key: &str, val: &str, schema: &Value) -> Result<Value, String> {
    Ok(match schema_type(schema) {
        "integer" => Value::from(
            val.parse::<i64>()
                .map_err(|_| format!("{key}={val:?} is not a whole number"))?,
        ),
        "number" => Value::from(
            val.parse::<f64>()
                .map_err(|_| format!("{key}={val:?} is not a number"))?,
        ),
        "boolean" => Value::Bool(match val {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => return Err(format!("{key}={val:?} is not true or false")),
        }),
        "array" | "object" => serde_json::from_str(val).map_err(|e| format!("{key} takes JSON: {e}"))?,
        _ => Value::String(val.to_string()),
    })
}

/// Merge `--args` (a JSON object, or `-` for stdin) with the pairs, typing each
/// pair by the tool's schema and refusing keys the tool does not take, so a
/// typo fails loudly instead of being silently ignored.
pub(crate) fn build_args(
    tool: &Tool,
    args_json: Option<&str>,
    pairs: &[(String, String)],
    stdin: &mut dyn Read,
) -> Result<Map<String, Value>, String> {
    let mut out = match args_json {
        None => Map::new(),
        Some(src) => {
            let raw = if src == "-" {
                let mut s = String::new();
                stdin.read_to_string(&mut s).map_err(|e| format!("reading --args from stdin: {e}"))?;
                s
            } else {
                src.to_string()
            };
            match serde_json::from_str::<Value>(&raw) {
                Ok(Value::Object(m)) => m,
                Ok(_) => return Err("--args must be a JSON object".into()),
                Err(e) => return Err(format!("--args is not JSON: {e}")),
            }
        }
    };
    let schema = Value::Object((*tool.input_schema).clone());
    let props = schema["properties"].as_object().cloned().unwrap_or_default();
    for (k, v) in pairs {
        let Some(prop) = props.get(k) else {
            let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
            keys.sort_unstable();
            return Err(format!("{} takes no argument {k:?}; it takes: {}", tool.name, keys.join(", ")));
        };
        out.insert(k.clone(), typed(k, v, prop)?);
    }
    for req in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !out.contains_key(req) {
            return Err(format!("{} needs {req}=…", tool.name));
        }
    }
    Ok(out)
}

async fn run_cmd(
    client: &Client,
    args: &[String],
    stdin: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<i32, Usage> {
    let parsed = parse_run(args).map_err(Usage)?;
    let tools = list_tools(client).await?;
    let Some(tool) = tools.iter().find(|t| t.name == parsed.tool) else {
        return Err(Usage(format!(
            "unknown tool {:?} (it may be hidden by GITLAB_READ_ONLY, DISABLED_TOOLS or GITLAB_TOOLSET); `gl-mcp tools` lists what is available",
            parsed.tool
        )));
    };
    let call_args = build_args(tool, parsed.args_json.as_deref(), &parsed.pairs, stdin).map_err(Usage)?;
    let call = client.call_tool(CallToolRequestParams::new(tool.name.clone()).with_arguments(call_args.clone()));
    let (ok, text) = match tokio::time::timeout(parsed.timeout, call).await {
        Err(_) => (false, format!("timed out after {}s", parsed.timeout.as_secs())),
        Ok(Err(e)) => (false, format!("tools/call failed: {e}")),
        Ok(Ok(r)) => {
            let text = r
                .content
                .iter()
                .filter_map(|c| c.as_text())
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (r.is_error != Some(true), text)
        }
    };
    if parsed.json {
        // Stable field names a scheduler can rely on (same shape as slk-mcp's).
        let record = serde_json::json!({
            "tool": parsed.tool,
            "args": Value::Object(call_args),
            "ok": ok,
            "text": text,
        });
        let _ = writeln!(out, "{record}");
    } else {
        let _ = writeln!(out, "{text}");
    }
    Ok(if ok { EXIT_OK } else { EXIT_TOOL_ERROR })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn tool(schema: Value) -> Tool {
        Tool::new("get_pipeline", "Get a pipeline.", schema.as_object().unwrap().clone())
    }

    #[test]
    fn flags_may_sit_anywhere_among_pairs() {
        let r = parse_run(&s(&["get_pipeline", "project_id=g/p", "--json", "pipeline_id=12", "--timeout", "2m", "--args={}"])).unwrap();
        assert_eq!(r.tool, "get_pipeline");
        assert_eq!(r.pairs, vec![("project_id".into(), "g/p".into()), ("pipeline_id".into(), "12".into())]);
        assert!(r.json);
        assert_eq!(r.timeout, Duration::from_secs(120));
        assert_eq!(r.args_json.as_deref(), Some("{}"));
        assert!(parse_run(&s(&[])).is_err() && parse_run(&s(&["--json"])).is_err());
        assert!(parse_run(&s(&["t", "--bogus"])).unwrap_err().contains("unknown flag"));
        assert!(parse_run(&s(&["t", "novalue"])).unwrap_err().contains("not key=value"));
        assert!(parse_run(&s(&["t", "--timeout", "soon"])).is_err());
    }

    #[test]
    fn pairs_are_typed_by_the_schema_and_validated() {
        let t = tool(json!({
            "type": "object",
            "properties": {
                "project_id": {"type": "string"},
                "pipeline_id": {"type": "integer"},
                "per_page": {"type": ["integer", "null"]},
                "draft": {"type": ["boolean", "null"]},
                "files": {"type": "array"},
                "ratio": {"anyOf": [{"type": "number"}, {"type": "null"}]}
            },
            "required": ["project_id", "pipeline_id"]
        }));
        let pairs = |v: &[(&str, &str)]| v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect::<Vec<_>>();
        let a = build_args(
            &t,
            None,
            &pairs(&[("project_id", "g/p"), ("pipeline_id", "12"), ("per_page", "5"), ("draft", "true"), ("files", r#"[{"x":1}]"#), ("ratio", "0.5")]),
            &mut std::io::empty(),
        )
        .unwrap();
        assert_eq!(Value::Object(a), json!({"project_id": "g/p", "pipeline_id": 12, "per_page": 5, "draft": true, "files": [{"x": 1}], "ratio": 0.5}));
        // An integer stays an integer on the wire, never 12.0.
        let b = build_args(&t, Some(r#"{"project_id":"g/p"}"#), &pairs(&[("pipeline_id", "12")]), &mut std::io::empty()).unwrap();
        assert_eq!(serde_json::to_string(&b["pipeline_id"]).unwrap(), "12");
        // --args from stdin; pairs override it.
        let mut stdin = std::io::Cursor::new(r#"{"project_id":"x","pipeline_id":1}"#);
        let c = build_args(&t, Some("-"), &pairs(&[("pipeline_id", "2")]), &mut stdin).unwrap();
        assert_eq!(c["project_id"], "x");
        assert_eq!(c["pipeline_id"], 2);

        let err = |p: &[(&str, &str)]| build_args(&t, None, &pairs(p), &mut std::io::empty()).unwrap_err();
        assert!(err(&[("projct_id", "g/p")]).contains("it takes: draft, files, per_page, pipeline_id, project_id, ratio"));
        assert!(err(&[("project_id", "g/p")]).contains("needs pipeline_id"));
        assert!(err(&[("project_id", "g/p"), ("pipeline_id", "12.5")]).contains("not a whole number"));
        assert!(err(&[("project_id", "g/p"), ("pipeline_id", "1"), ("draft", "maybe")]).contains("true or false"));
        assert!(build_args(&t, Some("[1]"), &[], &mut std::io::empty()).unwrap_err().contains("JSON object"));
    }

    #[test]
    fn durations_and_command_detection() {
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        assert!(parse_duration("5d").is_err());
        assert!(is_headless(&s(&["run", "x"])) && is_headless(&s(&["tools"])));
        assert!(!is_headless(&s(&["--transport", "http"])) && !is_headless(&s(&[])));
    }
}
