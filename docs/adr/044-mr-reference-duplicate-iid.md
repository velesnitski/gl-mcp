# ADR 044: Don't append the IID to a reference that already has one

## Status

Accepted (2026-07-30)

## Context

`list_merge_requests` in `summary_only` mode emitted lines like:

```
group/project!42!42|opened|author|assignee|title
```

The compact line was built as `format!("{project}!{iid}|…")` where `project` came
from GitLab's `references.full`. That field is the **full reference and already
includes the IID** (`group/project!42`), so appending `!{iid}` duplicated it.

The verbose path was always correct — it prints `references.full` as-is — so the two
output modes disagreed, and only the compact one was wrong. It surfaced during a
post-upgrade smoke test of the API surface; it is a long-standing formatting defect,
not an upgrade regression.

Beyond looking wrong, the duplicated form is not a valid GitLab reference: anything
parsing the summary line to recover a project path and IID gets a malformed value.

## Decision

Extract `mr_reference(mr, iid)`: return `references.full` when present and non-empty,
otherwise fall back to a bare `!iid`. Use it for the summary line. Pinned by two
tests — one asserting no duplication, one covering the missing/empty fallback.

## Consequences

- Compact and verbose outputs now agree on how an MR is identified.
- The summary line is machine-parseable again.
- Small general point: when an upstream field is *already* a composed identifier,
  composing it again is easy to miss by eye — the fix belongs in one named helper
  with a test, not inline in a format string.
