# ADR 041: Industry benchmark rating + expanded AI-tool markers

## Status

Accepted (2026-07-17)

## Context

The adoption report gave raw metrics — developer-adoption rate and AI commit share —
but left the reader to judge whether the numbers were good, and what to do next.
"20% developer adoption" means nothing without a reference point. Leads need a
verdict and a roadmap, not just percentages.

Separately, the marker set — even after the tool-agnostic pass (ADR 040) — still
missed several 2026-common signals: JetBrains **Junie**, **Gemini** (`GEMINI.md`),
**Cline** (`.clinerules` + `memory-bank/`), the **Serena** memory store (`.serena/`),
Claude Code **output-styles** and **plugins**, and the emerging **`llms.txt`**
standard. A team on those tools still read as under-adopting.

## Decision

**1. Expanded, deeper markers.** Detect Junie, Gemini, Cline, `.serena/` and Cline's
`memory-bank/` (folded with `.codebase-memory/` into one **agent-memory** signal),
`.claude/output-styles` and `.claude/plugins`, and `llms.txt`. All are cheap
root/`.claude`-tree match arms.

**2. An "Industry Benchmark" section** at the top of the report:

- **Maturity tier** from the developer-adoption rate, against directional reference
  bands: Nascent <15% · Emerging 15–29% · Mainstream 30–49% · Advanced 50–74% ·
  Leading ≥75%.
- **Config coverage** — share of active repos that are configured (ad-hoc / partial
  / standardized).
- **Depth** — whether any repo runs agents / memory / structured config.
- **Top-3 gap-driven suggestions** — generated from the actual data (invisible
  usage, squash-hidden attribution, devs-to-next-tier, coverage), not generic advice.
- **Reference links** to widely-used practices (Claude Code best-practices/docs,
  AGENTS.md, llms.txt, DORA).

Two honesty guardrails are stated inline: the bands are **directional reference
points, not a cited statistic**, and the grade is a **floor** — squash-hidden and
local-only usage sit above it (per ADR 040). The tier/coverage mappings are pure,
tested functions.

## Consequences

- The report now leads with a verdict a lead can act on, and a concrete path to the
  next tier, instead of leaving interpretation to the reader.
- Teams on non-Claude / JetBrains tooling are counted; the org picture is genuinely
  tool-agnostic.
- The bands are a deliberate judgement call, labelled as such. If an org disagrees,
  the thresholds live in two small functions and are trivial to adjust — better to
  ship a transparent, tunable default than no benchmark at all.
