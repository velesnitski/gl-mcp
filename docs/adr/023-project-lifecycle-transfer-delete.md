# ADR 023: Project lifecycle — transfer, delete, namespace-by-path

## Status

Accepted (2026-07-01)

## Context

`create_project` was the only project-lifecycle tool, and it took a **numeric**
`namespace_id`. In practice a project was created one level too high — in the
org-wide parent group instead of the intended subgroup — because the numeric id
is easy to transpose and there is no in-tool way to look a subgroup's id up.
There was then **no way to recover**: no transfer and no delete tool, so the
stray project could neither be moved nor removed programmatically. Placement is
not cosmetic here: subgroup membership differs from the parent, so a repo in the
wrong namespace is invisible to exactly the people who need it.

## Decision

Add the missing lifecycle operations and fix the root cause:

- **`transfer_project`** — `PUT /projects/:id/transfer`. `namespace` is a numeric
  id **or a full path** (`group/subgroup`); it is resolved via `GET /groups/:ns`
  (which accepts both) and validated before the move, then the new
  `path_with_namespace` is echoed back. Non-destructive.
- **`delete_project`** — `DELETE /projects/:id`, gated by a required
  `confirm_full_path` that must equal the project's `path_with_namespace`. The
  tool fetches the project and refuses on mismatch, so an id typo cannot delete
  the wrong repo.
- **`create_project`** gains a `namespace` param (path or id, resolved/validated),
  preferred over `namespace_id`. Passing `my-org/devops` now works
  directly — no id lookup — which removes the failure mode above.

## Consequences

- Project placement is fixable (`transfer_project`) and removable
  (`delete_project`) from the tool surface, and mis-placement is largely
  prevented by path-based namespaces.
- Both new tools are in `WRITE_TOOLS` (blocked in read-only mode).
- `delete_project`'s confirm-path gate trades a little friction for a strong
  guard against destroying the wrong project.
- Deletion honors GitLab's deletion-protection window (a project may be
  *marked for deletion* rather than purged immediately); the tool says so.
