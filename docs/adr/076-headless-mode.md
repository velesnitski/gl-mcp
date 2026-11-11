# ADR 076: Headless mode — run any tool once from the command line

## Status

Accepted (2026-11-11)

## Context

gl-mcp answers only inside an MCP session, apart from two report-specific flags
(`--adoption-report`, `--dependency-audit`). A scheduled job — a weekly email, a
CI step, a bot that checks pipelines every hour — cannot use the other tools
without an MCP client, and re-implementing them per consumer would fork the
logic. Each new one-shot use would otherwise need another flag.

## Decision

Two subcommands on the same binary, with the interface of the sibling Slack
server's headless mode, so one scheduler drives both:

```
gl-mcp tools [--json]
gl-mcp run <tool> [key=value ...] [--args JSON|-] [--json] [--timeout 5m]
```

1. **The real protocol, in-process, nothing per tool.** The command starts the
   server on one end of an in-memory pipe and a real MCP client on the other, and
   sends `tools/list` and `tools/call`. Every call goes through the same
   serialization, routing, wrappers and gating as a session: `GITLAB_READ_ONLY`,
   `DISABLED_TOOLS` and `GITLAB_TOOLSET` apply exactly as they do for a client, so
   a write tool under read-only does not exist. A tool is reachable the day it is
   registered.
2. **Arguments typed by the schema.** `key=value` pairs become integers, numbers,
   booleans or JSON according to the tool's input schema, looking through nullable
   types. Integers stay integers on the wire: the parameter types reject `12.0`
   where they expect `12`. A key the tool does not take, a malformed value or a
   missing required argument is a usage error that names the valid keys. `--args`
   takes a JSON object, or `-` for stdin; pairs override it. Flags may sit
   anywhere among the pairs.
3. **Output for machines.** Plain text by default; `--json` prints one object with
   stable fields `tool`, `args`, `ok`, `text`.
4. **Exit codes:** 0 success, 1 the tool reported an error (GitLab refused,
   unreachable, not found, timed out), 2 a usage or configuration error. Logs go
   to stderr at `warn` unless `RUST_LOG` says otherwise, so stdout holds only the
   result.

## Consequences

- Schedulers get every tool without an MCP client or a second implementation. For
  read-only jobs, `GITLAB_READ_ONLY=1` removes write tools entirely rather than
  trusting the caller.
- One call per process; anything stateful between runs is the caller's to keep.
- The existing `--adoption-report` / `--dependency-audit` flags stay: they write
  several files (snapshot, full report) that a single tool result does not carry.
- The binary now links rmcp's client side as well as its server side.
