# ADR 060: Tested against an outage

## Status

Accepted (2026-10-14)

## Context

Line coverage was 39%. Nearly all of the untested code sat in the async tools, behind
a network call, and the project does not use mock servers: they test the mock, and
they drift from the real API without anyone noticing.

That left the most important behaviours unverified:

- what each tool does when GitLab does not answer;
- whether the published tool surface (names, parameters, schemas) changes between
  releases;
- whether the string helpers that guard output are correct on unusual input. These
  are the HTML escaper, the truncation helpers and log redaction.

## Decision

Five kinds of test, none of which needs a mock server or a network.

1. **End-to-end over the MCP protocol, with GitLab down.** The real server runs
   in-process on one end of an in-memory pipe, and a real MCP client sits on the
   other. GitLab points at a localhost port that was just released, so every request
   is refused on the spot. The test:
   - lists the tools over the wire;
   - calls every one of them with arguments generated from its own input schema;
   - requires each call to answer without panicking or hanging;
   - requires none of them to report success with data it could not have fetched.

   `HOME` is a temporary directory, so nothing touches real files.
2. **A contract snapshot of the tool surface.** Every tool's name, description and
   input schema are recorded in `tests/golden/tool_surface.json`, as received over
   the protocol. A rename or a removed parameter fails the build and shows up as a
   reviewed diff.
3. **Property-based tests (proptest)** for the output guards and the arithmetic:
   - escaping round-trips and leaves no markup;
   - truncation never splits a character;
   - base64 survives GitLab's line wrapping;
   - redaction removes token-shaped secrets wherever they sit;
   - scores stay in range and never rise with a violation;
   - every line of a file is classified exactly once.
4. **Golden snapshots of the read tools' renderers.** Rendering is split from fetching
   (functional core, I/O shell) for 28 read tools and fed synthetic responses.
5. **Byte-level source hygiene.** Control characters anywhere in source, rule files or
   docs fail the build.

The first run of these tests found real defects, which are fixed here:

- **Nine fan-out tools rendered an outage as an empty result.** They answered "no
  users", a table of zeros, or an all-clear security audit. A shared `FanOut`
  tracker now distinguishes three cases:
  - a lookup that answered "not found" is treated as empty;
  - a lookup that failed for any other reason is an outage;
  - if every lookup failed, the tool fails; if only some did, the result says how
    many.
  The cross-instance dashboard returned per-instance error text as a successful
  result; it now fails when every instance fails.
- **The CI security audit could not tell "unreadable" from "clean".** A refused
  variables listing counted as "no findings". The report now names the projects
  whose variables could not be read.
- **Token redaction in job logs matched nothing.** A literal backspace (0x08) sat in
  the token pattern where `\b` was meant. It was invisible in an editor and in a
  diff. The pattern is fixed, and the hygiene test keeps this class of defect out.
- **Spec sweeps given no targets** rendered a confident report about nothing. That
  is now a user-input error.

## Consequences

- Coverage rose from 39% to above 60% of lines. Most of what remains is fan-out
  orchestration in `merge_requests`, `commits` and `reports`.
- Mutation testing (cargo-mutants, run narrowly and single-threaded) grades the new
  tests. Surviving mutants are tracked as follow-ups rather than chased to zero.
- Any tool added later is exercised by the outage sweep and recorded in the contract
  snapshot automatically, with no new test code.
