# ADR 026: Keep user-input errors out of Sentry

## Status

Accepted (2026-07-02)

## Context

Tool errors are surfaced to the caller and, when compiled with the `sentry`
feature, actionable ones are also captured as Sentry events. The gate,
`is_actionable_error`, filtered out expected user-side errors by matching their
message against an **exact-string allowlist** (`"User not found"`,
`"User has no ID"`, …).

That allowlist is brittle. The newer member/project-lifecycle tools emit
variably-worded validation messages that the exact strings didn't cover:

- `add_member` → `"User '@ghost' not found"` — the interpolated username
  breaks the contiguous match against `"User not found"`.
- `resolve_user_id` → lowercase `"has no id"` vs the list's `"User has no ID"`.
- `resolve_namespace` → `"Group '…' not found or not accessible"`.
- `parse_access_level` → `"Unknown access level '…'"`.
- `delete_project` → `"Refusing to delete: confirm_full_path …"`.

A mistyped username therefore created a **Sentry issue and paged the team for a
non-bug** — the tool had correctly reported that the user does not exist.

## Decision

Match user-error **phrases case-insensitively** instead of exact strings.
`is_actionable_error` lowercases the message and checks for markers such as
`not found`, `not accessible`, `unauthorized`, `forbidden`, `unknown instance`,
`unknown access level`, `has no id`, `refusing to delete`, `nothing to update`,
`no commits/changes/activity`. GitLab 4xx (except 408/429) remain user errors.
Real errors — 5xx, HTTP/transport, JSON parse — are unaffected and still page.

## Consequences

- Validation / not-found errors from every current tool stay out of Sentry;
  the noise (and the false page) is gone.
- The phrase set is resilient to interpolated ids/paths and casing, so new
  validation messages that use the same vocabulary are covered automatically.
- Regression tests pin the exact messages that leaked
  (`test_is_actionable_error`), so this can't silently regress.
- Trade-off: a genuine defect whose message happens to contain a marker like
  "not found" would be suppressed — acceptable, since those phrases denote
  expected conditions in this codebase.
