# ADR 079: CI credentials as a separate email

**Status:** Accepted
**Date:** 2026-11-11

## Context

ADR 077 put credentials written into CI files into the weekly dependency email,
and ADR 078 grouped them by value. The dependency email goes to a wide list: team
leads and stakeholders who act on package upgrades. A list of where live
credentials sit is a different kind of information. It names exactly which files
to read to obtain a working password, and the people who act on it (rotating at
the service, moving the value into a CI variable) are few. Sending it to everyone
who receives dependency findings widens the exposure it reports.

## Decision

- `--dependency-audit` produces two emails from one scan. The dependency email
  carries no CI literals at all: no section, no coverage line, no items in its
  snapshot or subject.
- The CI-literal email is written with `--ci-out`. It has its own snapshot
  (`--ci-previous`, `--ci-snapshot-out`) so "new" and "no longer in the file" are
  measured since the last CI email, independent of whether the dependency email
  was sent.
- Its subject follows the dependency subject's contract: `CI credentials —
  <scope>: baseline | N new live | nothing new live · N live[ · N removed]`. A
  mid-week job reads it to send only when a new live credential appears.
- Recipients are the deployment's choice. The email states that distribution is
  restricted and should not be forwarded.
- A dependency snapshot written before the split still holds CI literals. They are
  dropped when it is read, so the first split run does not report them as fixed.
  The first CI email has no previous CI snapshot and is a baseline.

## Consequences

- The dependency email's subject and new/fixed counts no longer move because of a
  CI literal; its mid-week alert fires on dependency findings only.
- A deployment that does not pass `--ci-out` gets no CI-literal report from this
  command. `audit_ci_security` and `audit_dependencies` still show them on demand.
