# ADR 007: Name the humans behind AI usage

## Status

Accepted (2026-06-11)

## Context

The adoption scan counts AI-trailed commits but discards *who* made them and
*which tool* co-authored them. Reading the report, "15% AI commits,
squash-hidden" answers nothing actionable: leadership wants to know which
engineers are the invisible champions worth recruiting, and whether the usage
is Claude, Copilot, or Cursor. Answering that today requires manually pulling
full commit messages per repo — exactly the drill-down the report was built to
avoid.

The key observation: the scan already fetches up to 100 full commit objects
per repo across all branches (step 5 of `detect_markers`). Author names,
trailer lines, and commit subjects are all in that payload and were being
thrown away.

## Decision

Extract three facts in the same pass, **zero additional API calls**:

- `ai_authors: Vec<(String, usize)>` — commit authors of trailed commits,
  count desc, cap 3.
- `ai_tools: Vec<String>` — tool name parsed from the `Co-Authored-By:`
  trailer itself (e.g. "Claude Opus 4.7"), parenthetical suffix and email
  stripped, cap 3 distinct. Human co-authors are filtered by the same
  claude/copilot/cursor/AI heuristic as commit counting.
- `ai_sample: Option<(String, String)>` — (short_sha, subject) of the most
  recent trailed commit; commits arrive newest-first so the first match wins.

Surfacing:

- **Invisible usage** (both formats): new "Who" column — authors + tool; the
  HTML repo cell gains a linked sample commit (`/-/commit/<sha>`), keeping the
  "a number should link to its evidence" rule from ADR 006.
- **Quality flags** "usage w/o config" and "squash-hidden usage": top author
  named inline — the flag now points at a person to talk to, not just a repo.
- **Adopting Repos** (HTML): muted "Who" subline under the Usage cell.
- Methodology footnote documents the extraction.

## Consequences

- The report names names. That is the point — champions become visible — but
  it reads as a leaderboard, not surveillance: only trailed (self-declared)
  commits are counted, and the data was already one click away in GitLab.
- Tool names come from free-text trailers, so unparseable variants fall back
  to author-only display rather than guessing.
- Author identity is `author_name` from the commit, which is self-reported
  git config — duplicate identities ("Jane D" vs "Doe Jane") appear as
  separate entries. Acceptable: the cap-3 list keeps it readable and the
  counts stay honest per identity.
