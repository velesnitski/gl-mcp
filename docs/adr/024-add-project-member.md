# ADR 024: add_member — grant project access

## Status

Accepted (2026-07-01)

## Context

After adding project lifecycle tools (ADR 023), a project can be created, moved,
and deleted from the tool surface — but there was still no way to **grant a
person access** to it. Placing a repo in the right namespace is only half the
job; the people who need it must also be members. The raw GitLab endpoint needs
a **numeric** `user_id` and a **numeric** `access_level`, neither of which is
convenient to supply by hand.

## Decision

Add **`add_member`** (`POST /projects/:id/members`), write-guarded, that removes
both friction points:

- **`user`** accepts a **username** (leading `@` optional) or a numeric id;
  usernames are resolved via `GET /users?username=…` (mirroring `get_user`).
- **`access_level`** accepts a **role name** — guest / reporter / developer /
  maintainer / owner (also planner) — or the numeric level (10/20/30/40/50).
- Optional **`expires_at`** (YYYY-MM-DD) for time-boxed access.

It echoes back the resolved username, id, and role so the grant is confirmed.

## Consequences

- Access can be granted directly (`add_member(project, "@user", "developer")`)
  without looking up ids or level numbers.
- Scope is intentionally **project members** (the common case). Group membership
  uses a different endpoint (`POST /groups/:id/members`) and can be added later
  as `add_group_member` if needed.
- An invalid role name fails fast with the list of accepted values; an unknown
  user fails with a clear "not found".
