# ADR 032: audit_readmes — org-wide README quality scan

## Status

Accepted (2026-07-09)

## Context

"Find all repos with no / small / Russian README" is a real doc-hygiene
question for an org with 150+ projects. Answering it ad-hoc means fetching every
README through `get_file_content` — 150+ calls, each dumping full file content
into the caller's context. That is both slow and token-hostile, and the answer
(a short list of problem repos) is tiny.

## Decision

Add `audit_readmes(group_path, small_bytes=300, cyrillic_pct=20, include_ok=false)`,
a read-only scan modeled on `get_ai_adoption`: all fetching and classification
happen **server-side**, and the caller receives a compact table.

Per project (non-archived, default branch), at most two calls:

1. list the repository root tree and find a `readme*` blob (any variant/case);
2. fetch its raw bytes via the files `…/raw` endpoint (no base64 handling).

Classification, by priority:

- **Missing** — no README blob (or empty repo).
- **Russian/Cyrillic** — `cyrillic_pct(content) ≥ threshold`, where the metric is
  the share of *alphabetic* characters in the Cyrillic block. Checked before
  "small" so a short Russian README is reported as the language issue, which is
  what the operator asked to find.
- **Small** — under `small_bytes` (a stub).
- **Ok** — otherwise (listed only when `include_ok`).

READMEs are fetched in bounded-concurrency chunks (12) to avoid provoking 429s
at scale; the client's retry handles any that slip through.

## Consequences

- The whole org is auditable in one call, returning counts + a per-bucket table
  instead of 150 file bodies — cheap in tokens and fast.
- Complements `get_ai_adoption` (AI-tooling hygiene) with documentation hygiene;
  both are server-side org scans.
- The Cyrillic ratio is a language *proxy*, not detection: a README that is
  genuinely majority-Russian scores high; a mostly-English one with a short
  Russian note stays low (pinned by `cyrillic_detection` test). Thresholds are
  tunable per call.
- Additive (new tool, no surface change) → minor version under the 1.0 contract.
