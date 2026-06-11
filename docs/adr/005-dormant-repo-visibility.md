# 005 – Dormant Repo Visibility

## Status
Accepted

## Context
`scan_group` (ADR 004) skips repos with no activity in 180 days and reduced
them to a bare `dormant_count`. That hides actionable information the scan
already had in hand from the project listing: *which* repos are dormant,
*whose* they are, and *how long* they have been idle. Leadership reading the
By Team table could not tell a lean team from one sitting on a graveyard of
abandoned repos, and there was no archive-candidates list to act on. The
180-day threshold was also a hard-coded constant.

## Decision
1. **Keep the data, not the count.** `AdoptionScan.dormant_count: usize`
   becomes `dormant: Vec<DormantRepo>` where `DormantRepo { path, team,
   last_activity }` uses the same `team_of()` mapping as active repos and the
   ISO date already present in the listing — zero extra API calls. A
   `dormant_count()` convenience method preserves the old summaries.
2. **`dormant_days` parameter.** `scan_group`, `get_ai_adoption`, and
   `generate_ai_adoption_report` take `dormant_days: u32`; the old constant
   stays as `pub(crate) const DORMANT_DAYS: u32 = 180`, used by `server.rs`
   as the `unwrap_or` default. Both param structs gain an optional
   flex-deserialized `dormant_days`.
3. **By Team gains a Dormant column** (markdown and HTML). Dormant repos are
   folded into the team map after aggregation, so teams with *only* dormant
   repos still appear as a row (0 active, N dormant). The fold happens after
   the `summary_only` early return, keeping the compact line unchanged
   ("N repos scanned, M dormant").
4. **Archive-candidates section** after Recommendations. Markdown: a
   `| Repo | Team | Last activity |` table sorted oldest-first, capped at 20
   with a "+N more" note. HTML: the same table inside a collapsible
   `<details>` block (uncapped — file output is not token-constrained) so it
   does not clutter the leadership view. Omitted entirely when nothing is
   dormant.
5. **Pure helpers, unit-tested:** `sorted_dormant` (oldest-first, unknown
   dates last) and `dormant_by_team` (per-team counts) — both pure over
   `DormantRepo`, tested with my-org style fixtures.

## Consequences
- Easier: dormancy is now actionable (who owns what, idle since when) at no
  API cost; the threshold is tunable per call (e.g. 90 days for an
  aggressive cleanup sweep); both formatters share one sort/count
  implementation.
- Harder/limits: dormant repos carry no markers (they are skipped before
  `scan_repo`), so a dormant repo with a CLAUDE.md is invisible to adoption
  levels — by design, scanning them would cost ~5 calls each; the markdown
  cap (20) means very large graveyards need the HTML report for the full
  list.
- Follow-up: none planned; an `archive_project` write tool could consume
  this list if cleanup is ever automated.
