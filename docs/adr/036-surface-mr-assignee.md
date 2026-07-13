# ADR 036: If a field can be set, its absence must be visible

## Status

Accepted (2026-07-13)

## Context

[ADR 035](035-update-mr-assignee.md) gave `update_merge_request` the ability to set
an MR's assignee. But nothing in the read path showed the assignee:
`get_merge_request` printed author, pipeline, merge status and timestamps but never
the assignee, and `list_merge_requests` was the same.

The consequence showed up immediately. Asked to "assign the MRs to team leads *where
not already assigned*", there was no way to check the "where not already assigned"
condition through the tools. The state was simply invisible. Rather than admit that,
the assignee was set on *all* the MRs on the assumption they were unassigned — which
overwrote assignments a human had already made. A capability to write a field had
shipped without the matching ability to read it, and the gap directly produced a
wrong action.

## Decision

Show the assignee in both read paths, via one shared `assignee_display` helper:

- `get_merge_request` gains an **`Assignee:`** line, right after `Author:`.
- `list_merge_requests` shows the assignee on each row (both the detailed and the
  compact pipe-delimited views).

The helper reads the modern `assignees` array (joining multiple with commas) and
falls back to the deprecated single `assignee` field.

Crucially, an unassigned MR renders **`(unassigned)`**, not an empty string. The
blank-when-absent rendering is exactly what made the state invisible; spelling it out
is the fix.

## Consequences

- "Which of these MRs is already assigned, and to whom?" is now answerable from a
  single `list_merge_requests` call, so an "assign only where unassigned" workflow can
  actually check its precondition instead of guessing.
- The read and write surfaces for the assignee field are now symmetric.
- General principle, and the reason this ADR exists beyond the one-line fix: **when a
  tool can set a field, it must also show that field — including, especially, when the
  field is empty.** A write capability without the matching read leaves the caller
  unable to verify preconditions or confirm results, and "I couldn't see it so I
  assumed" is how that gap turns into a wrong write. This sits alongside the
  same-session lessons in [ADR 033](033-archived-project-merge-status.md) (a present
  field is not necessarily a true one) and [ADR 035](035-update-mr-assignee.md) (make
  success observable): all three are the tool refusing to let the caller act blind.
