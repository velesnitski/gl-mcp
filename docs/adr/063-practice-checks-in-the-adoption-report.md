# ADR 063: Practice checks in the adoption report

## Status

Accepted (2026-10-14)

## Context

ADR 061 aligned the report's per-repository flags with Anthropic's published Claude
Code guidance. Flags answer "what is wrong with this repo". They do not answer the
question a reader of an org-wide report asks: how far the organisation as a whole
follows the practices, and where to start.

## Decision

The report gains a **Practice checks** section, placed after the benchmark, in both
the markdown and HTML renderings. Each row scores one practice across the scanned
repositories. It shows how many applicable repos follow it, names up to five that
don't, and gives a one-line reason. Only practices a repository scan can verify are
listed:

- CLAUDE.md is within 200 lines, because it loads on every request.
- CLAUDE.md is more than a stub, so it says what the repo is.
- Procedures live in skills, which load only when needed.
- Team setup is committed to the repo (skills, commands, agents or `.mcp.json`).
  Personal `~/.claude` config reaches neither teammates nor cloud sessions.
- No credential is committed in `.mcp.json`.

A check applies only to the repos it can judge. A check with no applicable repo is
omitted, not shown as 0/0 or as a perfect score. The section cites its source.

Practices a scan cannot observe are left out rather than approximated: judgment over
rules, effort levels, cost per task, and read-only tokens for automations.

## Consequences

- The section is computed by a pure function (`practice_checks`) from markers the scan
  already gathers. It adds no requests.
- It is pinned by a unit test and by the report golden snapshots.
