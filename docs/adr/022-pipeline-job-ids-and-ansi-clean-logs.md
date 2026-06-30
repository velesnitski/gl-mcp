# ADR 022: Job IDs in get_pipeline, ANSI-clean job logs

## Status

Accepted (2026-06-30)

## Context

Debugging a CI failure through the tools surfaced two friction points (distinct
from the bugs fixed in ADR 021):

1. **`get_pipeline` listed jobs by name/status but not their numeric IDs**, yet
   `get_job_log` *requires* a `job_id`. There was no in-tool way to get from a
   pipeline to its job log — the only workaround was scraping the job id from the
   raw API with a token, which is awkward and trips secret-handling guards.

2. **`get_job_log` returned the trace with raw ANSI escapes** (`\x1b[0K`,
   `\x1b[32;1m`, …). They are unreadable, and under the `tail` limit the control
   codes crowd out real log lines and waste tokens.

## Decision

1. `get_pipeline` now prints each job's **numeric id** (`(job <id>)`) and, for
   failed jobs, the GitLab **`failure_reason`** (e.g. `script_failure`). The
   `get_pipeline → get_job_log` flow is now self-contained.

2. `get_job_log` strips ANSI escape sequences and carriage returns from the
   trace (`strip_ansi`, a small dependency-free scanner) before tailing it. It
   also shows `failure_reason` in the job header.

## Consequences

- Pipeline triage is a two-step, no-token flow: read the pipeline, copy the
  failed job's id, fetch its (now clean) log.
- Logs are readable and cheaper; `tail` shows more signal per line.
- `strip_ansi` only removes CSI sequences / `\r`; ordinary `#`/text is untouched,
  and it composes with the fence-safe compact pass from ADR 021. Covered by
  `strips_csi_color_and_erase_codes` and `leaves_plain_text_untouched`.
