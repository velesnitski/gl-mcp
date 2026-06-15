# ADR 016: Build-profile optimization (disk + build speed)

## Status

Accepted (2026-06-15)

## Context

The local `target/` directory had grown to 18 GB (years of incremental cruft)
and sat around 2.3 GB even fresh. The repo itself is tiny (~5 MB; `target/` is
gitignored), so this is purely a local build-footprint and build-speed concern —
but the tool is rebuilt very frequently, and the release profile used full LTO
(`lto = true`) + `codegen-units = 1`, which minimizes binary size at the cost of
slow link times.

## Decision

Two `Cargo.toml` profile changes:

- **`[profile.release] lto = "thin"`** (was `true`). Thin LTO links far faster
  than full LTO with nearly identical optimized output. Measured: binary
  9.8 MB → 10 MB (+2%), with a markedly faster release link. `strip = true` and
  `codegen-units = 1` are kept (still want a small, well-optimized binary).
- **`[profile.dev] debug = "line-tables-only"`**. Keeps file:line in backtraces
  (so test failures stay legible) but drops local-variable debug info. Measured:
  `target/debug` 1.7 GB → 1.3 GB (~24%), tests still pass.

## Consequences

- Release builds are faster for a ~2% larger binary — the right trade for a
  frequently-rebuilt dev tool. Reversible by setting `lto = true` again if a
  minimal binary ever matters more than build speed.
- Debug builds lose step-debugger local-variable inspection; backtraces (the
  thing test runs actually rely on) are unaffected. Acceptable for an MCP/CLI
  tool that's rarely run under a debugger.
- These bound growth but don't make Rust's `target/` small — it's dominated by
  dependency artifacts. The operational lever remains: clear `target/debug`
  (or `cargo clean`) when not actively testing. Not enforced in-repo to avoid
  slowing anyone's iteration; a documented habit instead.
