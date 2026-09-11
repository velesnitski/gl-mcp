# ADR 055: A sweep that states its window, and an audit that names no secret

## Status

Accepted (2026-09-11)

## Context

Group code search capped at the 60 most recently active repos and printed a warning —
but offered no way to reach the rest. There was no offset, no cursor, no opt-in full
pass; the only recourse was narrowing to a subgroup by hand. Used in anger during an
incident, it covered well under half a large group, and the answer had to be handed
over with a caveat attached. A sweep whose coverage cannot be completed is the worst
shape this server takes: **"no matches" over an unstated subset reads as proof of
absence, which is the exact claim a sweep is run to establish.**

Separately, several distinct incidents turned out to share one root: configuration that
would not stop a secret from being written to a job log, or would let a build change
underneath its own pipeline. Each was found by hand, none by a tool, and each was
visible in configuration well before it became an outage or an exposure.

Building the audit on top of a silently-partial search would have been worse than not
building it: an "all clear" covering an unstated fraction of a group is a false
assurance with a security label on it.

## Decision

**The sweep states its window and prints the way forward.** `offset` continues a
partial sweep, `full_sweep` covers everything in one call against a bounded ceiling,
and when repos remain the response gives the literal next invocation rather than
leaving the caller to infer one exists. An offset past the end is reported as such
instead of returning an empty, successful-looking result.

**`audit_ci_security` checks configuration, not contents.** Four detectors, each
derived from a failure that actually occurred rather than from a checklist: secret-
shaped variables that are neither masked nor file-type; debug tracing left enabled;
floating image tags; and unpinned remote artifacts executed inside CI.

Two constraints on it are not negotiable.

**It reports names and locations, never values.** A scanner that prints the
credentials it finds into a transcript has reproduced the bug it hunts (ADR 052).
Variable values are not read at all, except the boolean it must compare for debug
tracing.

**Its advice must be executable.** Key material cannot be masked — the platform
rejects multi-line values — so recommending masking for it produces a failed API call
and a reader who concludes the finding was noise. Those keys are detected separately
and told to become file-type variables. Wrong-but-plausible advice is the failure mode
ADR 054 was written about, and a security tool is where it costs most.

The audit states its own blind spot in every report: it can tell that nothing would
prevent an exposure, never that one occurred.

## Consequences

- A group sweep can be completed, and a partial one cannot be mistaken for a whole.
- Configuration-level exposure is found by a tool instead of by incident.
- Method note, tenth in the series: this pair is the two halves of the same lesson.
  One makes the tool say what it did not look at; the other makes it say what it
  cannot know. **Honest scope and honest advice are the same property.**
