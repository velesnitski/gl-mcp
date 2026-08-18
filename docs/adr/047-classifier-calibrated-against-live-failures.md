# ADR 047: Calibrate the failure classifier against real failures

## Status

Accepted (2026-07-30)

## Context

`analyze_pipeline_failures` (ADR 046) was built from first principles and its unit
tests used synthetic log lines. The first run against a live infrastructure group
showed the classifier was under-calibrated in three concrete ways:

1. **Most clusters fell to `unknown`.** Two were unambiguous misses: the config
   list matched `"missing required"` but not a bare `"Missing …"`, and `"undefined"`
   but not `"undeclared"` — so real Terraform/provider configuration errors were not
   recognized as config.
2. **One cluster rendered as `error: …`.** The volatile-value masker had eaten the
   entire message because it was almost all digits, destroying the information the
   cluster exists to convey.
3. **Median duration printed `0s`.** Bridge/child pipelines report a null duration, so
   the sample was empty and the code substituted a default that reads as a real
   measurement.

## Decision

- **Broaden the config markers** to the forms that actually occur (`missing`,
  `undeclared`, `not set`, `required`, `unsupported`), while leaving the transient
  list untouched.
- **Guard the masker**: if the masked form retains almost no alphabetic content, keep
  the original text. A slightly over-specific cluster is far better than an empty one.
- **Report `n/a` when no duration sample exists** instead of `0s`.

Deliberately *not* changed: the recurring secret-store folder errors remain
`unknown`. Whether they are transient API races or genuine state conflicts is not
determinable from the log line, and the tool's contract is to refuse to guess rather
than advertise something as retryable on a hunch.

## Consequences

- Config-class failures are now recognized, so retry guidance covers the cases where
  retrying is provably futile.
- Cluster labels stay informative on numeric-heavy messages.
- The report no longer states a duration it does not have.
- Method note: **a classifier calibrated only on invented examples will be wrong in
  the ways reality is weird.** All three defects were invisible to a green unit-test
  suite and obvious within one live run; the fixes are pinned by tests built from the
  real signatures.
