# ADR 009: Spec audit — security check, version-tag prefixes, local metadata map

## Status

Accepted (2026-06-15)

## Context

ADR 008 shipped the route + version drift MVP. A live run against a real
app-spec article validated it and surfaced three follow-ups:

1. The version check returned "could not compare" because the repo's latest tag
   was `release-4.9.10` — the parser only stripped a leading `v`, so a prefixed
   tag failed to parse. The correct answer (spec `4.9.5` is behind) was lost.
2. The spec is, in practice, a secret store — base64 keys, an encrypted bearer
   token, a device UUID, and a service-account email, all in an org-readable
   wiki. This is the "security check" pillar from the original three-pillar
   vision (metadata / versioning / security).
3. Re-running the audit re-derives everything from scratch; there's no sense of
   "what changed since last time."

## Decision

Three additions to `audit_spec_drift`, all reusing the existing scan infra.

**Version-tag prefixes.** `parse_semver` now extracts the first dotted-numeric
run anywhere in the string (regex `[0-9]+(?:\.[0-9]+)+`), so `release-4.9.10`,
`v4.9.5`, and `app-v2.3.0` all parse. Comparison stays numeric per-component
(so `4.9.10` > `4.9.5`, not lexical).

**Security check.** Extract secret-shaped strings from the spec — base64 blobs
(≥32 chars, so short obfuscated route segments don't match), UUIDs, and
credential emails — then code-search each to distinguish:
- *doc-only leak* — not in code: rotate the secret and restrict the doc.
- *hardcoded in code* — also a literal in the repo: rotate **and** remove from
  code (worse).

Critically, the report shows only a **masked** preview (`AAAA…0000= (44
chars)`), never the raw value, so a report pasted into a ticket or chat does not
re-leak the secret. The code cross-reference keeps only file:line, not the
matched snippet, for the same reason.

**Local metadata map.** After each run a compact snapshot (version verdict, per
route path+verdict, per secret masked+hardcoded) is persisted to
`~/.gl-mcp/spec_maps/{project}__{ref}.json`. The next run for the same
project+ref diffs against it and adds a "Changes since last audit" section:
routes that drifted or got fixed, the version verdict moving, secrets appearing
or resolved. This is the persist-and-diff half of the map idea; reverse-drift
(code endpoints absent from the spec) still needs a code-side inventory and is
deferred.

## Consequences

- The map file holds real internal route paths and lives in the user's data dir
  (like `teams.json`), never committed. Persistence is best-effort: a write
  failure logs a warning and does not fail the audit.
- Secret detection is shape-based and will have both false positives (any long
  base64 string) and false negatives (bespoke key formats). It's a tripwire, not
  a vault scanner — the masked previews and code locations let a human judge.
- The snapshot intentionally stores masked secret previews, not raw values, so
  the on-disk map is itself safe to keep around. Diffing on the masked form is
  sufficient to detect appear/resolve/hardcoded-status transitions.
- Persisting on every call means "since last audit" is relative to the previous
  invocation, which is the intended behaviour for tracking drift over time.

## Correction (2026-06-15)

A live run surfaced a false positive: the base64 character class includes `/`,
so a long slash-delimited route path (`/v2/network/info/leading/primary`, 32
chars) matched as a "secret." Fixed with a `looks_like_b64_secret` guard — a
base64 candidate counts as a secret only if it carries padding (`=`) or a `+`,
or has no `/` at all. Real keys/tokens (which have padding or `+`) are kept;
slash-delimited paths with neither are rejected.
