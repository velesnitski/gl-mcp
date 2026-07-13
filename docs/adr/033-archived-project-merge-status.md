# ADR 033: Surface archived projects — merge_status alone is not trustworthy

## Status

Accepted (2026-07-10)

## Context

An **archived** GitLab project is read-only: pushes, comments, and merges are all
rejected. But GitLab's merge-request API keeps returning a normal
`detailed_merge_status` regardless — an MR in an archived project still reports
`mergeable`.

This bit us in practice. After opening twelve README merge requests, four were
reported `mergeable` and were tracked as "just awaiting a reviewer's click". Two
of them (`infrastructure/k8s`, `infrastructure/argocd`) were in projects that had
since been **archived** — they could never be merged, and the repos were being
retired. The signal that something was wrong came from a human noticing the
GitLab UI banner ("This project is archived and cannot be commented on"), not
from the tool.

Compounding it, `get_project` did not surface `archived` at all, so there was no
way to check even after suspecting it.

## Decision

1. **`get_project`** prints a prominent `⚠️ ARCHIVED — read-only` line when the
   project is archived. It is the single most consequential project attribute for
   any write operation, so it goes above the normal fields.

2. **`get_merge_request`** no longer relays `merge_status` uncritically. The MR
   payload has no archived flag, so the project is fetched and, when archived,
   the status line becomes:

   > **Merge status:** ⚠️ **BLOCKED — project is ARCHIVED** (read-only). GitLab
   > reports `mergeable`, but any merge will be rejected.

   The original GitLab value is still shown — we correct the interpretation
   rather than hide the raw data.

   Cost is bounded deliberately: the lookup runs **only while the MR is open**
   (for a merged/closed MR the flag changes nothing) and goes through
   `get_cached` with a 60s TTL, so checking many MRs in one project costs one
   extra request, not one per MR.

## Consequences

- A "mergeable" verdict from `get_merge_request` can now be trusted; the tool
  no longer sends callers to merge something that cannot be merged.
- Archived state is visible wherever it matters, instead of only in the web UI.
- Slight extra cost on open MRs (one cached project fetch), accepted as the price
  of a status that does not lie.
- General lesson, worth remembering for other tools: **an upstream field being
  present is not the same as it being true.** Where GitLab's own model has a
  known blind spot, the tool should reconcile it rather than pass it through.
