# ADR 039: `merge_mr` interprets its failure codes

## Status

Accepted (2026-07-17)

## Context

`merge_mr` propagated the raw HTTP error from GitLab's merge endpoint, so a failed
merge surfaced as `GitLab API error (401)` or `GitLab API error (405)` with no
guidance. During the README campaign this cost real time: merging across many
groups produced a stream of **401**s and **405**s that had to be reverse-engineered
by hand (401 = "I'm only a Developer here", 405 = "CI still running") and then
confirmed with a separate `get_merge_request` call.

GitLab's merge endpoint has well-defined failure modes, so the status code already
carries the answer — it just wasn't being read.

## Decision

Catch the `Error::GitLab { status, .. }` from the merge call and translate the
known codes into actionable messages, preserving GitLab's original text:

- **401 / 403** → "you lack permission to merge here — Maintainer role is required
  on this project/group."
- **405** → "the MR is not in a mergeable state — pipeline may still be running,
  required approvals may be missing, conflicts, or draft. Run `get_merge_request`
  for the detailed merge status."
- **406** → "merge conflict — the source branch needs rebasing (see `rebase_mr`)."

Interpreted cases are returned as `UserInput` errors (they are actionable by the
caller, not tool faults); unknown codes pass through unchanged.

## Consequences

- A failed merge now tells you *why* and *what to do next*, instead of a bare
  status code.
- No change to the success path or the tool's output shape; this only enriches the
  error message. Purely a DX fix.
- The mapping is deliberately small and only covers the merge endpoint's documented
  codes; anything else still surfaces the raw GitLab error rather than guessing.
