# ADR 006: Clickable entities in the HTML AI adoption report

## Status

Accepted (2026-06-11)

## Context

The HTML adoption report (`generate_ai_adoption_report`) is consumed in a
browser, but every entity in it was plain text: to inspect a repo, branch, or
marker file the reader had to reconstruct the GitLab URL by hand. The project
listing already returns `web_url` and `default_branch`, which the scanner
discarded.

## Decision

- `RepoResult` gains `web_url` + `default_branch`; `DormantRepo` gains
  `web_url`. Both populated from the project listing JSON (zero extra API
  calls; `default_branch` falls back to `main`, `web_url` to empty).
- HTML formatter only — the markdown scorecard (`get_ai_adoption`) stays
  link-free for token efficiency.
- A `link(url, text)` helper wraps text in an anchor when the URL is non-empty
  (graceful plain-text fallback otherwise); URLs are HTML-escaped, link text is
  pre-escaped by callers.
- `markers_html()` rebuilds the marker list from `RepoMarkers` with each
  marker linked to its source blob/tree (`CLAUDE.md`, `.claude/agents`,
  `.claude/skills`, `.claude/commands`, `.claude/hooks`,
  `.claude/settings.json`, `.tasks`, `docs/adr`). `cursor` stays unlinked
  (ambiguous source file).
- Linked: repo names everywhere (Adopting, In-flight, Invisible usage, Quality
  flags, pilot candidates, Dormant), in-flight branches
  (`/-/tree/<url-encoded branch>`), usage cells (`/-/commits/<default>`,
  `+N MRs` → merged MR list), and team names
  (`{origin}/{group}/{team}`, origin derived from any repo's `web_url`;
  `(root)` skipped).
- CSS: links inherit color with a dotted underline, highlight on hover, and
  flatten to plain text in print/PDF.

## Consequences

- Report entities are one click from their GitLab source; no API cost added.
- Markdown output and scan logic are byte-identical to before.
- Missing `web_url` (or unknown host) degrades to today's plain text.
