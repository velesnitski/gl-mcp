# ADR 038: `update_file` can initialize an empty repository

## Status

Accepted (2026-07-17)

## Context

`update_file` assumed every repo already has history: it commits to a feature
branch created with `start_branch = from_branch` (default `main`). In an **empty**
repo — no commits, no default branch — this is impossible:

- there is no `main` to branch a feature off, and
- `start_branch=main` is rejected because `main` does not exist.

This surfaced during a documentation campaign: several freshly-created,
never-committed-to repos could not get a README through the tool at all, and had to
be flagged as "needs a human `git init`."

A naive fix — "just drop `start_branch`" — collides with the protected-branch
guard (`repository.rs`), which refuses to write to `main`/`master`/`develop` and
tells the caller to use a feature branch. But an empty repo's first commit *must*
create a branch, and that branch is usually `main`. The guard's premise (a feature
branch already exists to protect `main` from) is false before the repo has any
history, so the guard would block the one write that is legitimate.

## Decision

Detect the empty repo up front via `GET /projects/:id` → `default_branch`
(null/empty ⇒ empty). When empty:

1. **Bypass the protected-branch guard** — creating the default branch is the only
   way to initialize the repo.
2. Commit the file **directly to `from_branch`** (the intended default, normally
   `main`) with **no `start_branch`** — GitLab creates that branch from the first
   commit.
3. **Skip the MR**, even if `create_mr` was requested: this commit *is* the base;
   there is nothing to merge against. The response says so explicitly.

Non-empty repos are unchanged — the guard and feature-branch flow still apply.

## Consequences

- Empty repos can be documented/populated through the tool instead of being
  punted to a manual `git init`.
- The protected-branch guard now has a principled carve-out rather than a blanket
  rule, scoped exactly to the case where its premise does not hold.
- One extra `GET /projects/:id` per `update_file` call (cheap, and it also lets us
  give an honest "MR skipped" note). Acceptable for a write operation.
