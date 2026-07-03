# ADR 027: Adoption roll-up counts usage, not just config

## Status

Accepted (2026-07-03)

## Context

`get_ai_adoption` measures two different things about a repo:

- **config quality** — CLAUDE.md, agents, skills, MCP config, ADRs → the L0–L3
  "adoption level"; and
- **actual usage** — AI-trailed commits, `.tasks/` activity, AI-marked MRs.

The per-team roll-up counted a repo as "adopting" only when `level >= 1`, which
requires a **config marker**. Usage-without-config (a repo with AI-trailed
commits but no CLAUDE.md) scored level 0 and was shown only in a side
"Invisible usage" section — excluded from the team headline. Squash-hidden usage
(trailers on feature branches, stripped at merge) was likewise detected but not
counted.

The result understated reality: a team could read **"0/6 adopting"** while one
of its repos had 100% AI-trailed commits in the window, and leadership slides
built from the summary inherited the undercount. Config presence was acting as a
gate on the usage number, though they are independent axes.

## Decision

Report **two axes** in the per-team table:

- **Configured** — has config markers (`level >= 1`, unchanged).
- **Active** — configured **OR** any usage evidence, via a new
  `RepoMarkers::is_active()` (`has_any_marker() || has_usage_evidence()`).
  `has_usage_evidence()` is broader than the L3 threshold: a single AI-trailed
  commit on **any** branch (so squash-hidden counts), a `.tasks/` commit, or an
  AI-marked MR is enough.

The `summary_only` headline becomes `active/repos · configured (best L…)`.
"Best level" stays marker-based — config quality remains a distinct, visible
axis, just not a gatekeeper of the adoption number.

## Consequences

- The headline reflects real adoption; config markers become a hygiene signal
  ("add a CLAUDE.md") rather than a precondition for being counted.
- Pure re-aggregation of already-collected evidence — no extra scanning.
- Covered by `test_active_counts_usage_without_config` (usage-only and
  squash-hidden repos are `active` but not configured; empty repos are neither).
- The HTML `generate_ai_adoption_report` still shows the marker-based
  "Adopting (L1+)" figure and needs the same split — tracked in `tasks.md`.
