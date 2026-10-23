# ADR 067: Dependency audit findings per package

## Status

Accepted (2026-10-23)

## Context

Using `audit_dependencies` (ADR 066) showed that the report was
correct but hard to act on:

- **Go repositories were dominated by stale findings.** `go.sum` keeps a checksum for
  every module version seen anywhere in the module graph, including versions that
  minimal version selection discarded. Most of those findings were for code the
  build never links.
- **One issue appeared several times.** OSV publishes the same vulnerability under
  several ids: a GHSA record alongside a GO- or RUSTSEC- record, linked by aliases.
  Each was its own row.
- **Rows were per advisory, not per decision.** A package with six advisories took
  six rows, and none of them said which version fixes all six.
- **Runtime and development dependencies were indistinguishable.** A test-only
  package ranked level with the HTTP stack.
- **Mobile repositories were blind spots.** Swift Package Manager pins and Gradle
  version catalogs were not read.

## Decision

1. **`go.mod` over `go.sum`.** When both sit in the same directory, `go.mod` is read
   and `go.sum` is skipped. `go.mod` lists the selected version of every module in
   the build list (Go 1.17+ records indirect requirements too). `replace` directives
   apply: a versioned replacement is audited in place of the original, and a local
   path is internal. A `go.sum` without a `go.mod` beside it is still read.
2. **Advisories of one issue merge.** Advisories against one package that share an
   id or alias are merged into one. The merged advisory keeps the highest rating,
   the union of aliases and fixed versions, and prefers a rated GHSA record as the
   one it shows.
3. **One row per vulnerable package version.** Each row shows the worst severity,
   the advisory count with CVE ids, and the upgrade target. The target is the lowest
   version that fixes every advisory: for each advisory, the smallest fixed version
   above the current one, then the highest of those. The row flags a major upgrade
   (for 0.x, a minor one). It says so when some advisory has no fix yet.
4. **Runtime before dev.** The dev flag is taken from the lockfiles that record it:
   npm `dev`/`devOptional`, Composer `packages-dev`, Poetry `category = "dev"` and
   Pipfile `develop`. A package counts as runtime if any lockfile pins it outside
   dev. Within a severity, runtime packages come first. Dev-only rows are marked,
   not hidden.
5. **Swift and Gradle catalogs.**
   - `Package.resolved` (v1–v3) is matched under OSV's `SwiftURL` ecosystem. Branch
     or revision pins are counted as unversioned. Non-remote and GitLab-hosted pins
     are internal.
   - `libs.versions.toml` is read with `version.ref` resolved. A catalog declares
     direct dependencies only, and the coverage section says so. Ranges and
     BOM-managed entries are counted as unversioned.
6. **`min_severity` and `summary_only`.**
   - `min_severity` filters the table but never the totals. An unknown value is a
     caller error, not "all".
   - `summary_only` returns one line: counts, runtime CRITICAL/HIGH, and the five
     packages to fix first.

## Consequences

- Go findings now reflect what is built. A vulnerable module that appears only in
  `go.sum` is no longer reported. That is the intended trade: a version the build
  does not select is not running.
- The upgrade target compares versions numerically, ecosystem-agnostic. That is
  right for the semver-like versions every supported ecosystem uses. It is not
  Maven's or PEP 440's full ordering, so the target is advice to verify, not a
  resolver result.
- Writing the tests found two defects before release: the merge dropped the id of a
  record that stopped being shown, and equal versions written with a different
  number of components ("1.0" and "1.0.0") compared as unequal.
