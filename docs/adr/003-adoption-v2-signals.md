# 003 – Adoption v2: Squash-Proof and Pre-Adoption Signals

## Status
Accepted

## Context
ADR 001/002 scoring relies on default-branch commit trailers plus `.tasks`
path activity. Two systematic blind spots remained:

1. **Squash merges erase trailers.** GitLab's squash rewrites the merge commit
   message, so a team working entirely through AI-trailed feature-branch
   commits can show 0% AI commits on the default branch. The work is invisible
   exactly where the scan looks.
2. **Pre-adoption work is invisible.** A repo where AI experiments live only on
   feature branches (no CLAUDE.md merged yet) registers as L0 with no signals,
   even though adoption is actively in flight.

Secondary gaps: an ADR directory counts the same whether it is maintained or a
one-time import, and a CLAUDE.md written once and never updated keeps passing
the config check while drifting out of date.

Options considered for the squash problem: scanning merge-commit parents
(extra call per MR, fragile) vs. all-branch commit listing plus MR-description
matching. Chosen: the latter — `?all=true` is one parameter on an existing
call, and MR descriptions survive squash by design, making them the most
reliable usage signal available.

## Decision
Five new signals in `scan_repo`, all `.ok()/unwrap_or_default()` resilient:

1. **All-branch commit scan** — the existing commits call gains `all=true`;
   `ai_commits`/`total_commits` now mean all-branches. If trailers exist, one
   extra default-branch call fills `ai_commits_default`. Flag
   `squash-hidden usage` when trailers exist on branches but not on default.
2. **Branch radar** (every active repo, +1 call) — branch names matched against
   `(?i)(claude|copilot|llm|agentic|agent|ai[-_])` (compiled once via
   `LazyLock`), up to 3 hits stored. Repos whose ONLY signal is branch hits
   surface in a new "In-flight" output section with flag
   `in-flight (branch: …)` — level stays L0 but the pipeline is visible.
3. **MR description scan** (only repos with markers or branch hits) — MRs
   updated in the window whose description contains "generated with claude",
   "co-authored-by", or the robot emoji → `ai_mr_count`/`total_mr_count`.
   `has_active_usage` now also accepts `ai_mr_count > 0`, so squash-heavy
   repos can still reach L3.
4. **ADR cadence** (only when `adr_count > 0`, +1 call) — commits touching
   `docs/adr` in the window; markers render `ADR active(n)` vs `ADR stale`.
5. **Config staleness** (only when CLAUDE.md exists, +1 call) — last commit
   ever touching CLAUDE.md; flag `stale config (30+ commits behind)` when the
   touch predates the scan window and the window holds >= 30 commits (window
   count is a lower bound, hence "30+"). Staleness is precomputed into a bool
   during the scan so `quality_flags` stays a pure function of `RepoMarkers`.

A new pure `trajectory()` renders a per-repo direction column: "↑" building
(branch hits, or markers with recent `.claude`/`docs/adr` maintenance — wins
over decay), "↓" decaying (markers, no usage, no maintenance), "→" steady,
"" no signals.

## Consequences
- Easier: squash-heavy teams are scored on evidence that survives their merge
  strategy; managers see adoption *direction* (trajectory, in-flight pipeline)
  rather than only a snapshot; stale configs and dead ADR logs get called out.
- Harder/limits: worst case ~10 API calls per active repo (was ~7), still
  batched 10× concurrent; `quality_flags` returns owned `String`s now (the
  in-flight flag embeds a branch name); the branch regex can false-positive on
  e.g. `useragent-fix` — accepted, hits are capped at 3 and only ever add an
  "↑"/in-flight hint, never a level.
- Follow-up: none planned; if GitLab adds squash-message templating with
  trailer preservation, `ai_commits_default` could become a fidelity check.
