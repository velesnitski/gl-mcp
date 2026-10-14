# ADR 062: History queries without a clone

## Status

Accepted (2026-10-14)

## Context

Anthropic's write-up on redesigning its source control for agents ("Redesigning source
control", anthropic.com/engineering) argues that most automated work should not need
a working copy. Instead, history queries such as blame, merge-base, bisect and revert
should be API calls that a job makes directly.

Most of that article is about Anthropic's own infrastructure: a custom VCS,
server-side commits, a tiered cache, and dropping the merge queue. None of that
applies to a GitLab MCP server, and it is deliberately not adopted here. The
API-first point does apply.

gl-mcp already reads file windows (ADR 054), compares refs, reverts and rebases. It
had no blame and no merge-base, so an agent asking "who last changed these lines, and
why?" or "where did these branches diverge?" had to clone the repository or guess.
GitLab answers both with one request each.

## Decision

- **`get_blame`** reads a line window of a file's blame. A start line alone gives
  100 lines, matching the windowed file reads. The output shows:
  - a header with the full range;
  - lines per author;
  - each chunk as short SHA, date, author and subject line, followed by numbered
    lines.

  Output is capped at 300 lines and says so when it is cut. An invalid window
  (line 0, end before start) is rejected before any request is made.
- **`get_merge_base`** takes two or more comma-separated refs and returns the common
  ancestor. Fewer than two refs is a user error. The answer points to
  `compare_branches` for what changed on each side.

Bisect is not added. It needs a build or test run at each step, which is a pipeline
concern rather than a query.

## Consequences

- Two read tools are added, for 116 in total. Both are exercised by the outage sweep
  and recorded in the tool-surface contract (ADR 060).
- Rendering is pure (`render_blame`, `render_merge_base`) and pinned by tests and a
  golden snapshot.
