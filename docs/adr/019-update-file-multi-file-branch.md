# ADR 019: update_file — stack multiple files onto one branch

## Status

Accepted (2026-06-29)

## Context

`update_file` always sent `start_branch` to the GitLab commits API. That's
correct only when creating the branch — once the branch exists, GitLab rejects
the request with `400 "A branch called 'X' already exists"`. So the tool could
commit exactly **one** file per fresh branch and never a second, making a
multi-file change (e.g. a README rewrite plus several ADR files in one MR)
impossible. Surfaced writing English docs to the `backend/golang/ip` repo: the
README committed, the first ADR failed.

A secondary bug: create-vs-update was checked against `source_branch` (main), so
a file already added earlier *on the feature branch* (but not on main) was
mis-classified as "create" and would collide.

## Decision

- Probe whether the target branch already exists
  (`GET /repository/branches/:branch`).
- Send `start_branch` **only when the branch does not exist** (i.e. when we're
  creating it); otherwise commit straight onto the existing branch.
- Check create-vs-update against the branch we're committing to, not
  `source_branch`.

## Consequences

- Multiple `update_file` calls now stack onto the same feature branch — a
  README plus N ADRs land as N+1 commits on one branch, ready for a single MR
  (squashed on merge).
- The protected-branch guard and MR-creation behaviour are unchanged.
- One extra lightweight request per call (the branch-existence probe) — cheap
  and worth it for correctness.
