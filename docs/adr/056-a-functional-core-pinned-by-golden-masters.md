# ADR 056: A functional core pinned by golden masters

## Status

Accepted (2026-09-25)

## Context

An audit of the codebase against common Rust practice found a set of problems that
were each small but shared one property: **they failed silently.** Nothing crashed;
the output was simply wrong, and nothing in the build said so.

- Nineteen tools reported caller errors as successful responses (`Ok("**Error:** …")`),
  so clients saw `isError: false` on a failed call.
- Two hand-rolled base64 decoders disagreed with each other; one truncated a file at
  the first bad character and scored whatever was left as if it were the whole file.
- Seven lint rules used lookaround syntax the `regex` crate rejects. They compiled to
  nothing and never fired, and the loader discarded the error.
- The file-quality score existed in two copies that had drifted: the same file could
  get a different grade from `analyze_file` and from `analyze_project`.
- The report ticket-reference check accepted any capital letters (`A-1`, `UTF-8`),
  while commit validation required a project key, so the two disagreed about the
  same commit.
- Several report fields were interpolated into HTML unescaped, including branch
  names, which git allows to contain `<` and `>`.
- Two of three median calculations took the upper middle element for even-length
  input.
- About sixty secondary API calls turned a failure into empty data with no trace,
  so an outage and a genuinely empty result rendered identically.
- CI ran the test suite with `|| true`, so no test failure could fail a build.

The report renderers were each several hundred lines of async code that mixed fetching
with rendering. They could only be tested against a live GitLab, so in practice they
were not tested at all.

## Decision

**Fetch in a thin async shell; render in a pure function.** Report and analysis tools
gather data and hand a plain struct to a renderer (`render_dev_report`,
`render_project_report`, `render_ai_adoption[_html]`, `analyze_content`,
`render_audit_report`). Renderers are split into one function per section. Aggregates
that two renderers both need (`file_facts`, the adoption `rollup`) are computed in
exactly one place.

**Pin renderer output with golden masters.** Each renderer's full output is compared
byte-for-byte with a snapshot under `tests/golden/`; today's date and the crate version
are normalised. The snapshots were taken *before* each split, so the refactors are
shown to preserve output rather than assumed to. Regenerate after an intended change
with `UPDATE_GOLDEN=1` and review the diff.

**Make failure loud.**
- Caller mistakes are `Err(Error::UserInput)`, which becomes an MCP tool error.
- Malformed base64 is an error, never a partial result.
- Invalid rule patterns and unparsable rule files are logged, and a test gate fails the
  build on them.
- Optional fetches use `or_default_logged()`, which keeps the best-effort fallback but
  logs the call site (4xx at debug, anything else at warn).
- CI tests are blocking.

**Encode invariants in types.** `Grade`, `RuleSeverity` and the security `Severity` are
enums with `Ord` giving report order, replacing string matching and hand-kept order
arrays. A misspelt severity in a rule file is now a parse error. Floats sort with
`total_cmp`.

Smaller mechanical changes: compile-once regexes (`LazyLock`), `write!` instead of
`push_str(&format!(..))`, `tokio::fs` instead of blocking `std::fs` in async code,
character-safe string truncation, a binary that uses the library crate instead of
compiling every module a second time, and shared helpers replacing duplicated ones
(`format_size`, protection-level names, `median`).

## Consequences

- Report logic can be tested on plain values, without mock servers. Coverage of the
  renderers is now real instead of nominal.
- Any change to report output fails a golden test until the snapshot is regenerated
  on purpose, which puts output changes into code review.
- Behaviour changes that callers can see:
  - Error responses are now flagged `isError`.
  - The report's ticket-reference rate may fall, because it now applies the same
    definition as commit validation.
  - Swift files count `init(` initializers as functions, which were previously never
    detected.
- Not yet split: `generate_team_report`, `compare_developers`,
  `analyze_pipeline_failures` and `analyze_project` still mix fetching with rendering.
  They get the same treatment next, snapshot first.
