# ADR 064: What endpoint AI monitoring looks like from the repository

## Status

Accepted (2026-10-19)

## Context

Endpoint AI-monitoring products (browser extensions and desktop sensors such as
NordLayer AI Scout) report:

- which AI tools are in use, including unapproved ones;
- usage by team;
- sensitive data in prompts;
- for engineering: which repositories use coding agents, and which agent sessions
  need review.

A GitLab scan can see none of the prompts or sessions. Some of those views do have an
equivalent in what lands in the repository. Comparing the two exposed gaps and a
defect in the adoption report.

- **Defect.** A commit counted as AI-assisted when it had *any* `Co-authored-by`
  trailer and the string "AI" appeared *anywhere* in its message. A human pairing on
  "fix AI summary endpoint" counted as AI usage. MR descriptions counted *any*
  co-author trailer, human or not.
- **Coverage.** Only Claude, Copilot and Cursor were recognised in trailers. Coding
  agents named by endpoint products — Codex, OpenHands, Aider by trailer, Kilo Code,
  Roo Code, opencode — were invisible, so usage was under-read.
- **Review.** Nothing looked at whether AI-marked changes were reviewed by anyone.

## Decision

- **Attribution is decided per trailer line.** A `Co-authored-by` line names an AI
  tool when its email domain is a known tool domain (anthropic.com, cursor.com,
  aider.chat, all-hands.dev, devin.ai, cline.bot, kilocode.ai, opencode.ai,
  roocode.com), or when a whole word of the name is an unambiguous tool name (copilot,
  codex, aider, openhands, gemini, windsurf, kilocode, opencode, roocode).
  - Tool names that are also people's names match only by domain: Devin, Cline,
    Junie.
  - "Claude" matches by domain, or by name only alongside a model word (opus,
    sonnet, haiku, fable, code, ai). "Claude Dupont" stays a person.
  - Matching is on whole words, so "Brooks" is not Roo Code.
  - Aider's `aider: ` commit-subject prefix also counts.

  The same classifier decides MR descriptions. A "Generated with <tool>" footer or
  the robot emoji that tools stamp on their footers still counts.
- **More tools are recognised.** New config markers: `.kilocode/`, `.roo/` and
  `.roomodes`, `opencode.json` and `.opencode/`, and `.openhands/`. Only markers with
  a documented repository footprint are added; tools without one are not guessed at.
- **AI changes without review.** From the MR list the scan already fetches, it counts
  AI-marked MRs that were merged with no reviewer or by their own author. This is a
  repo flag ("unreviewed AI changes"), an organisation recommendation, and a
  practice-check row ("AI changes reviewed by someone else"). It costs no extra
  requests.
- **The methodology states the boundary.** It lists what a repository scan cannot see:
  prompts, chat or IDE use that never reaches a commit, and per-person usage outside
  git. Endpoint tools see those; this report sees what landed in the code.

## Consequences

- AI commit counts can fall where human co-authors were counted before, and rise
  where other agents' trailers were missed. Both moves are corrections.
- An agent whose trailer carries neither a known domain nor an unambiguous name is
  still not counted. That remains a lower bound, as the report already says.
- Tool inventory per team and an approved-tools list ("shadow AI" by name) are left
  for later. The data is in place.
