# ADR 045: Three silent wrong answers — don't render data the API never supplied

## Status

Accepted (2026-07-30)

## Context

A live smoke-test of the read surface after a GitLab upgrade found three tools
returning confident, well-formed output that did not mean what it appeared to
mean. None of them errored. All three were shipped in v1.5.1 alongside an
unrelated fix; this ADR documents the reasoning that commit did not carry.

The shared failure mode: **a missing or absent value rendered as a real one.**
That is worse than an error, because an error gets investigated and a plausible
number gets quoted.

### 1. `check_branch_protection` conflated "unprotected" with "does not exist"

`GET /projects/:id/protected_branches/:branch` returns 404 both when the branch
exists without a protection rule *and* when the branch is not there at all. The
handler treated every 404 as the former and answered `Branch 'X' is not
protected.`

Observed: asking about a deliberately invented branch name produced exactly the
same sentence as asking about a real unprotected branch. Asking about `main` on
a project whose default branch is something else also produced it.

For a compliance check this is the expensive direction of wrong. "Not protected"
reads as a real gap: it invites someone to add protection to a branch that never
existed, or to record a finding against a repo that is actually fine — while a
genuine typo goes unnoticed.

### 2. `get_contributors` rendered Additions/Deletions columns that are always 0

GitLab's contributors endpoint reports `additions` and `deletions` as `0` for
every entry, on every project. The tool read them anyway, so each row ended
`+0 -0` and the header read `Total: N commits, +0 -0`.

Verified across more than one project: the zeros are structural, not a quiet
repository. "This project changed no lines" and "the API does not supply this"
are opposite claims, and the output made them indistinguishable.

### 3. `list_environments` promised deploy info the list endpoint doesn't return

`last_deployment` is not part of the environments *list* payload — only the
single-environment endpoint carries it. The tool read `env["last_deployment"]`
off each list entry, found null, and printed `no deployments`.

Observed: every environment reported `no deployments`, including live ones with
an external URL. The tool's own description promises "last deployment info (SHA,
branch, status, deployer)", so the one thing it advertised never appeared.

## Decision

**1. Probe before answering.** On a 404 from the protected-branches endpoint,
check the branch itself and answer the question actually asked:

- branch present → `exists but is NOT protected`
- branch absent → `does not exist … the default branch is not always 'main'`
- probe itself failed → state the protection fact and say existence is unverified

One extra request, only on the 404 path.

**2. Drop the columns.** `get_contributors` reports commits and share only, with
one line stating why line counts are absent and where to get them (per-commit
diff, or the aggregated churn in the dev-report tools). Removing a column is
better than keeping one that can only ever say zero.

**3. Resolve deployments from the feed.** Read the project's deployments
endpoint once, newest-first, and take the first sighting per environment name.
That is a single extra request for the whole project rather than one per
environment, and it keeps the advertised behaviour. The list payload is still
preferred when present, so a future GitLab that includes `last_deployment`
needs no change here.

## Consequences

- Three tools now distinguish "no data" from "zero", which is the distinction
  each of them was silently collapsing.
- `check_branch_protection` costs one extra request when a branch has no
  protection rule; `list_environments` costs one extra request per call. Both
  were judged worth it: the previous answers were not merely incomplete, they
  were misleading.
- Output shape changed for two tools (a narrower contributors table, real deploy
  lines), so anything parsing them positionally needs a look.

## Not included

- **Computing real per-contributor line counts.** It needs a walk over commit
  diffs — orders of magnitude more expensive than the endpoint it would replace.
  The dev-report tools already do this where the cost is justified.
- **Auditing the remaining read tools for the same pattern.** Three were found
  by spot-checking the surfaces most sensitive to API drift; a systematic sweep
  of every tool against a live instance is a separate exercise.
