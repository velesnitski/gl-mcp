# 002 – Task-Directory Commit Activity as AI Usage Evidence

## Status
Accepted

## Context
ADR 001 scores "active usage" purely from `Co-Authored-By` AI trailers in recent
commits. Real-world scans showed teams that demonstrably run Claude Code agents
(live `.tasks/` state files, maintained `.claude/` configs) but whose commits
carry no trailer — attribution is disabled in their settings or stripped by
squash merges. These repos were stuck at L2 and mislabeled "setup unused".

Options considered:

1. **Path-scoped commit counting** – for repos that already show a `.tasks` or
   `.claude` marker, count recent commits touching those paths
   (`GET /repository/commits?path=...&since=...`) and treat `.tasks` activity
   as usage evidence.
2. **Lower the trailer threshold / keep trailer-only** – rejected: no threshold
   fixes a signal that is literally absent when attribution is off.
3. **Scan task-file contents for freshness** – fetch and parse dated task files.
   Rejected: many extra API calls per repo, format-fragile, and commit history
   on the path proves the same thing more cheaply.

Constraint: no extra API calls for unmarked repos — the group scan must stay
within its current request budget for the common case.

## Decision
Extend `RepoMarkers` with `tasks_recent_commits` and `claude_recent_commits`,
fetched only when the corresponding marker (`.tasks` dir, `.claude` dir) exists.

- `has_active_usage` = `ai_pct >= 10%` OR `tasks_recent_commits > 0`.
  `.claude` activity alone is config maintenance, not usage.
- L3 becomes `agents_count > 0 && has_active_usage` — the skills requirement is
  dropped (skills are a marker, not a gate).
- "setup unused" fires only when agents exist AND there are zero AI commits AND
  zero recent `.tasks`/`.claude` commits.
- New flag "no attribution": `.tasks`/`.claude` activity with zero AI-trailed
  commits; output recommends standardizing Co-Authored-By trailers.
- The Adopting Repos table appends `+N task commits` so untrailed activity is
  visible next to the AI-commit percentage.

## Consequences
- Easier: teams with attribution disabled are scored correctly (L3) and get an
  actionable "enable attribution" recommendation instead of a false
  "setup unused" verdict.
- Harder/limits: up to 2 extra API calls per marked repo; `.tasks` commit
  counting caps at 20 (count display saturates, scoring only needs >0); a repo
  that commits task files manually without agents would still count — accepted,
  the `.tasks` convention is agent-specific in practice.
- Follow-up: none planned; if other agent-state paths emerge, add them to the
  same path-scoped counter.
