# 001 – AI Adoption Scorecard via Repository Marker Scanning

## Status
Accepted

## Context
We want to track how teams across a GitLab group adopt AI-assisted development
(Claude Code in particular) without surveys or self-reporting. Options considered:

1. **Repository marker scanning** – detect config artifacts (CLAUDE.md, `.claude/`
   agents/skills/commands/settings, `.mcp.json`, cursor/windsurf configs, ADR dirs)
   plus `Co-Authored-By` AI trailers in recent commits.
2. **Commit-trailer-only analysis** – count AI co-authored commits per repo.
   Rejected: misses teams that configure tooling but squash commits or strip
   trailers; gives no signal about setup quality.
3. **CI/editor telemetry** – accurate usage data but requires agent-side
   instrumentation we don't control and raises privacy concerns. Rejected.

Constraints: must be read-only, token-efficient, and resilient — one broken or
empty repo must not fail a 300-repo group scan.

## Decision
Implement `get_ai_adoption` (src/tools/adoption.rs) using option 1:

- List group projects (max 3 pages = 300 repos), skip repos dormant >180 days
  to save ~5 API calls each.
- Per repo, fetch root tree, `.claude/` subtrees, `docs/adr`, CLAUDE.md size,
  and one page of recent commits — batched 10× concurrent, every call
  `unwrap_or_default()` so failures degrade to "no markers".
- Score with a pure function (`adoption_level`, `quality_flags`) on a
  `RepoMarkers` struct, keeping scoring unit-testable without HTTP:
  L0 none → L1 exploring (any config) → L2 practicing (CLAUDE.md + deeper
  practice, or unused agent setup) → L3 scaling (agents + skills + real usage).
- Team = second path segment of `path_with_namespace`; `(root)` for 2-segment paths.
- Quality flags surface anti-patterns: stub (<200B) / bloated (>15KB) CLAUDE.md,
  "setup unused" (agents but 0 AI commits), "usage w/o config" (AI commits, no CLAUDE.md).

## Consequences
- Easier: per-team adoption visibility from a single read-only MCP call;
  scoring thresholds are centralized and covered by 16 unit tests.
- Harder/limits: marker-based detection misses devs who use AI without
  committing config or trailers; commit scan reads only the first 100 commits
  of the default branch; dormant cutoff (180d) and level thresholds are
  hardcoded constants — tuning requires a code change.
- Follow-up: none planned; thresholds can be parameterized later if teams ask.
