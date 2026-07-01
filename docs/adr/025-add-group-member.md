# ADR 025: add_group_member — grant group-wide access

## Status

Accepted (2026-07-01)

## Context

`add_member` (ADR 024) grants access to a single project. But access is often
better managed at the **group** level: adding a person to a group grants them
every project in it, current and future, in one step — the right tool when
someone should see a whole team's work rather than one repo. GitLab exposes this
via a different endpoint (`POST /groups/:id/members`) than project membership.

## Decision

Add **`add_group_member`** (`POST /groups/:id/members`), write-guarded, sharing
the same ergonomics as `add_member`:

- **`group_id`** accepts a numeric id or a full path (e.g. `my-org/devops`).
- **`user`** is a username (leading `@` optional) or id — resolved via the same
  `resolve_user_id` helper.
- **`access_level`** is a role name (guest/reporter/developer/maintainer/owner)
  or number, via the same `parse_access_level` helper.
- Optional **`expires_at`** (YYYY-MM-DD).

The response states plainly that the grant covers all projects in the group.

## Consequences

- Group-wide access is a one-liner and needs no id lookups.
- Project vs group membership is now a deliberate choice: `add_member` for a
  single repo, `add_group_member` for a whole team/subgroup.
- Both reuse the shared user/level helpers, so behaviour and error messages stay
  consistent across the two tools.
