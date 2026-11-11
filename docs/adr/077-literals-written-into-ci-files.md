# ADR 077: Secrets written into CI files

## Status

Accepted (2026-11-11)

## Context

`audit_ci_security` checked CI *variables*: secret-shaped ones GitLab would not mask,
debug tracing left on. A credential typed straight into `.gitlab-ci.yml` (a
`variables:` entry with a literal password, a token in a `curl -H` header, a
`user:password@` clone URL) is not a CI variable, so that audit reported such a
project clean. It is the more serious exposure of the two: everyone who can read
the repository has the credential, so does every clone and fork, and deleting the
line leaves it in git history.

The weekly dependency email did not cover it either.

## Decision

1. **A detector for literal credentials in CI files** (`tools/ci_exposure.rs`), pure:
   file text in, hits out. It reads:
   - assignments whose key names a secret (`PASSWORD`, `TOKEN`, `SECRET`, `API_KEY`,
     `CREDENTIAL`, `WEBHOOK`, `DSN`, …) — YAML `KEY: value` and shell `KEY=value`;
   - credentials in HTTP headers (`PRIVATE-TOKEN:`, `Authorization: Bearer …`);
   - `--password` / `--token`-style flags;
   - passwords embedded in URLs;
   - credential formats recognisable on their own (GitLab, GitHub, Slack, AWS,
     Google, Stripe keys, private key blocks), whatever they are assigned to.
2. **Precision before recall.** Not reported: run-time references (`$NAME`,
   `${NAME}`, `$(cmd)`, `Bearer $TOKEN`), placeholders (`***`, `<token>`,
   `changeme`, `your_…`), templates, booleans, file paths, and keys that name
   *about* a secret rather than hold one (`…_USERNAME`, `…_FILE`, `…_TTL`). Bare
   `KEY` is not a secret name: GitLab's own `cache: key:` would match it in nearly
   every pipeline. A `$` that does not start a reference is part of a literal.
3. **Commented-out credentials count**, one level lower: no longer used, still in
   the file and in history. Active: HIGH; commented: MEDIUM.
4. **No value leaves the detector.** A hit carries the file, line, key, how the
   value was written, whether the line is commented, and the value's length.
5. **Where it runs:**
   - `audit_ci_security`: the root CI file and the local files it includes (up to
     10), as finding `CI-SECRET`.
   - `audit_dependencies` and the weekly email: CI definition files found in the
     repository (the root file and `.gitlab/`, `ci/` YAML, up to 10 per repository).
     Each hit becomes an item keyed by repository, file and key (not line), so it
     is tracked as new / fixed / open like a package. A live one is urgent, so it
     trips the mid-week alert.
6. **The remedy names rotation first.** Moving the value into a masked variable
   without rotating it leaves the old one valid and in history.

## Consequences

- A credential under a key name outside the list, in an unrecognised format, is
  missed; widening the list trades precision.
- Included files from other projects (`include: project:`) are not followed.
- Commenting a secret out reads as one fixed item and one new, lower one; rotating
  and removing it reads as fixed.
