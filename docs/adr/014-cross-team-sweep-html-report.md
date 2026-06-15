# ADR 014: Cross-team spec-drift HTML report

## Status

Accepted (2026-06-15)

## Context

`sweep_spec_audit` rolls several teams' specs into one markdown table. For a
"full teams" report — something leadership reviews or pastes into a deck — that
should be the same clickable, dark-theme HTML artifact every other gl-mcp report
produces, with per-team drill-down rather than just counts.

## Decision

Add `generate_sweep_report(targets[])`. It reuses `compute_audit` per target
(same concurrent, chunk-capped, failure-isolated execution as `sweep_spec_audit`,
and the same `map_key`-by-label snapshot discipline), then renders one HTML page:

- **Summary cards** — teams, stale versions, total drift, stale-doc,
  undocumented, secrets (with hardcoded count).
- **By-team table** — one row per team (version verdict, cleanup/drift/stale/
  undoc/secrets/in-sync), each linking to that team's anchored detail; `~` marks
  search-harvested (approximate) reverse-drift.
- **Needs attention** — teams with cleanup, drift, a stale version, or a
  hardcoded secret.
- **Per-team `<details>`** — version line, drift and stale-doc routes,
  undocumented endpoints (first 15, with GitLab blob links), masked secrets.

The dark-theme head is factored into a shared `html_head(title)` helper.

## Consequences

- Teams that fail to audit (bad repo/ref) become a "failed to audit" row, not a
  failed report — same resilience as the markdown sweep.
- Per-team detail caps undocumented at 15 with an "… and N more" note to keep the
  page readable; the full list is one drill-down (single-team report) away.
- Specs are still caller-supplied per target. Org teams without an API route spec
  (devops, node, qa, wordpress) aren't auditable this way — the report covers the
  API-touching teams (the app platforms, backend, web frontend). That scope is a
  property of what specs exist, not a tool limit.
