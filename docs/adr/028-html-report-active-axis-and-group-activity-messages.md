# ADR 028: HTML report AI-Active axis; commit titles in group activity

## Status

Accepted (2026-07-03)

## Context

Two items remained open in `tasks.md`:

1. ADR 027 fixed the **text** `get_ai_adoption` roll-up to count usage evidence,
   but the **HTML** `generate_ai_adoption_report` — the artifact leadership
   slides are built from — still keyed its "Adopting (L1+)" card and By-Team
   table on config markers only, inheriting the same undercount.
2. `get_group_activity` aggregated pushes/commits/MRs as counts only, so callers
   could not do text correlation (e.g. find pushes whose commits reference an
   issue ID) without falling back to per-project `list_commits` calls.

## Decision

1. **HTML report mirrors the two axes.** A new **AI-Active** summary card
   (`is_active()`: markers OR usage evidence) sits next to the marker-based card,
   which is renamed **Configured (L1+)** to say what it actually measures. The
   By-Team table gains an **AI-Active** column; when a team's AI-Active exceeds
   its Configured count, the cell is highlighted green — that gap is precisely
   the usage-without-config adoption the old number missed. The naming avoids
   colliding with the existing "Active Repos" card (= scanned, non-dormant).
   The Methodology section defines both axes. Levels/funnel stay marker-based.

2. **`get_group_activity(include_commit_messages: bool = false)`.** When set,
   each member's line is followed by indented `branch: title` entries — the
   **head-commit title of each push event**, capped at 10 per member. This data
   is already present in the events payload, so it costs **zero extra API
   calls**. Trade-off, stated plainly: push events carry only the head commit's
   title, not every commit in the push; for exhaustive per-commit messages,
   `list_commits(all_branches=true)` per project remains the tool.

## Consequences

- The report and the text tool now tell the same story; slides no longer
  understate adoption, and the green highlight points directly at repos worth a
  "add CLAUDE.md" nudge.
- Issue-ID correlation over group activity works in one call for the common
  case (head commits), with a documented escape hatch for the exhaustive case.
- Both changes are re-aggregation/formatting only — no new scanning or API load.
