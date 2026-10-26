# Dependency vulnerability audit — group `acme`

**5 repo(s) scanned** of 7 · **3 lockfile(s)** · **180 distinct packages** · **3 vulnerable** (2 runtime, 0 build tooling, 1 dev-only) · 4 distinct advisories
Vulnerable packages by worst advisory: CRITICAL 1 · HIGH 1 · MEDIUM 1

| Severity | Package | Advisories | Fix | Where |
|----------|---------|------------|-----|-------|
| CRITICAL | minimist@0.0.8 (npm) · dev | 1 — CVE-2021-44906: issue GHSA-xvch-5gv4-984h | → 0.2.4 (major upgrade) | acme/web:package-lock.json |
| HIGH | lodash@4.17.15 (npm) | 2 — CVE-2020-8203, CVE-2021-23337: issue GHSA-35jh-r3h4-6jhm | → 4.17.21 | acme/web:package-lock.json +1 more |
| MEDIUM | semver@5.7.1 (npm) | 1 — CVE-2022-25883: issue GHSA-c2qf-rxjj-qqgw | → 5.7.2 | acme/web:package-lock.json |

## Infrastructure: end of life

| Severity | Where | What | Status | Fix |
|----------|-------|------|--------|-----|
| HIGH | acme/web:Dockerfile +1 more | node:16-alpine (nodejs 16) | 16 reached end of life on 2023-09-11: no security fixes since | → 22 (supported until 2027-04-30) |
| HIGH | GitLab instance (git.example.com) | GitLab 17.5.2 (gitlab 17.5.2) | behind the latest 17.5 patch release (17.5.5); patch releases carry security fixes | → 17.5.5 |
| LOW | acme/web:.gitlab-ci.yml | nginx:latest (nginx latest) | unpinned: each build pulls whatever is newest, so what runs is unknown | pin a supported version |

## Coverage
- **No supported lockfile** (1): acme/docs — dependencies there are not pinned, so they cannot be audited.
- **Not auditable against OSV** (1): acme/ios:Podfile.lock — no advisory feed for that ecosystem.
- **Direct dependencies only** (1): acme/android:gradle/libs.versions.toml — a version catalog declares direct dependencies; transitive ones need a `gradle.lockfile`.
- 3 internal or non-registry dependencies (GitLab-hosted, git or path sources) were kept out of the query and not audited.
- 2 dependencies are pinned to a branch, commit or range rather than a release, so they could not be matched.
- 2 of 7 repos were not scanned: beyond `max_projects` (the most recently active come first) or unreadable — see any warning above.

_Matched against OSV.dev (GitHub, RustSec, Go, PyPA and other advisory feeds). Only exact pinned versions are checked; advisories for the same issue are merged; severity is the advisory's own rating, else its CVSS v3 base score. Go is read from `go.mod` (the selected build list), not `go.sum`. Images, Terraform-declared platforms and the GitLab instance are checked against endoflife.date support dates. npm roles come from reachability: a package reached only through build tools (webpack, babel, vite…) is build tooling even when declared under `dependencies`._