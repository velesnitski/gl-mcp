# ADR 080: Unmasked CI/CD variables, judged by value

**Status:** Accepted
**Date:** 2026-11-12

## Context

`audit_ci_security` reported a CI/CD variable as an unmasked secret when its name
contained a word such as KEY, PRIVATE or PASS. Names alone mislead both ways:
`STORAGE_PRIVATE_BUCKET` and `DEPLOY_PUB_KEY` are not secrets, a variable holding
`$OTHER_VAR` or `true` holds nothing, and a single-line value under a `PRIVATE_KEY`
name can be masked while a multi-line one cannot. The CI-literal email (ADR 079)
listed credentials typed into CI files but not these variables, which its readers
also act on.

## Decision

- The variable check reads the value the API returns, decides what it is, and keeps
  only its length and a per-run fingerprint (ADR 078). Nothing about the value is
  printed or stored.
  - Empty values, references to other variables, booleans, placeholders and paths
    are not secrets, whatever the name.
  - Multi-line or PEM-armoured values are key material: HIGH, fixed by file type.
  - JSON values (`DOCKER_AUTH_CONFIG`) cannot be masked: MEDIUM, fixed by
    protection, scoping and a robot credential.
  - Other values are single-line secrets: MEDIUM, fixed by masking.
  - Without a readable value the name decides, as before.
- A value holding only public SSH keys or certificates is not a secret; neither is
  a reference to a secret store (GCP `name:latest`, `projects/…/secrets/…`, AWS
  Secrets Manager or SSM ARNs, Vault paths, `op://`), nor an address: an email, a
  hostname or IP, or a URL without user info, query or a token-length segment. A
  webhook's long path segment is its secret, so webhooks still count. These rules
  apply to CI-file literals too.
- For variables the last word of the name decides: one ending in USER, LOGIN,
  EMAIL, NAME, TYPE, TTL, PATH, FILE, VERSION, MODE and similar describes a secret
  rather than holding one, so `DB_USER_PASSWORD` still counts.
- Names that locate a resource (`_BUCKET`, `_ENDPOINT`, `_REGION`, `_HOST`,
  `_PORT`, `_DIR`) and public keys (`PUB_KEY`) are not secrets.
- A secret-named variable skipped because of its value or name is listed with the
  reason, never the value, so a skip can be verified by whoever owns the variable.
- Whether a variable is protected is stated. Masking keeps a value out of logs;
  protection keeps it from pipelines on unprotected branches.
- The weekly scan reads each project's variables and adds them to the CI-literal
  email, grouped by value, and to its snapshot so new ones are tracked. A project
  whose variables the token cannot read is listed as not checked, and its earlier
  variable findings carry over rather than reading as fixed.

## Consequences

- One request more per project in the weekly scan.
- Fewer false positives, and a severity that matches what fixing it takes.
- Key material in an unmasked variable counts as a new live finding and so trips
  the mid-week CI email; ordinary unmasked secrets do not.
