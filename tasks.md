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

### `list_merge_requests` — expose descriptions in summary mode

Currently `list_merge_requests(summary_only=true)` returns
`{project}!{iid}|{state}{draft}|{author}|{title}` — title only, no
description. For downstream parsing (e.g. extracting YouTrack/JIRA IDs
from MR descriptions) we have to fetch each MR individually with
`get_merge_request`, which is N+1.

**Possible fix:** add `include_descriptions: bool = false` param. When
true, return full description bodies even in summary mode. Tradeoff:
defeats some of the token-saving purpose of summary_only, so opt-in.

**Priority:** Low — workaround exists.

### `get_group_activity` — return per-commit messages

Currently aggregates pushes/commits/MRs as counts. Doesn't surface raw
commit messages, so can't do text-based correlation (e.g. finding
commits that reference an issue). Use `list_commits(all_branches=true)`
per-project as a workaround.

**Possible fix:** add `include_commit_messages: bool = false` param.

**Priority:** Low — workaround exists.
