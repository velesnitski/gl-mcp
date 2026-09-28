# ADR 057: Lifecycles that end, and a pending job that explains itself

## Status

Accepted (2026-09-28)

## Context

Two automation paths could be started through this server but not maintained.

- **Project access tokens** could be created but not listed or revoked. A credential
  you can mint but cannot inventory or withdraw is only half managed. Rotation, expiry
  review and cleanup after an incident all had to happen outside the tool.
- **Pipeline schedules** could be created and played, but not listed, changed,
  deleted, or given variables. A schedule that needed a new ref or cron had to be
  rebuilt by hand in the UI.

Separately, ADR 052 introduced `runner_verdict` to tell apart the four states that all
render as `pending (0s)`: no runner, all offline, tag mismatch, genuinely queued. It was
only reachable through `list_project_runners`, so the diagnosis had to be asked for.
`get_pipeline`, the view where a stuck job is actually seen, stayed silent.

## Decision

**Complete both lifecycles.**
- Tokens: `list_project_access_tokens` returns metadata only (GitLab never returns a
  value again) and flags active tokens that expire within seven days.
  `revoke_project_access_token` is irreversible, so it requires the token's exact name
  (`confirm_name`), the same pattern as `delete_project`. A mistyped id cannot revoke
  a neighbouring token.
- Schedules: `list_pipeline_schedules`, `update_pipeline_schedule`,
  `delete_pipeline_schedule`, `set_pipeline_schedule_variable` (upsert by key) and
  `delete_pipeline_schedule_variable`.

**Updates validate exactly as creation does.** `update_pipeline_schedule` shares the
cron and ref checks with `create_pipeline_schedule` (one implementation). A ref that
does not resolve is refused, because GitLab would save it and never fire. An empty
update is a user error, not a silent no-op.

**The schedule list names the other "accepted, never fires" state.** A schedule runs
as its owner. A blocked, deactivated or missing owner stops it from producing
pipelines while it still shows a next-run time, so the list marks such schedules
*will not run*.

**Schedule variable values are never echoed.** The response says created or updated,
and nothing more.

**`get_pipeline` checks runners only when a job is pending.** In that case it makes one
extra call to the project's runners and gives each pending job the `runner_verdict`
note. Pipelines with nothing pending pay nothing. Listing runners needs Maintainer
access, so a failed fetch is reported as *runner list unavailable*, never as *no
runners*. Reading "could not look" as "nothing there" is the absence-versus-empty
defect this server keeps removing.

`search_code`'s `ref_name`, flagged as unverified next to the `list_commits` alias bug
(ADR 053), is now pinned by a test: the query builder is a pure function, and a test
asserts that a given ref reaches GitLab as `ref`.

## Consequences

- Seven new tools (114 total). Five are write tools, blocked in read-only mode.
- New output is rendered by pure functions (`render_access_tokens`,
  `render_schedules`, `render_runner_check`), tested on synthetic data with a fixed
  date.
- `get_pipeline` makes one extra request when, and only when, a job is pending.
