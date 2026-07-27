# ADR 043: Single-repo adoption scans + group-wide code search

## Status

Accepted (2026-07-27)

## Context

Two tools were scoped to exactly one level of the hierarchy, and in both cases the
missing level was the one real work needed.

**1. `get_ai_adoption` was group-only.** Asked "how is *this* repo doing on AI
adoption?", the tool could not answer — the only entry point took a group path. The
practical fallback was to inspect `.claude/`, `CLAUDE.md`, `docs/adr` and the commit
log by hand and eyeball a verdict: the tool's own logic, re-done manually and less
consistently. Internally the scan is already per-repo (`scan_repo`); only the
enumeration assumed a group.

**2. `search_code` was project-only.** A sweep for a string across an org — a rename,
or checking where a stale identifier still appears — had to be run repo by repo, so
coverage was whatever handful of repos someone remembered to check. "Where else does
this appear?" was effectively unanswerable.

## Decision

**1. `get_ai_adoption` / `generate_ai_adoption_report` accept a project path.** If the
group listing yields nothing, the path is resolved as a project and scanned as a
one-element list; every downstream step (markers, commits, benchmark, flags) is
unchanged. No new parameter — the existing path argument accepts either, so there is
nothing new to learn. If the path is neither a group nor a project, the original
error is preserved rather than masked.

**2. `search_code` gains `group_path`** for a whole-group sweep, implemented as a
**per-project fan-out** (bounded concurrency, non-archived projects, most-recently-
active first) rather than GitLab's `/groups/:id/search`.

That choice is the substance of this ADR. Group-level *blob* search requires advanced
search (Elasticsearch); on an instance without it the endpoint returns an **empty
list** — indistinguishable from "no matches". For a rename or leaked-string sweep, a
confident empty on an unsupported query is the worst possible answer: it reports
"clean" when nothing was actually searched. The fan-out works on every instance.

Two safeguards follow from the same reasoning: a per-repo error yields no hits for
that repo instead of aborting the sweep, and the repo cap (60) is **always disclosed**
in the output when it truncates — a partial sweep reported as complete would
reintroduce exactly the false-confidence problem the fan-out exists to avoid.

`project_id` becomes optional (either it or `group_path` is required); existing calls
are unaffected.

## Consequences

- "How is this repo doing?" is answerable by the tool instead of by hand.
- Org-wide string sweeps are one call, with honest coverage reporting.
- The fan-out costs one request per repo. That is the price of a correct answer on
  instances without advanced search; the cap and concurrency limit bound it.
- General rule, third instance this cycle (cf. ADR 038, ADR 040): **a tool that
  answers confidently from incomplete data is worse than one that refuses.** Prefer
  the mechanism that always works, and disclose every limit that remains.
