# ADR 054: Advice derived from the schema, and windowed file reads

## Status

Accepted (2026-09-02)

## Context

Every response over the size threshold carried the same sentence: *"Use
`summary_only=true` or filter parameters to reduce token usage."* It is emitted by the
`tool_call!` wrapper, so it fired for all 106 tools — while only 19 parameter structs
have `summary_only`. On the other ~87 the server was recommending a parameter it does
not accept. A caller who followed the advice got a no-op or an error.

That is the defect class ADR 049 and ADR 053 both addressed, turned inward: the tool
stating something untrue about *itself*. Lower stakes than the `search_code` "regex
supported" claim, which produced a false conclusion — this one only wastes a call — but
identical in shape, and it is the server's own guidance.

Fixing the sentence alone would have left a worse problem behind. On
`get_file_content` there was no way to reduce the response **at all**: no line range,
no pattern, no summary. An accurate warning there would read "this is large and there
is nothing you can do", which is honest and useless. Meanwhile `search_code` returns
file paths *with line numbers*, and the only way to look at one was to pull the entire
file.

## Decision

**The advice is derived from the tool's own parameter schema.** `shrink_hints_for`
inspects the `JsonSchema` of the params struct and names only the levers that are
actually present, ordered by how much they help. A hand-kept table beside the structs
would drift straight back into the original bug; the schema cannot, because it is the
same artifact the tool advertises to callers. When a tool has no lever, the warning
says so plainly rather than inventing one. The lookup is evaluated lazily, inside the
oversized branch, so it never runs on the common path.

**`get_file_content` gains the capability the warning implies:** `start_line`/`end_line`
for a window, and `pattern` for a regex search across the whole file. A window is
returned with its **original line numbers**, so it can be quoted without re-fetching,
and the header always states the file's true line count — a window must never be
mistaken for the whole file. A `start_line` past the end is an **error**, not an empty
success: silently returning nothing for line 900 of a 200-line file is the same
absence-reads-as-a-value failure this series keeps correcting. An end past the file
clamps, because that is unambiguous.

## Consequences

- Guidance a caller follows now works, on every tool.
- A line number from `search_code` can be read directly instead of pulling the file.
- Method note, ninth in the series: the previous fixes made the tool honest about the
  **data**. This one makes it honest about **itself** — and shows the two are linked,
  because accurate advice was only worth giving once the capability existed to advise.
