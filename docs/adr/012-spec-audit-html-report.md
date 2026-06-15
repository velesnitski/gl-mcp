# ADR 012: HTML spec-audit report

## Status

Accepted (2026-06-15)

## Context

`audit_spec_drift` returns markdown. Every other analytics surface in gl-mcp has
a shareable, clickable HTML twin (the AI-adoption report), and a spec audit —
version drift, cleanup debt, undocumented endpoints, leaked secrets — is exactly
the kind of artifact that gets pasted into a review or a townhall deck. It should
look like the others.

## Decision

Extract the audit computation into `compute_audit` returning an `AuditOutcome`
struct, so the markdown path (`audit_spec_drift`) and a new HTML path
(`generate_spec_audit_report`) share one scan — no duplicated heuristics, no
double API traffic per renderer.

`render_html` produces the same dark-theme house style as the adoption report
(shared `PRINT_CSS` / `EXPORT_BUTTON` / `htmlescape`, Export-PDF button,
auto-expanding `<details>` on anchor navigation):

- Summary cards (version verdict, cleanup-debt, drift, undocumented, secrets)
  that link to their section anchors.
- A version banner coloured by verdict (risk when stale).
- "Changes since last audit" when a prior snapshot exists.
- Route-drift sections, an undocumented-endpoints table with GitLab file links,
  and a security section showing **masked** secret previews only.

## Consequences

- Both tools call `compute_audit`, which persists the metadata-map snapshot. If a
  user runs the markdown tool and then the HTML tool, the second sees the first's
  snapshot, so its "changes since last audit" shows nothing. Acceptable: each
  call is itself an audit, and the diff is "since the previous run."
- The dark-theme CSS is duplicated as a module const rather than shared with the
  adoption report, to avoid refactoring working code; the theme is stable enough
  that drift between the two copies is low-risk.
- Security previews in the HTML are masked exactly as in markdown — a report file
  saved to disk or shared never contains a raw secret.
