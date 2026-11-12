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

## Addendum (2026-11-11): precision from a first wide sweep

A first run across many repositories showed four false-positive shapes,
now handled:

- an escaped reference inside an `echo` (`KEY=\"${VAR}\"`) was cut at the
  backslash — escaped quotes are now part of the value;
- `key: value` in prose (an `echo` message, a comment sentence) — the YAML form
  must now be the whole line; the shell `KEY=value` form still counts anywhere;
- variables that hold a path to a key file (`.config/key.json`) — file-like values
  are paths;
- values shorter than three characters are not credentials.

Initial passwords of database service containers (`MYSQL_*`, `POSTGRES_*`,
`MARIADB_*`, `MONGO_INITDB_*`) are reported one level lower: they usually belong
to a throwaway test database, and the advice is not to reuse them.

## Addendum (2026-11-11): access key IDs are identifiers

An access key ID (`S3_ACCESS_KEY`, `AWS_ACCESS_KEY_ID`, an `AKIA…` value), a
public or publishable key and a client ID name a key; on their own they sign
nothing. Written into a CI file they are now reported as MEDIUM ("keep it next to
its secret in CI variables", so the pair rotates together) and do not count as
live credentials. An unmasked CI variable with such a name is no longer reported.
A name containing SECRET or PRIVATE is never treated as an identifier.

A literal password equal to a database service container's password in the same
file (`DB_PASSWORD` set to the value of `MYSQL_ROOT_PASSWORD`) is the job's own
connection to that throwaway database and is reported as one too: MEDIUM.
