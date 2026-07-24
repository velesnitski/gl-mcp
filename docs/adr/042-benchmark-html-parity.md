# ADR 042: Industry Benchmark — one source, both output paths

## Status

Accepted (2026-07-24)

## Context

ADR 041 (v1.4.0) added the **Industry Benchmark** section — maturity tier,
config coverage, depth, and top-3 gap-driven suggestions. It was written into
the **markdown** builder (`get_ai_adoption`) only. The **HTML** generator
(`generate_ai_adoption_report`) — the path behind the `--adoption-report` CLI
and the emailed weekly report — never rendered it.

Result: the audience the benchmark was designed for (leads receiving the HTML
email) saw the raw metrics but not the verdict or roadmap. Discovered by
running the real report against a live group and diffing the section headers:
`AI Adoption Report → Adoption Levels → By Team → …`, no benchmark.

This is the same failure class as ADR-034 (linked-issue state fixed in the
report renderer but not `normalize_issue`): a feature added to one of several
parallel output paths, with no shared code or cross-path test to force parity.

## Decision

Extract the benchmark into **one computation + two thin renderers**, so the two
formats cannot diverge:

- `compute_benchmark(results, org_dev_pct, ai_devs, all_devs) -> Benchmark` —
  pure; owns the tier/coverage/depth logic and the suggestion *selection*
  (which advice applies, in what order, truncated to 3).
- `render_benchmark_md(&Benchmark) -> Vec<String>` and
  `render_benchmark_html(&Benchmark, esc) -> String` — presentation only.
- Both `get_ai_adoption` and `generate_ai_adoption_report` call
  `compute_benchmark` and their respective renderer. The HTML section sits
  right after the headline cards, before the funnel (a `#benchmark` anchor).

Suggestions are stored as **plain prose** in `Benchmark` (shared, no markdown),
so the two renderers can't disagree on wording; each adds its own emphasis
(markdown numbered list; HTML `<ol>`). The reference-band caption is a shared
`const`.

A test (`benchmark_rendered_by_both_paths`) asserts **both** formats emit the
`Industry Benchmark` heading and the tier — the guard that would have caught
the original omission.

## Consequences

- The emailed HTML report now leads with the verdict, matching the markdown
  tool and ADR 041's intent.
- The markdown suggestions lose their inline `**bold**`/`` `code` ``
  decoration (now plain prose); immaterial for the secondary MCP-tool path and
  untested previously.
- Patch release **1.4.1**. 233 tests pass (2 new).
- General rule reinforced: a report feature is not "shipped" until every output
  path renders it and a test pins parity — same lesson as ADR-034.

## Related

- ADR 041 — the benchmark this makes reachable in HTML
- ADR 034 — prior instance of the same "one path updated, others not" class
