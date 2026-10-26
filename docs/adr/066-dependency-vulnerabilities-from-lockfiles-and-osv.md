# ADR 066: Dependency vulnerabilities from lockfiles and OSV

## Status

Accepted (2026-10-23)

## Context

We wanted known-vulnerability checks across our GitLab. Two options came up:

- **Reuse an existing CVE MCP server** (roadwy/cve-search_mcp). It wraps CIRCL's
  cve-search API and looks CVEs up by vendor and product name. That answers "which CVEs
  exist for product X". Auditing repositories asks something else: "which of the
  dependency versions we pin are affected". Answering that takes ecosystem package
  names (crates, npm, Composer, Go) and version-range matching. CPE vendor/product
  names don't map onto package names, so lookups by them are noisy and miss findings.
  The server was also a single-file Python wrapper, unmaintained since mid-2025, and
  its upstream endpoints had begun to redirect.
- **GitLab's built-in dependency scanning.** It needs an Ultimate licence and a CI job
  in every repository. It would be the right tool where both exist; neither can be
  assumed here.

## Decision

`audit_dependencies` in gl-mcp audits one project or a group:

1. **Lockfiles are read over the API.** The repository tree is listed and lockfiles are
   picked by name: `Cargo.lock`, npm, yarn and pnpm lockfiles, `composer.lock`,
   `go.sum`, `poetry.lock`, `Pipfile.lock`, `requirements*.txt`, `Gemfile.lock`,
   `gradle.lockfile`. Vendored trees are ignored, and at most 12 lockfiles are read per
   repository; the report names capped repositories.
2. **Only exact pinned versions are checked.** A version range is not a version, so
   unpinned requirements are not guessed at.
3. **Matching uses OSV.dev.** It aggregates GitHub, RustSec, Go, PyPA and other feeds
   with exact affected ranges and fixed versions. Each distinct package and version is
   queried once in a batch, and each advisory is fetched once.
4. **Severity** is the advisory's own rating where it has one. Otherwise it is the
   CVSS v3 base score, computed per the FIRST specification from the vector.
5. **Internal packages never leave the network.** Dependencies resolved from the
   GitLab host itself, a git URL or a local path are not sent to OSV. The report counts
   them as unaudited.
6. **The report states what it could not audit:** repositories without a supported
   lockfile, lockfiles with no advisory feed (CocoaPods), capped monorepos, skipped
   internal packages, and repositories outside `max_projects`. When nothing at all was
   auditable, it says so rather than "no vulnerabilities". An unreachable OSV is an
   error, through the same outage handling as ADR 060.
7. **`get_vulnerability`** looks up a single advisory by id. It covers the useful part
   of the CVE server above without running a second server. `OSV_API_URL` points at a
   mirror for air-gapped use.

## Consequences

- One request per lockfile, plus a tree listing per repository. OSV calls scale with
  distinct packages, not with repositories.
- Package names and versions of public dependencies are sent to OSV.dev. That is
  public information, and internal ones are excluded as described above.
- Parsers handle untrusted file content. They are pure, unit-tested per format against
  synthetic fixtures, and property-tested to never panic on arbitrary input. Writing
  those tests found two parser defects before release: a yarn entry was decided before
  its `resolved` line was read, and blank lines ended pnpm's `packages:` section.

## Addendum (2026-10-26): Composer branch aliases are not versions

Composer pins a package tracked from a branch with a version like `dev-main`, or as
a branch alias such as `12.x-dev`. Only the first form was treated as unversioned.
`12.x-dev` went to OSV, which matched it against version ranges and reported
CRITICAL advisories for a branch name. Any version ending in `-dev` is now counted
as unversioned, as point 2 always intended: a branch is not a version.
