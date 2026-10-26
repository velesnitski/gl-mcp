# ADR 072: Infrastructure end of life, including the GitLab instance

## Status

Accepted (2026-10-26)

## Context

The dependency audit (ADR 066–070) reads lockfiles. Infrastructure repositories
(Terraform, Helm, Kubernetes manifests, Dockerfiles, CI configuration) have none, so
the report listed them as "not covered". In a platform-heavy group, that can be most
of its repositories.

Infrastructure risk has a different shape from package CVEs. It is a runtime, OS,
database, Kubernetes or GitLab release that no longer receives security fixes. That
can be decided from the declared version alone, with no CVE matching, using a
product's published support dates. The platform the audit runs against is part of
the same surface. A self-managed GitLab that is behind on patch releases is missing
security fixes, because GitLab ships them in its patch releases.

## Decision

1. **Data source: endoflife.date.** It publishes per-cycle support dates for several
   hundred products, including base images, databases, Kubernetes and GitLab. Only
   public product names are sent (e.g. `nodejs`, `postgresql`). Repository content
   is never sent. `ENDOFLIFE_API_URL` points at a mirror.
2. **What is read** (at most 40 files per repository):
   - `FROM` in Dockerfiles, with `ARG` defaults resolved and stage aliases skipped;
   - `image:` in compose, `.gitlab-ci.yml` and Kubernetes YAML;
   - Helm `repository` + `tag` pairs;
   - in Terraform: Kubernetes versions, managed database engines (RDS
     `engine`/`engine_version`, Cloud SQL `database_version`);
   - `.terraform.lock.hcl` providers, as the Go modules their advisories are
     published under (OSV, the normal package path).
3. **Images.**
   - An image carries one or two products: itself and an embedded distro
     (`-alpine3.18`, `-bookworm`).
   - A version maps to the longest cycle prefix ("16.20.2" → "16"), or to a
     Debian/Ubuntu codename.
   - Private-registry images are counted and not checked: their bases are covered
     through their own Dockerfiles.
   - Templated tags (`${TAG}`) and products the data does not know are never
     guessed.
4. **Verdicts:**
   - past end of life → HIGH;
   - end of life within 90 days → MEDIUM;
   - unpinned (`latest`, no tag) → LOW.
   - The fix named is the newest supported cycle.
5. **The platform checks itself.**
   - The instance version (`/version`) is checked against the `gitlab` cycles. For
     GitLab only, a supported minor that is behind its latest patch is HIGH ("N.M
     patch releases carry security fixes").
   - Runner versions are checked against the same cycles when the token may list
     every runner (admin). Otherwise this check is skipped silently.
6. **One report.** End-of-life findings are items in the weekly email, keyed by
   repository, product and version. Upgrading a base image therefore reads as
   "fixed", a newly declared end-of-life version as "new", and open ones age, exactly
   like packages. endoflife.date being unreachable is a warning, not a failed audit.
7. **Several groups in one report.** `group_path` accepts a comma-separated list, so
   an instance whose code lives in several top-level groups gets a single email.

## Consequences

- **This is end of life, not image CVEs.** OS packages inside an image need an image
  scanner run against the built image. The email states this limit.
- **A version declared through a variable or an external values file is not seen.**
  These show up as fewer findings, never as false ones.
- **Each infrastructure file read is one more request per repository**, capped at 40.

## Addendum (2026-10-26): grouping, LTS targets, empty repositories

Three refinements:

- **Runners are grouped by release line.** A fleet of runners on one line produced
  one identical row per runner. Now there is one finding per line, naming the
  runners on it.
- **The fix names the newest supported LTS line** when the product marks LTS lines.
  The newest supported line can be a short-lived "current" or innovation release,
  which is a poor upgrade target.
- **An empty project (no repository tree) is skipped,** not counted as a failed
  scan. The coverage note no longer blames `max_projects` for repositories that were
  not scanned for other reasons.
