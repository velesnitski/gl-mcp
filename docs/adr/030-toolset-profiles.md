# ADR 030: Toolset profiles — prune tools/list, not just calls

## Status

Accepted (2026-07-03)

## Context

A token-effectiveness audit measured the two costs of a 99-tool server:

- **Responses** are lean: ~1.8 KB (~450 tokens) per call averaged over 838 real
  calls; only 1.9% exceed the 15 KB warn threshold. No action needed.
- **The `tools/list` schema payload is ~77 KB (~20k tokens)** — descriptions,
  431 parameter fields, JSON-schema structure. Clients that load all schemas up
  front pay this **every session**. (Claude Code defers schemas via on-demand
  lookup, so the primary client was already unaffected — but other MCP hosts
  are not.)

Separately, `DISABLED_TOOLS` and `GITLAB_READ_ONLY` only gated tools at **call
time**: the tools still appeared in `tools/list`, costing schema tokens and
inviting the model to attempt calls that would be rejected.

## Decision

Prune the tool router **at construction** (`build_router`), so filtered tools
are absent from `tools/list` itself. One mechanism serves three filters:

1. **`GITLAB_TOOLSET`** — `full` (default, everything), `core` (~33 everyday
   dev-workflow tools chosen from usage analytics: navigate/read/search,
   issues, MRs, commits, CI, basic writes), or an explicit comma-separated
   tool list. Unknown names in a custom list produce a startup warning.
2. **`DISABLED_TOOLS`** — now removed from the listing, not just rejected.
3. **`GITLAB_READ_ONLY`** — write tools now hidden, not just guarded.

Call-time guards (`write_guard!`) are kept as defense in depth. Startup logs
`toolset 'core' — exposing 33/99 tools` so the active surface is visible.

Two tests keep the profile honest: every `CORE_TOOLS` name must resolve to a
registered route (catches stale entries when tools are renamed), and pruning
counts are asserted for each filter.

## Consequences

- `core` cuts the schema payload ~70% for full-schema clients and shrinks the
  model's tool-choice surface to the everyday set.
- A read-only deployment no longer advertises tools it will refuse — better
  token economics *and* fewer doomed call attempts.
- Behavior change (intentional): calling a pruned tool now yields rmcp's
  generic "tool not found" instead of the tailored read-only/disabled message.
  Acceptable — the model no longer sees those tools, so such calls should not
  be generated at all.
- `full` remains the default; nothing changes for existing deployments unless
  they opt in.
