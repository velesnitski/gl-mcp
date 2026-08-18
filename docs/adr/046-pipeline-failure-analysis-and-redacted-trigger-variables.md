# ADR 046: Pipeline failure analysis + redacted trigger variables

## Status

Accepted (2026-07-30)

## Context

Where infrastructure is provisioned by API-triggered GitLab pipelines, each red
pipeline is not a developer annoyance — it is a **customer's resource that failed to
provision**. Two things made that hard to see with the existing tools.

**1. Failure counts conflate two unrelated populations.** A shared CI-template repo
can show *every* recent pipeline failed while production provisioning is completely
healthy, because those failures are all `merge_request_event`/`push` runs — people
iterating on the template. Counting them together produces a permanently-red
dashboard that teams learn to ignore, and hides the failures that matter.

**2. A failed pipeline could not be tied to what it was acting on.** Trigger
variables (org/network identifiers passed by the calling system) were not exposed at
all, so "pipeline N failed" could not become "customer X's network failed".

There was also no way to answer "is this worth retrying?" without opening each job
log by hand.

## Decision

**`analyze_pipeline_failures(project|group, days, max_logs)`** — reports health and
clusters failures:

- **Splits by pipeline source.** `trigger|api|schedule|pipeline|web|external` are
  automated/operator runs (the production signal); `push|merge_request_event` are
  development CI, reported separately and excluded from the success rate. The full
  source mix is printed so the split is auditable rather than asserted.
- **Clusters failures by root cause.** For recent automated failures it fetches the
  failed job's log, extracts the first *specific* error line — deliberately
  preferring it over the generic `ERROR: Job failed: exit code N` trailer every
  failed job ends with — and normalizes volatile parts (UUIDs, hashes, numbers) so
  repeats of one fault group together.
- **Triages each cluster** as `transient` (runner/network/rate-limit → retrying is
  likely to work), `config` (missing/invalid configuration → the same run will fail
  identically), or `unknown`. Classification is conservative: anything unrecognized
  is **not** advertised as retryable, and the output warns against blind retries of
  plans containing destroy operations, where a partial apply can worsen state.

**Trigger variables in `get_pipeline`, default-deny redacted.** Values are rendered
only when the key is not secret-ish, *and* is identifier-shaped, *and* the value is
short and plain; everything else prints `<redacted>`. Key names are always listed —
useful for debugging, low risk. The secret-ish check runs first, so `..._CLIENT_ID`
is denied despite ending in `_ID`. The endpoint needs elevated scope; a 403 simply
omits the section.

## Consequences

- Provisioning health becomes measurable (success rate, median duration, MTTR-shaped
  data) without dev-CI noise.
- Failure triage moves from "open every log" to a ranked cluster table with retry
  guidance.
- The infra→business link exists, without turning a debugging tool into a credential
  reader. Redaction is default-deny on purpose: an allowlist that is too strict only
  costs a little convenience, whereas a denylist that misses one key leaks a secret.
- Tests pin the security behaviour (secret-ish keys never render values) and the
  triage behaviour. One of them caught a real defect during development: the numeric
  mask was word-boundary anchored, so values glued to units (`1234ms`) were never
  normalized and identical faults would not have clustered.
