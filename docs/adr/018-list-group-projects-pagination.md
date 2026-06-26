# ADR 018: Paginate list_group_projects

## Status

Accepted (2026-06-26)

## Context

`list_group_projects` is documented as "List all projects in a GitLab group
(including subgroups)" and its `per_page` param as "Max results". But it issued a
single `client.get` with `per_page` passed straight through — and GitLab clamps
`per_page` to a maximum of 100. So a call with `per_page=300` on a group with
111 projects silently returned only the first 100, with no indication of
truncation. Surfaced while enumerating the org to find Go repos: the listing
capped at 100 and dormant repos beyond it were missing.

## Decision

Use the existing `get_all_pages` helper (which loops pages at per_page=100) and
treat `max_results` as the cap it claims to be:

- `max_pages = ceil(cap / 100)`, hard-ceilinged at 20 (2000 projects).
- Fetch all pages, then truncate to `cap`.
- When the result is truncated, the header notes it ("capped — pass a higher
  per_page for more") so silent loss is impossible.
- Default raised 50 → 200, since the tool's job is enumeration — most groups now
  return in full by default.

## Consequences

- Groups with >100 projects now list completely (up to the cap). The org's full
  ~111-repo tree returns in one call.
- The 2000 hard ceiling bounds worst-case API cost (20 pages) for a pathological
  group; the header flags when it's hit.
- A related single-page limitation remains in `get_tree` with `recursive=true`
  (caps at one page of 100 entries) — lower priority since full recursive trees
  of large repos are rarely needed; left for a follow-up if it bites.
