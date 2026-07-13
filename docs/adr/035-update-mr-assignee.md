# ADR 035: `update_merge_request` assigns — and resolves the assignee *hard*

## Status

Accepted (2026-07-13)

## Context

`create_merge_request` could set an assignee; `update_merge_request` could not. It
only touched title, description, labels and target branch. That left a real gap:
**an already-open merge request could not be assigned to anyone through the tool.**

It surfaced while trying to route a batch of documentation MRs to the relevant team
leads after the MRs had already been opened. There was no path to do it short of
closing and recreating each MR — which would have thrown away the MR numbers and any
discussion on them.

## Decision

Add an `assignee` parameter to `update_merge_request`. It takes a username (leading
`@` optional) or a numeric id, and accepts `none` / `unassign` / `unassigned` / `0`
to clear the assignee (GitLab clears it when `assignee_id` is `0`).

The consequential decision is **how** the username resolves, and it is deliberately
the *opposite* of `create_merge_request`:

- `create_merge_request` resolves the assignee **softly** (`lookup_user_id` →
  `Ok(None)`). There the MR is the point and the assignee is enrichment, so a bad
  username is dropped rather than allowed to block creation.
- `update_merge_request` resolves the assignee **hard** (`resolve_user_id` → errors).
  Here setting the assignee is the *entire reason the call was made*. Silently
  dropping an unknown username would report "Updated!" while changing nothing — the
  exact "errors are half the story" trap that produced the Sentry false-pages
  ([ADR fixed in v0.34.1 / v0.37.0], where `is_actionable_error` let a real failure
  read as success).

The tool also **echoes the resulting assignee** (`Assignee: @user`, or `(none)`) from
the API response, so the caller sees what actually happened rather than trusting a
bare acknowledgement.

## Consequences

- Open MRs can be assigned and reassigned, and unassigned, without recreating them.
- The same verb now has two resolution modes for the same field across two tools.
  That is intentional and worth stating plainly: **the right failure behaviour
  depends on whether the field is the goal or a garnish.** When the user's whole
  intent is the field, failing loudly is correct; when the field is incidental to a
  larger write, failing loudly would be obstructive. The choice is not "hard is
  safer" — it is "match the strictness to the intent."
- Success is now observable from the tool output, closing the loop the Sentry
  false-page work was about: don't let a write claim to have done something it
  didn't.
