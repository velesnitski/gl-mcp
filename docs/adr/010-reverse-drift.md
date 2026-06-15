# ADR 010: Reverse drift — undocumented endpoints in code

## Status

Accepted (2026-06-15)

## Context

ADR 008/009 audit the spec → code direction (documented routes, version,
secrets). The reverse direction is the higher-value governance signal: endpoints
that exist *in code* but the spec never documented — shadow API surface that
escapes review. A documented endpoint that's gone is tidy-up; an undocumented
one that's live is the thing nobody is looking at.

The obstacle, noted when deferring this in ADR 008: GitLab blob-search can't
enumerate "every endpoint" — it answers queries, it doesn't list. So we need a
code-side endpoint inventory.

## Decision

Add reverse-drift to `audit_spec_drift` with two inventory sources:

1. **`routes_file` (precise).** When the caller names the file that defines the
   routes (e.g. a network-layer source file), fetch its raw content and harvest
   every quoted leading-slash path literal (`"/v3/user"`), normalizing out
   `\(interp)` / `{param}` placeholders and query strings, with line numbers.
   This is the reliable path and reports all undocumented endpoints including
   ones in entirely new namespaces.

2. **Search-harvested (fallback).** With no routes file, run a handful of seed
   searches (`return "/`, `"/v`, …) and mine each result snippet for path
   literals. This is noisy — filesystem and asset paths leak in — so the output
   is filtered to paths whose first segment is a namespace the spec already
   documents. The report states this mode and recommends `routes_file` for full
   coverage, since the filter deliberately can't see brand-new namespaces.

An endpoint counts as documented (not reverse-drift) when its **match key** —
the last two path segments — equals a documented route's key. This tolerates
prefix differences (code `/api/v3/user` vs doc `/v3/user`) symmetrically with
the forward direction.

Undocumented endpoints are added to the report, the `summary_only` line, and the
persisted metadata-map snapshot, so the map's "changes since last audit" diff
reports a shadow endpoint *appearing* or *resolving*.

## Consequences

- File mode is only as good as the file the caller names; the search fallback
  trades coverage for noise control. Both report file:line so a human confirms.
- The match-key (last two segments) can in theory mark a genuinely distinct
  endpoint as documented when two routes share a tail (`/v3/user` vs
  `/admin/user`). Accepted: under-reporting reverse-drift is safer than crying
  wolf, and the forward direction already uses the same key.
- Harvesting only finds path *literals*. Endpoints assembled from fragments
  (`base + "/" + name`) won't be seen — a known blind spot, not a silent one.
- This closes the spec-audit roadmap (forward drift, version, security, map,
  reverse drift). Further depth (fragment-assembled routes, multi-platform
  sweep) is additive, not foundational.
