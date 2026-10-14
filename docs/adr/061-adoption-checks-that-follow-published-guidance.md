# ADR 061: Adoption checks that follow published guidance

## Status

Accepted (2026-10-14)

## Context

The AI-adoption report judges each repository's AI configuration. Three of those
judgments were checked against Anthropic's published guidance for Claude Code
(claude.dev, 2026: context engineering for Claude 5 models, task cost on Opus 5.5,
cloud sessions). The check found two that disagreed with it and one gap.

- **CLAUDE.md length.** The cost guidance says to keep CLAUDE.md under 200 lines,
  because it loads on every request. The report measured bytes and called a file
  bloated only above 15 KB. That is roughly 300 or more typical lines, so files well
  past the guidance passed.
- **Advice wording.** The report suggested "add a CLAUDE.md (or the org template)".
  The context-engineering guidance is that CLAUDE.md should say what this repository
  is and list its gotchas. It should not restate what the file tree already shows,
  and it should not be a central store of generic practices. A shared template copied
  into every repo is exactly that.
- **Committed credentials.** `.mcp.json` counted as a positive adoption marker, but
  its contents were never read. The file is committed, so every clone gets it, and
  it travels into cloud sessions. The cloud guidance keeps secrets outside the VM and
  out of shared variables, so a literal credential in it is a published secret.

## Decision

- CLAUDE.md length is judged in lines (`CLAUDE_MD_MAX_LINES = 200`). The count comes
  from the content already returned by the request that reads the file's size, so it
  costs no extra request. Bytes remain the fallback when the content cannot be read.
  The flag text points at the remedy the guidance names: keep the file to what the
  repo is and its gotchas, and move procedures into skills.
- The no-config advice reads: add a short CLAUDE.md that says what the repo is for
  and lists its gotchas, not a generic template.
- When a repository has `.mcp.json`, its raw content is read once. It is flagged
  when a server's `env` or `headers` carries a credential-shaped literal instead of a
  `${VAR}` reference. The report states only that a credential is present, never its
  value. The flag is rendered as a risk, and it heads the recommendations ahead of
  every adoption suggestion.

## Consequences

- Repositories with a CLAUDE.md between 200 lines and roughly 15 KB are now flagged.
  That is intended: those are the files the guidance says to cut.
- Repositories with `.mcp.json` cost one more request per scan.
- The detector errs toward silence. It needs a secret-like key name and a value of at
  least 8 characters, so a credential under an unusual key name can still pass. It
  catches the common case and does not claim to audit secrets in general; that is
  `audit_ci_security`'s job.
