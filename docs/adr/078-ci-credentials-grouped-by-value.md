# ADR 078: CI credentials grouped by value

**Status:** Accepted
**Date:** 2026-11-11

## Context

ADR 077 reports each literal credential in a CI file as its own finding.
CI definitions are often copied between repositories from a shared template, so
one exposed password can appear in many files under different names. Listed per line,
the report reads as many problems and hides the one action that matters: rotate
that credential once at its service, then replace every copy.

Grouping needs to know which values are equal, and ADR 077 keeps no value.

## Decision

- Each hit carries a fingerprint: the value hashed with a key drawn at random when
  the process starts (`RandomState`). Equal values compare equal within one run.
  The key is never stored or printed, so a fingerprint means nothing outside the
  process: it cannot be compared across runs, against another scanner, or against
  a guessed value.
- Fingerprints are never serialised: not in the snapshot, the email, the
  attachment or tool output. Only the grouping derived from them is shown.
- The weekly email lists credentials one row per value: the names it was assigned
  to, every place it is written, and whether any copy is live. Live first, then the
  most copied. The `audit_dependencies` table numbers rows; rows sharing a number
  hold the same value.
- A credential line that disappears is not reported as fixed. The email says it is
  no longer in the file and asks for rotation, because git history keeps it.

## Consequences

- One rotation task per credential instead of per line; owners of the copies can
  see they share one.
- Rotation in place (a new literal under the same name) cannot be recognised across
  weeks, by design: that would need a fingerprint that survives the run. The item
  stays open until the literal is replaced by a CI variable reference, which is the
  intended end state anyway.
- Two different credentials hashing to the same 64-bit value would be merged; the
  chance is negligible at the number of credentials a group holds.
