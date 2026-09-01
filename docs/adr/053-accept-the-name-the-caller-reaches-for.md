# ADR 053: Accept the parameter name the caller reaches for

## Status

Accepted (2026-09-01)

## Context

`list_commits` names its ref selector `branch`. Its siblings `search_code` and
`list_pipelines`, and the GitLab API underneath all of them, name the same thing
`ref_name` — and `list_commits` itself maps `branch` straight onto GitLab's `ref_name`
query parameter internally. A caller who reasonably passed `ref_name` to `list_commits`
hit the worst possible outcome: serde dropped the unknown field, the call fell back to
the default branch, and a plausible commit list came back with no error. A branch audit
built on it reached a confident wrong conclusion.

This is the same defect class the recent releases have chased — a state the tool cannot
honour rendered as one the reader recognises. Here it is a naming seam: two names for
one concept across sibling tools, and a silent drop when the wrong one is used.

Two fixes were possible and both are wrong. Denying unknown fields makes the drop loud,
but MCP clients legitimately add fields of their own, so a hard failure would break
valid calls. Adding a separate `ref_name` parameter that maps to the same query string
would leave two parameters for one concept and invite passing both.

## Decision

`branch` accepts `ref_name` as a serde alias. The canonical name is unchanged, the
internal mapping to GitLab's `ref_name` is unchanged, and a caller who uses either name
is honoured. The parameter description states the alias so it is discoverable rather
than folklore. A deserialization test asserts that `ref_name` populates `branch`, which
is the assertion the silent-drop bug would fail.

## Consequences

- The natural name works instead of being discarded, and the fleet reads consistently
  from the caller's side without renaming a shipped parameter.
- Method note, eighth in the series: the fix for an unobservable-state defect is not
  always to surface the state — sometimes it is to remove the seam that produced it. A
  parameter the caller keeps reaching for is evidence about the name, not the caller.
