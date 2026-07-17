# ADR 037: People-based adoption metrics (developer rate + commit share)

**Status:** Accepted
**Date:** 2026-07-17

## Problem

The adoption report was entirely **repo-centric**: L0–L3 maturity per
repo, AI-Active/Configured repo counts per team, per-repo AI-commit
percentages. A methodology review against how the industry reports AI
adoption (platform-team and DevEx research reporting, AI-assistant
impact studies) found the two headline metrics those audiences expect
were missing:

1. **Developer adoption rate** — the share of *people* actively using
   AI in the window, not the share of repos. A team can be "3/6 repos
   AI-active" while 1 of 9 engineers does all of it — repo counts hide
   that. This is the number leadership asks for first.
2. **AI commit share, commit-weighted** — the existing per-repo
   percentages average a 10-commit repo and a 500-commit repo equally;
   the org-level "what fraction of our commits are AI-assisted" needs
   Σ AI-trailed / Σ total across all repos.

The data for both was already fetched (the all-branches commit list
per repo); it just wasn't aggregated by author.

## Decision

`RepoMarkers` gains two uncapped sets collected in the same commit
pass the scan already makes (zero extra API calls):

- `ai_author_set` — distinct non-bot authors of ≥1 AI-trailed commit;
- `all_author_set` — distinct non-bot authors of any commit.

`is_bot_author` excludes automation identities from both sides
(`[bot]` convention + known names: renovate, dependabot, GitHub
Actions, gitlab-ci, semantic-release). Deliberately conservative — a
human named "Abbott" is never excluded; a bot slipping through
deflates the rate slightly rather than erasing a person.

Aggregation is a union across repos and teams, so a developer active
in several repos counts once. Surfaced as:

- **Markdown (`get_ai_adoption`)**: bold headline
  `Developer adoption: A/B devs (P%) · AI commit share: S%`, a
  `Devs (AI/all)` column in the team table, and `devs A/B (P%) · AI
  share S%` in the `summary_only` line.
- **HTML (`generate_ai_adoption_report`)**: two new summary cards
  (Developer Adoption, AI Commit Share) placed first, a
  `Devs (AI/all)` By-Team column (green when every active committer
  has AI-assisted work), and a methodology paragraph defining both
  metrics and their **telemetry-lower-bound** caveat (trailers are the
  only visible signal; squash-merge and disabled attribution hide real
  usage).

Repo-centric maturity metrics (levels, AI-Active/Configured,
trajectories, flags) are unchanged — config quality and people
adoption are separate axes and both stay visible.

## Consequences

- The report now leads with the numbers external reporting frameworks
  treat as primary, computed from data already in hand.
- Union semantics mean team `Devs` columns do not sum to the org
  headline when people commit across teams — that is correct, and the
  methodology says so.
- Not addressed (deliberately): period-over-period trend deltas would
  need a second scan window or persisted snapshots; acceptance-rate /
  outcome metrics need editor or delivery telemetry that git does not
  carry. Both are honest gaps, not silent ones — the methodology
  states what is and is not measured.

## Tests

`test_bot_author_detection` (humans containing "bot" letters never
excluded), `test_collect_author_sets_people_rate` (AI ⊆ all, bots
dropped, dedupe), `test_collect_author_sets_empty_and_missing_author`.
213 tests pass.
