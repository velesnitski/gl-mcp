# ADR 031: Path-traversal review and snapshot-filename hardening

## Status

Accepted (2026-07-09)

## Context

Sibling project yt-mcp published **GHSA-99mq-fjjc-6v9j** (CVSS 7.5): its
`add_attachment` tool accepted a caller-controlled `file_path`, read the local
file with only an existence check, and uploaded the bytes to YouTrack — an
arbitrary-local-read + external-egress channel exploitable via a malicious MCP
client or prompt injection. A lower-severity write traversal existed in
`get_project_health`, where a `project` argument flowed into a snapshot filename.

We audited gl-mcp for the same two classes.

## Findings

- **Read-and-exfiltrate class (the high-severity one): absent.** No gl-mcp tool
  reads a caller-controlled *local* file. `update_file` takes `content` directly
  (never opens a local path); `get_file_content` / `analyze_file` read from the
  *remote* GitLab repository via URL-encoded API paths
  (`/projects/:id/repository/files/:path`). The local↔remote bridge that made
  `add_attachment` dangerous does not exist here.
- **Local writes from caller input: one, already safe.** Spec-audit snapshots
  are written to `~/.gl-mcp/spec_maps/{project}__{ref}[__{key}].json`. The prior
  sanitizer replaced `/`, `\`, and space with `_`, so no path separator survived
  and traversal was already impossible. All other local FS paths
  (`teams.json`, `instance_id`, `analytics.log`) are fixed, not caller-derived.

gl-mcp is **not affected** by GHSA-99mq-fjjc-6v9j.

## Decision

Harden the one look-alike spot as defense-in-depth, converting the snapshot
sanitizer from a **denylist** ("replace known-bad chars") to an **allowlist by
construction**: `safe_component` keeps only `[A-Za-z0-9._-]`, maps everything
else to `_`, and strips leading `.` (so a token can neither contain a separator
nor form a `.`/`..` dotfile). The snapshot therefore lands inside `spec_maps/`
for *any* input. Pinned by tests (`snapshot_path_is_traversal_proof`,
`safe_component_allowlist`) that feed hostile inputs across all three tokens.

## Consequences

- The safety is now obvious from the sanitizer itself, not an emergent property
  of replacing three specific characters — robust against future refactors.
- The filename scheme changed slightly, so the first spec audit after upgrade
  finds no prior snapshot and re-baselines (a one-time "no changes since last
  audit" reset for that feature). Acceptable for a security-hardening patch.
- Documents, for future contributors, the architectural rule that keeps gl-mcp
  clear of this class: **tools read/write the remote repo, never the caller's
  local filesystem by path.** Any future tool that would read a local path must
  confine it (allowlisted roots + `realpath`/`commonpath`), per the yt-mcp fix.
