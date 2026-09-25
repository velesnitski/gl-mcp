## AI Adoption: acme (last 30d, 6 active repos, 2 dormant skipped)

**Developer adoption: 3/5 devs (60%) · AI commit share: 30% (52/173 commits)**

### Industry Benchmark

**Maturity: Advanced** (developer adoption 60%) · **Config coverage: partial** (3/6 active repos, 50%) · **Depth: agentic (subagents in use)**

_Reference bands (directional, not a cited statistic): Nascent <15% · Emerging 15–29% · Mainstream 30–49% · Advanced 50–74% · Leading ≥75% developer adoption. The grade is a floor — squash-hidden and local-only usage sit above it._

**To advance:**
1. 1 repo(s) have AI usage but no config — add a CLAUDE.md (or the org template). Cheapest tier win, and it makes the usage visible.
2. Attribution is lost at merge (squash-hidden usage) — disable trailer-stripping squash or standardize on MR-description attribution, so the dashboard stops under-reading real usage.
3. Activate 1 more developer(s) with AI to reach the Leading band (75%).

_Benchmarks & practices: [Claude Code best practices](https://www.anthropic.com/engineering/claude-code-best-practices) · [AGENTS.md](https://agents.md/) · [12-Factor Agents](https://github.com/humanlayer/12-factor-agents) · [Pragmatic Engineer: AI tooling survey](https://newsletter.pragmaticengineer.com/p/ai-tooling-2026) · [DORA 2025: State of AI-assisted development](https://dora.dev/dora-report-2025/)._

### By Team

| Team | Repos | Active | Configured | Best level | Devs (AI/all) | AI commits % (avg of configured) | Dormant |
|------|-------|--------|-----------|-----------|---------------|----------------------------------|---------|
| core | 2 | 2 | 2 | L3 | 2/3 | 20% | 1 |
| data | 2 | 1 | 0 | L0 | 1/2 | – | 0 |
| ops | 0 | 0 | 0 | L0 | 0/0 | – | 1 |
| web | 2 | 1 | 1 | L1 | 0/1 | 0% | 0 |

### Adopting Repos

| Repo | Level | Traj | Markers | AI commits | Flags |
|------|-------|------|---------|-----------|-------|
| core/api | L3 | ↑ | CLAUDE.md, agents(3), skills(2), commands, settings, hooks, .mcp.json, tasks, ADR active(2) | 40% (40/100) +4 MRs +5 task commits | – |
| core/worker | L2 | ↓ | CLAUDE.md, agents(1) | 0% (0/20) | setup unused, stale config (30+ commits behind) |
| web/site | L1 | → | AGENTS.md, cursor, tasks | 0% (0/15) +2 task commits | no attribution |

### In-flight (branch signals only)

| Repo | Branch | Last activity |
|------|--------|---------------|
| web/app | feature/claude-setup | 2026-09-01T00:00:00Z |

### Invisible usage (no config)

Devs adopted Claude on their own — the repo gives it no context. Cheapest win: add a CLAUDE.md.

| Repo | AI commits | Who | Attribution |
|------|-----------|-----|-------------|
| data/etl | 40% (12/30) | joe (12) · Copilot | squash-hidden (branches only) |

### Quality Flags
- acme/core/worker: setup unused — .claude/agents present, 0 AI commits in 30d
- acme/core/worker: stale config — CLAUDE.md last touched 2025-11-01 but 20+ commits since — refresh it
- acme/web/site: no attribution — 2 .tasks / 0 .claude commits in 30d but 0 AI-trailed commits — enable Co-Authored-By attribution for measurable adoption
- acme/web/app: in-flight (branch: feature/claude-setup) — AI work on feature branches, no config on default yet
- acme/data/etl: usage w/o config — 40% AI commits (joe) but no CLAUDE.md (add one)
- acme/data/etl: squash-hidden usage — 12 AI-trailed commits (joe) on feature branches, 0 on default — squash strips attribution at merge

### Recommendations
- data team: 0 adoption across 2 active repos — pilot candidate: acme/data/etl
- 1 repos have AI commits but no CLAUDE.md — quick win: add one
- 1 repos show agent activity without commit attribution — standardize Co-Authored-By trailers to measure adoption
- 1 repos lose attribution at merge — check squash settings or rely on MR descriptions
- 1 repos have AI work on feature branches — adoption pipeline

### Dormant repos (archive candidates)

Inactive 90+ days and not archived — consider archiving to reduce noise.

| Repo | Team | Last activity |
|------|------|---------------|
| ops/old-infra | ops | 2024-06-01T00:00:00Z |
| core/legacy | core | 2025-01-01T00:00:00Z |

### How to read this
- **Config, usage, and visibility are three separate axes.** A repo can be fully configured yet unused (setup present, 0 AI commits — flagged), or heavily used with no config (invisible-usage section). Don't read "has `.claude/`" as "adopting".
- **The commit share is a lower bound.** Squash-merge strips Co-Authored-By trailers (flagged as squash-hidden), teams disable attribution, and local-only tooling (a dev's own `.codebase-memory/`, hooks, or commands that are never committed) is invisible to any repo scan. Real usage is ≥ what's shown.
- **Tool-agnostic.** Markers cover Claude (`.claude/` incl. agents, skills, commands, output-styles, plugins; `CLAUDE.md`), plus Cursor, Copilot, Aider, Windsurf, Continue, Cline, JetBrains Junie, Gemini (`AGENTS.md`, `.mcp.json`), agent-memory infra (`.codebase-memory/`, `memory-bank/`, `.serena/`), hooks, and `llms.txt` — so a non-Claude repo isn't misreported as "no AI". Known gap: only **root** `CLAUDE.md` is detected, so nested per-directory memory in a monorepo is undercounted.