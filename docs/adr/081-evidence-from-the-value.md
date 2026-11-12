# ADR 081: Evidence from the value

**Status:** Accepted
**Date:** 2026-11-12

## Context

A literal flagged in a CI file is often disputed by the team that owns it: "that is
not a password, it is a test default", or "that name is legacy". Name-based rules
cannot settle it, and the report cannot show the value.

## Decision

- Each hit records what its value suggests, judged when the line is read and never
  stored with the value:
  - **Format**: the value has the shape of a specific credential type (Apple
    app-specific password, YouTrack permanent token, GitLab token, Slack webhook,
    Telegram bot token, JWT, provider-prefixed API key). The finding is
    near-certain.
  - **Random**: long, mixed letters and digits, many distinct characters.
  - **Trivial**: a default word, a repeated or sequential run, a short number, or
    the key's own name. Most likely a test default or a leftover.
  - **Unclear**: neither.
- A trivial value is MEDIUM and does not count as live: confirm nothing accepts it,
  then remove it. A format match keeps the finding HIGH.
- The verdict is shown next to each finding in `audit_ci_security`,
  `audit_dependencies` and the CI email, so an owner's objection can be checked
  against it without anyone seeing the value.

## Consequences

- Disputes about a finding start from evidence rather than from the variable's name.
- A weak but real password is reported as trivial; it is still listed, so it is
  still fixed.
