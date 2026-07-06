# ADR 029: Architecture pass — retry core, typed errors, shared user resolution

## Status

Accepted (2026-07-03)

## Context

An architecture review of the codebase (18.6k lines, 99 tools) confirmed the
overall shape is sound — feature-oriented `tools/*` modules, macro-collapsed
registration, a flat params file — but found three structural defects. Each had
already caused, or nearly caused, a production incident:

1. **The 429-retry loop was hand-copied into all six HTTP verbs.** Adding
   `get_text` (ADR 021) required copying the loop again; the original
   `get_job_log` bug existed precisely because adding a verb was a copy-paste
   job. Any retry-policy change needed six coordinated edits.
2. **Error classification was stringly-typed.** ADR 026 patched the Sentry
   false-page by widening a substring allowlist — but the allowlist has to be
   maintained forever, and every new validation message risks a new false page.
3. **Username→id resolution existed five times** — a hard-erroring version in
   `projects.rs` plus four inline soft copies in `issues.rs` and
   `merge_requests.rs`, one of which silently swallowed even transport errors.

## Decision

1. **One retry core.** `GitLabClient::send_with_retry(path, build)` owns the
   429/Retry-After loop and success/error body handling; every verb is now a
   3-line wrapper (build request → send through core → parse). `parse_json`
   centralizes the empty-body-as-null rule.
2. **Typed user errors.** New `Error::UserInput(String)` variant for
   validation failures, and `Error::is_user_error()` covering
   `UserInput`/`NotFound`/GitLab 4xx (except 408/429, which are environmental).
   The `tool_call!` boundary reports such failures with analytics status
   `user_error`; the Sentry layer captures only status `error`. Classification
   is by construction at the type level — the ADR 026 string heuristic remains
   only as a fallback for paths that carry a plain message string (e.g.
   cross-instance report fallbacks). Side benefit: analytics now separates
   user errors from real errors.
3. **One user-resolution module.** `tools/users.rs` holds both deliberate
   flavors: `resolve_user_id` (hard — errors on unknown user; used by member
   grants where the user *is* the point) and `lookup_user_id` (soft —
   `Ok(None)` on unknown user; used for assignee/reviewer enrichment where the
   write should proceed without the field). Access-level parsing
   (`parse_access_level`/`access_level_name`) moved here from `projects.rs`.
   MR assignee/reviewer lookups keep their historical never-block semantics
   (`Ok(Some(id))` pattern), now stated explicitly instead of via
   `unwrap_or_default()`.

## Considered and rejected

- **A `proj_path!()` helper for the ~120 `format!("/projects/{}", urlencoding::encode(id))`
  sites** — cosmetic; touching 10 files for no behavioral gain is churn, not
  refactoring.
- **Merging the text and HTML adoption render paths** (duplicated `TeamStats`) —
  real duplication, but the two outputs legitimately differ in shape; a shared
  render abstraction would be speculative.
- **Hard-failing on unknown assignee/reviewer in MR/issue creation** — arguably
  better UX than silent skip, but it changes behavior callers may rely on;
  deferred as a possible opt-in.

## Consequences

- Retry policy, empty-body handling, and error extraction each live in one
  place; a new verb is ~4 lines and cannot fork the policy.
- User-input mistakes can no longer page Sentry regardless of message wording;
  the fragile phrase list is defense-in-depth, not the primary gate.
- One definition of "resolve a user", with the hard/soft split documented as an
  intentional design axis rather than accidental divergence.
- Behavior-preserving by test: the full suite (175 tests, including new
  classification and access-level tests) passes; the tool surface is unchanged.
