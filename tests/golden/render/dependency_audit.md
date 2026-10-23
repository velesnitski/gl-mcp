# Dependency vulnerability audit — group `acme`

**4 repo(s) scanned** of 6 · **2 lockfile(s)** · **180 distinct packages** · **2 vulnerable** · 2 advisories
CRITICAL 1 · HIGH 1

| Severity | Package | Advisory | Fixed in | Where |
|----------|---------|----------|----------|-------|
| CRITICAL | minimist@0.0.8 (npm) | GHSA-xvch-5gv4-984h (CVE-2020-8203) — Prototype pollution in merge | 0.2.4, 1.2.6 | acme/web:package-lock.json |
| HIGH | lodash@4.17.15 (npm) | GHSA-p6mc-m468-83gw (CVE-2020-8203) — Prototype pollution in merge | 4.17.19 | acme/web:package-lock.json +1 more |

## Coverage
- **No supported lockfile** (1): acme/docs — dependencies there are not pinned, so they cannot be audited.
- **Not auditable against OSV** (1): acme/ios:Podfile.lock — no advisory feed for that ecosystem.
- 3 internal or non-registry dependencies (GitLab-hosted, git or path sources) were kept out of the query and not audited.
- Only the 4 most recently active of 6 repos were scanned; raise `max_projects` to cover more.

_Matched against OSV.dev (GitHub, RustSec, Go, PyPA and other advisory feeds). Only exact pinned versions are checked; severity is the advisory's own rating, else its CVSS v3 base score._