# ADR 021: Fix get_job_log and make compact stripping fence-safe

## Status

Accepted (2026-06-29)

## Context

Two bugs surfaced while debugging a real CI failure through the tools:

1. **`get_job_log` always failed** with `JSON parse error: expected value at
   line 1 column 1`. It fetched the job **trace** via `client.get::<String>()`,
   which JSON-deserializes the body — but GitLab's `/jobs/:id/trace` endpoint
   returns **plain text**, so every call errored before returning any log.

2. **Compact mode corrupted file contents.** With `GITLAB_COMPACT=1`, the
   `tool_call!` wrapper runs `strip_markdown` over *every* tool's output,
   including `get_file_content` — whose payload is raw source wrapped in a code
   fence. Stripping `#` and `**` there silently rewrote the data: a commented
   `# RUN apt-get …` in a Dockerfile read back as an active instruction, and
   markdown files lost their headings. This produced a real, wrong conclusion
   (a "build-breaking apt-get line" that does not exist).

## Decision

1. Add `GitLabClient::get_text(path, params)` — a GET that returns the raw
   response body (`resp.text()`) with the same 429-retry handling, for endpoints
   that are plain text rather than JSON. Use it in `get_job_log` for the trace.
   Empty traces now return a clear "(log is empty …)" note.

2. Make `strip_markdown` **fence-aware**: lines inside ```` ``` ````/`~~~`
   fences are passed through byte-for-byte; only prose outside fences has
   emphasis/headings stripped and blank lines collapsed. File reads, job logs,
   and diffs are returned through fences, so they are now never mangled.

## Consequences

- `get_job_log` works (it was 100% broken before) and is debuggable.
- `get_file_content`, `get_job_log`, and diff tools return **faithful** content
  even under `GITLAB_COMPACT=1`; compact still strips prose in reports/dashboards
  for token savings. Net effect is strictly safer with no loss of compaction.
- Covered by `test_strip_markdown_preserves_code_fences`.
