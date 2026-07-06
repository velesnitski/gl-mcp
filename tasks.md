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

### ~~`get_group_activity` — return per-commit messages~~ ✅ DONE (2026-07-03)

Implemented in v0.36.0 as `include_commit_messages: bool = false`. When set,
each member's line is followed by indented `branch: title` entries — the
head-commit title of each push event (cap 10/member), straight from the events
payload (zero extra API calls). Limitation, documented in ADR 028: push events
carry only the **head** commit's title; for exhaustive per-commit messages the
`list_commits(all_branches=true)` workaround remains the tool.

### ~~`generate_ai_adoption_report` (HTML) — mirror the active/configured split~~ ✅ DONE (2026-07-03)

Implemented in v0.36.0. New **AI-Active** summary card (config OR usage
evidence, via `is_active()`) next to the marker-based card, renamed
**Configured (L1+)**; new **AI-Active** column in the By-Team table, highlighted
green when it exceeds Configured (usage-without-config repos). Methodology
defines both axes. Named "AI-Active" to avoid colliding with the existing
"Active Repos" (= scanned, non-dormant) card. See ADR 028.

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
