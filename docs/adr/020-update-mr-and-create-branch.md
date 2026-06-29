# ADR 020: update_merge_request and create_branch tools

## Status

Accepted (2026-06-29)

## Context

Two gaps surfaced while doing real MR/branch work:

- **No way to edit an MR after creation.** `create_merge_request` exists, but a
  title/description that needs fixing (e.g. an MR that grew to include more than
  its title says) could only be changed in the GitLab UI.
- **No `create_branch`.** `delete_branch` existed, but branch creation was only
  a side effect of `update_file` — there was no explicit way to create a branch
  (e.g. to stage one before a series of commits, or for a release branch).

## Decision

Add two write tools (registered in `WRITE_TOOLS`, gated by `write_guard!`):

- **`update_merge_request`** — `PUT /merge_requests/:iid` with only the
  non-empty fields among title, description, labels, target_branch. Refuses a
  no-op (all fields empty) rather than issuing an empty update.
- **`create_branch`** — `POST /repository/branches?branch=&ref=`, defaulting the
  source ref to `main`. Returns the new branch and its head SHA.

## Consequences

- MR metadata is now fully manageable from the tool surface (create → update →
  merge/close), not just creatable.
- Branch lifecycle is symmetric (create + delete), and `create_branch` pairs
  naturally with the now-stackable `update_file` (ADR 019) for scripted,
  multi-file branches.
- `update_merge_request` intentionally omits assignee/reviewer (those need
  username→ID resolution) and draft (GitLab toggles that via a title prefix);
  they can be added later if needed.
