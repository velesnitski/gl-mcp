# gl-mcp pending feature requests

## ~~1. `list_commits` — add `all_branches: bool = false` parameter~~ ✅ DONE (2026-05-19)

Implemented in commit `<see git log>`. The `list_commits` tool now accepts
an `all_branches` boolean parameter that maps to GitLab's `?all=true`
query string. Mutually exclusive with `branch` (branch wins if both set).

Use case from `velesnitski/youtrack-reports` (`scripts/yt_pulse.py`) is
now unblocked — Pulse can correlate feature-branch commits with YouTrack
issue IDs without falling back to direct REST calls.

---

## Other observations (not blocking)

### ~~`list_merge_requests` — expose descriptions in summary mode~~ ✅ DONE (2026-05-19)

Added `include_descriptions: bool` opt-in param. When true with
`summary_only=true`, returns indented description lines under each MR.

### `get_group_activity` — return per-commit messages

Currently aggregates pushes/commits/MRs as counts. Doesn't surface raw
commit messages, so can't do text-based correlation (e.g. finding
commits that reference an issue). Use `list_commits(all_branches=true)`
per-project as a workaround.

**Possible fix:** add `include_commit_messages: bool = false` param.

**Priority:** Low — workaround exists.
