# ADR 011: Reverse drift — fragment-assembled routes

## Status

Accepted (2026-06-15)

## Context

ADR 010 noted a blind spot: the code-side endpoint harvester only saw whole
path literals (`"/v3/user"`). Two real patterns slipped through:

- **Split across fragments** — `"/v3" + "/user"`. The harvester emitted two
  bogus endpoints (`/v3`, `/user`) instead of one (`/v3/user`).
- **Interpolated middle segment** — `"/users/\(id)/posts"`. Stripping the
  interpolation left a double slash (`/users//posts`).

A live run against the iOS network layer confirmed its routes are clean leading-
slash literals (even `URLStrings.ipUrl + "/ip"` was caught, since `/ip` is still
a literal), so this is a forward-looking robustness fix for other repos and
platforms, not a fix for a miss observed there.

## Decision

`harvest_path_literals` now does two passes per line:

1. **Concatenation runs.** A run of 2+ string literals joined by `+` is stitched
   into one path before normalizing, so `"/v3" + "/user"` → `/v3/user`. The run's
   character span is recorded.
2. **Standalone literals.** Whole leading-slash literals are harvested as before,
   but skipped if they fall inside a stitched run (no double-emit of the pieces).

`normalize_code_path` additionally collapses repeated slashes left by removing an
interpolated/`{param}` segment, so `/users/\(id)/posts` → `/users/posts`.

A single literal preceded by a variable (`base + "/orders"`) is unchanged — only
one literal, so it's not a concatenation run; the literal tail `/orders` is still
harvested.

## Consequences

- Two literals where neither is a usable path (`"Bearer " + token`,
  `"application/json"`) still produce nothing — the concatenation only emits when
  the joined result normalizes to a leading-slash path. No new noise.
- A run mixing a literal and a variable (`"/v3" + "/" + resource`) stitches the
  literal parts (`/v3`) and drops the variable — an over-simplification, but
  better than emitting a split. Truly variable-assembled routes (array joins,
  fully-dynamic paths) remain out of reach without dataflow analysis; that's the
  residual, documented limit.
