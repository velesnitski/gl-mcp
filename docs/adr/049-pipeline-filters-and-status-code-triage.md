# ADR 049: Source filtering, log search, and status-code triage

## Status

Accepted (2026-08-18)

## Context

Three gaps left over from the ADR 046–048 series, all surfaced by using the pipeline
tools rather than by reading them.

**1. The population fix was not reachable from outside.** ADR 048 established that
child pipelines (`source=pipeline`) must not be counted as independent runs. But
`list_pipelines` had no `source` parameter, so a caller could not ask the question the
analysis had just proven was the right one — the correction lived inside one tool and
nowhere else.

**2. Whole logs were pulled to read one line.** `get_job_log` returned a tail. When the
decisive error sat above the tail window the only recourse was a larger tail, which
means paying for thousands of lines to reach one. Job logs are the single largest
response this server produces.

**3. Numeric status markers could not fire.** `failure_class` listed `502`/`503`/`504`
as transient markers tested against the cluster signature — but signatures are produced
by `normalize_signature`, which masks runs of two or more digits so that identical
faults cluster together. The codes were erased before they were tested. The markers
matched only on the short-signature fallback path, where masking is skipped, and were
inert everywhere else.

## Decision

**`source` on `list_pipelines`**, passed through to the GitLab API, so child pipelines
can be excluded or selected deliberately. The applied filter is echoed in the response
header — a filtered count that looks like a total is the failure mode this series keeps
returning to.

**`pattern` on `get_job_log`**: a case-insensitive regex that searches the *whole* log
and returns only matching lines, numbered, capped at `tail` matches and keeping the
**last** ones. The response states total matches against total lines, so a truncated
result cannot be mistaken for a complete one. An invalid regex is reported as such
rather than surfacing as a tool error.

**Status codes read from the log, not the signature.** A dedicated matcher extracts
HTTP status codes and maps them to classes: 408/425/429/500/502/503/504 transient,
400/401/403/404/405/409/422 config. It runs after the state-drift checks, so a delete
that 404s is still reconciliation rather than misconfiguration. A bare three-digit
number is not accepted as evidence — a code counts only next to a status word or with
its canonical reason phrase, so "took 403 ms" is not a permission error. The numeric
entries were removed from the signature word list, leaving one owner for codes.

## Consequences

- The correct population is selectable by any caller, not just by one analysis tool.
- Finding one line in a large log costs one search instead of the whole log.
- Auth, throttling and validation failures are triaged by the code that states them.
- Method note, fourth in this series: **a marker that is tested after normalization has
  destroyed it is not a conservative default, it is dead code.** The word lists looked
  thorough and were partly inert; the defect was invisible because a classifier that
  returns `unknown` looks the same whether it is being careful or being broken.
