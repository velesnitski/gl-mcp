# ADR 013: Multi-platform spec-drift sweep

## Status

Accepted (2026-06-15)

## Context

App specs are per-platform (one article each for iOS, Android, Windows, Mac).
Auditing them one tool call at a time is tedious and gives no cross-platform
view — yet the platforms are exactly comparable (same questions: version drift,
cleanup debt, shadow endpoints, leaked secrets). A single rolled-up table is the
natural governance artifact.

The platforms are fully independent — different repo, spec, and routes file — so
they can audit concurrently.

## Decision

Add `sweep_spec_audit(targets[])`. Each target is `{label, project_id, spec,
ref_name?, routes_file?}`. The sweep:

- Runs `compute_audit` per target **concurrently**, in chunks of
  `SWEEP_CONCURRENCY` (3). Each audit already fans out ~10 sub-requests
  internally, so the chunk cap bounds total in-flight load rather than letting
  N platforms × 10 requests hit the API at once.
- Catches per-platform failure: a bad project or ref yields an error row, not a
  failed sweep — the other platforms still report.
- Reuses `compute_audit`, so each platform also persists its own metadata-map
  snapshot (per project+ref) as a side effect — the per-platform "changes since
  last audit" history keeps accruing.

The rollup (`render_sweep`, pure/unit-tested) is a table — one row per platform
with version verdict and the cleanup/drift/stale/undoc/secrets/in-sync counts —
plus a "needs attention" list (platforms with cleanup, drift, hardcoded secrets,
or a stale version) and cross-platform totals.

## Consequences

- `SWEEP_CONCURRENCY` is a fixed cap, not a parameter — a deliberate simplicity
  choice; 3 platforms in flight is a reasonable ceiling for the per-audit fan-out.
- The sweep returns markdown; an HTML rollup can follow the same
  compute-then-render split if wanted, reusing `render_html` per platform behind
  a combined page.
- Specs are supplied by the caller (one per target), keeping gl-mcp GitLab-only
  and consistent with the single-target tools — fetching the per-platform
  articles is orchestrated at the agent layer.
