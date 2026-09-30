# ADR 058: Results that say what they left out

## Status

Accepted (2026-09-30)

## Context

Three read tools returned output that was technically complete, but misleading or
wasteful in practice:

- `search_code` printed raw bytes from image and binary files as "snippets", and
  lockfile hits (`bun.lock`, `package-lock.json`) crowded out source hits. A search in
  an Android repo spent most of its response on WebP headers.
- `list_commits` with `summary_only` omitted dates. Placing commits in time, which is
  what a scan is usually for, took one extra call per commit.
- `get_tree` stopped at `per_page` and reported the count it had, with nothing to say
  that the listing was cut short. A full page read as "that is the whole directory".

## Decision

- Project search renders source hits first, with snippets. Lockfile hits follow as
  paths only. Binary hits are listed as paths with the preview omitted. A hit counts
  as binary by extension, or when control or replacement characters make up more than
  a tenth of the snippet. Every hit is still reported: this changes presentation, not
  coverage.
- `summary_only` commit lines are `sha|date|author|title`, and the header names the
  columns.
- `get_tree` warns when the result fills `per_page`.

## Consequences

Rendering is a pure function (`render_search_hits`) with tests. The commit-summary
format gains a column; it is read by agents, not parsed by scripts.
