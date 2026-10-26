# ADR 073: Pipelines that never start — why, and starting one with variables

## Status

Accepted (2026-10-26)

## Context

Two failure shapes left the pipeline tools with nothing useful to say.

1. **A pipeline that fails with zero jobs.** No job means no log. REST's
   `yaml_errors` is the only reason REST carries, and it is often empty even when
   GitLab rejected the configuration. The pipeline page shows "failed" and nothing
   else.
2. **A job that stays pending.** The runner check (ADR 051) decides whether any
   online runner can take the job. It read `tag_list` and `run_untagged` from the
   project runner list, but that list does not return either field. A missing
   `run_untagged` defaulted to `true`, so a runner that accepts only tagged jobs was
   reported as able to take an untagged one: "genuinely queued", for a job that
   could never start.

A third gap made both slower to work around: there was no tool to start a pipeline
with variables, the usual switch for jobs whose `rules` test a variable.

## Decision

1. **Zero-job failures are explained in `get_pipeline`.**
   - `yaml_errors` is shown when present.
   - Otherwise GraphQL `failureReason` is fetched; REST does not expose it.
   - When that reason is a configuration error, or unknown, CI Lint is run against
     the pipeline's own commit (`content_ref` = its SHA), and its errors name the
     offending keys. A valid lint result says the file is not the cause.
   - Every lookup is best effort: a failure is reported in the output, never fails
     the tool.
2. **Runner facts come from each runner's own record.** Fields the project list
   omits are filled in from `GET /runners/:id` (at most 25 runners, concurrently).
   A record the token may not read stays incomplete. Its `run_untagged` is then
   unknown, never assumed, and the verdict becomes "cannot tell" instead of a guess
   in either direction. A paused runner is not counted as able to take jobs.
3. **`create_pipeline`** (write tool) starts a pipeline on a ref with optional
   variables.
   - Variable keys are validated; values are never echoed back, since they may be
     secrets.
   - GitLab's 400 for a pipeline it will not create is explained: rules that leave
     no job, an invalid configuration, or a missing ref.

## Consequences

- `get_pipeline` makes up to three extra calls, only for a failed pipeline with zero
  jobs.
- The runner check makes one extra call per runner whose list entry lacks the
  fields, only when a job is pending or `list_project_runners` is called.
- A token that cannot read runner records now gets "cannot tell" where it used to
  get a confident, possibly wrong, "queued".

## Addendum (2026-10-27): the empty-pipeline refusal

GitLab 18 refuses a pipeline whose rules leave no job with "The resulting pipeline
would have been empty. Review the rules configuration for the relevant jobs.", as
JSON (`{"base": [...]}`). The classifier did not know that wording and, seeing
"configuration", called the file invalid. Rules wording is now matched first and
includes this phrasing, and the JSON envelope is reduced to its text.
