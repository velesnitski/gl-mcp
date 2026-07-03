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

### `generate_ai_adoption_report` (HTML) — mirror the active/configured split

The HTML report's "Adopting (L1+)" summary card and team table are still
marker-based (`level >= 1`), so they inherit the same undercount that task #2
fixed in the text `get_ai_adoption`. Leadership slides are built from this
report, so it should gain an **Active** card/column alongside "Adopting" using
the same `is_active()` helper (already in `adoption.rs`).

**Priority:** Moderate — same evidence, re-aggregation only; separate render path.

## ~~2. `get_ai_adoption` — count usage-evidence in the adoption roll-up~~ ✅ DONE (2026-07-03)

Implemented in v0.35.0. The per-team table now has both an **Active** column
(config markers OR any usage evidence — AI-trailed commits on any branch incl.
squash-hidden, `.tasks/` commits, AI-marked MRs) and a **Configured** column
(marker-based, as before). The `summary_only` headline is now
`active/repos · configured (best L…)`. Level/"best level" stays marker-based
(config quality is a separate axis). Covered by `test_active_counts_usage_without_config`.

Follow-up (below): the HTML report `generate_ai_adoption_report` still shows the
marker-based "Adopting (L1+)" figure and inherits the same undercount.

**Symptom (live scan 2026-07-03, 79 repos):** the per-team summary counts a
repo as "adopting" only when it has config markers (CLAUDE.md / agents /
skills). Commit-trail evidence (Co-Authored-By trailers) lands in a separate
"Invisible usage" section and is NOT counted. Result: a team can show
"0/6 adopting" while one of its repos has 100% AI-trailed commits in 30d —
the headline undercounts real adoption, and leadership slides built from the
summary inherit the undercount.

Same class: "squash-hidden" repos (trailers on feature branches, stripped at
merge) are detected and listed but also excluded from the roll-up.

**Fix sketch:**
- Split the team table into two columns: `configured` (marker-based, as now)
  and `active` (markers OR AI-trailed commits on any branch in the window).
- Team headline becomes `active/repos`, with `configured` as the secondary
  number. Existing "best level" stays marker-based (config quality is a
  separate axis from usage).
- Optional: new level tag `U` (usage-without-config) in the adopting-repos
  table so those repos appear in the main list with a "add CLAUDE.md" hint
  instead of a side section.

**Why:** measurement should reflect reality; config nudges stay visible as
hygiene flags, not as gatekeepers of the adoption number.

**Effort:** moderate — the evidence is already collected; this is
re-aggregation + table shape. Tests: one fixture repo with trailers-only,
assert it lands in `active` and not `configured`.
