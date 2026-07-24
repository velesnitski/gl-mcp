# ADR 040: Make AI-adoption reporting realistic — tool-agnostic markers + the three axes

## Status

Accepted (2026-07-17)

## Context

The adoption scan under-reported reality in two ways.

**1. It was effectively Claude-centric.** It detected `.claude/`, `CLAUDE.md`,
`AGENTS.md`, `.mcp.json`, and Cursor/Windsurf configs, but missed other common
assistants and power-user infra:

- **GitHub Copilot** (`.github/copilot-instructions.md`),
- **Aider** (`.aider.conf.yml`, `.aiderignore`),
- **Continue** (`.continue/`),
- a committed **code-graph index** (`.codebase-memory/`) used for AI retrieval,
- hook-driven automation living in a root `githooks/` / `.githooks/` (only
  `.claude/hooks` was recognized).

So a repo whose team uses Copilot or Aider — or maintains a code-graph index — was
scored **L0 "no AI"**, which is the opposite of the truth.

**2. The report invited conflating three different things.** Config presence, actual
usage, and visibility of that usage are separate axes, and they routinely disagree:
a repo can be fully configured yet unused, or heavily used with no config, or used
in ways a repo scan can't see at all (squash-merge strips attribution; a developer's
own local-only tooling is never committed). Reading "has `.claude/`" as "adopting"
is wrong in both directions.

## Decision

1. **Tool-agnostic markers.** Detect Copilot, Aider, Continue, `.codebase-memory/`,
   and root `githooks/` / `.githooks/` alongside the existing set. Non-Claude
   configs are collected into a deduped `other_ai` list and surfaced in the marker
   list. Any of them now counts as a marker, so such a repo scores at least
   **L1 (Exploring)** instead of L0. Copilot's nested instructions cost one extra
   tree read, gated on a `.github/` dir being present.

2. **State the axes and the lower bound in the report.** A "How to read this"
   footer makes three things explicit: config ≠ usage ≠ visibility; the commit
   share is a **lower bound** (squash-hidden trailers, disabled attribution, and
   local-only tooling all sit below it); and the markers are tool-agnostic.

## Consequences

- Repos adopting non-Claude assistants are counted instead of read as "no AI"; the
  org-wide picture stops being a Claude-only view.
- The report no longer implies "configured" means "used" — the existing
  setup-unused / squash-hidden / stale-config flags now sit under an explicit
  framing that names the three axes.
- The scan still cannot see uncommitted local tooling; that is now stated as a known
  lower-bound rather than left implicit. The honest answer to "who's the most
  advanced AI user" may not be on any repo dashboard.
