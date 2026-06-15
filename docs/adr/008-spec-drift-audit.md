# ADR 008: Spec-drift audit (route + version)

## Status

Accepted (2026-06-15)

## Context

Teams keep per-platform "app spec" docs in a knowledge base: a table of API
routes (often annotated with usage status — active vs "remove / no longer
used"), a documented app version, plus config metadata. These docs rot. Routes
get deleted from code but linger in the doc; routes flagged "to remove" stay
wired in for months; the documented version drifts behind the shipped tag.
Nobody notices because cross-referencing a wiki table against a codebase by
hand is exactly the tedious work that never happens.

A probe confirmed the premise is tractable: documented route paths can be found
in code with the existing blob-search, and the first three routes tested
already showed real drift (two routes the doc flagged for removal were already
gone from code; one active route was correctly present).

## Decision

Add a read-only tool `audit_spec_drift(project_id, spec, ref_name?,
summary_only?)`.

**Boundary.** gl-mcp is a GitLab MCP; it does not fetch the doc. The caller
supplies the spec markdown via `spec` (fetched with whatever KB tool they use)
and gl-mcp does the GitLab-side analysis. The cross-tool glue lives at the
agent/skill layer, not in this codebase.

**Route drift.** Parse the spec's ROUTES section into normalized paths
(template tokens like `{{api}}`, query strings, and `(identifier)`
interpolation stripped; multi-path cells split on `<br>`). Each row's usage
status is read from deprecation markers in the row text (English + the Russian
annotations the docs actually use, e.g. "убрать", "не используется"). Each route
is code-searched and classified:

| doc status | in code | verdict |
|---|---|---|
| flagged-remove | yes | **cleanup-debt** (still wired — the backlog) |
| flagged-remove | no | **stale-doc** (safe to delete the row) |
| active | no | **drift** (renamed or dropped — investigate) |
| active | yes | in-sync |

Paths too generic to match reliably (single common words like `/login`) are
**not guessed** — they go to a "needs review" bucket. Multi-segment paths search
the last two segments, robust to prefix differences between doc and code.

**Version drift.** Parse the documented version, compare against the repo's
latest tag (numeric semver compare, `v` prefix tolerated). The plist/Info file
is deliberately *not* used as the source of truth — versions there are build
variables (`$(MARKETING_VERSION)`), not literals.

## Consequences

- The `RouteAudit` set this tool computes is the natural persistence unit for a
  future **local metadata map**: serialize it after the first run, and
  run-over-run diffs ("what drifted since last audit") plus reverse-drift
  ("endpoints in code the spec never documented") layer on top without
  re-deriving the parser. Reverse-drift needs a code-side endpoint inventory,
  which basic blob-search can't enumerate — that's the map's job, deferred to a
  v2.
- Blob search is substring-based and only covers the searched ref; short or
  common paths over-match, hence the needs-review bucket and the
  last-two-segments heuristic. The tool reports match locations so a human
  confirms rather than trusting a bare boolean.
- Parsing a semi-structured wiki table is heuristic. The parser is tolerant and
  the report shows what it couldn't classify rather than dropping rows silently.
- Security cross-referencing (secrets pasted into the spec, and whether they're
  also hardcoded in code) is a natural third check, deliberately out of scope
  for this first cut — it ships next.
