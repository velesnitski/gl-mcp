# ADR 050: Redact job logs, and time pipelines the list endpoint can actually report

## Status

Accepted (2026-08-19)

## Context

Three defects, all found by running the tools against a live fleet rather than by
reading them — the same way the ADR 047 defects surfaced.

**1. Redaction protected an endpoint, not a value.** ADR 046 established that trigger
variables are rendered default-deny, and they are. But a CI job running under `set -x`
echoes its whole payload into the job trace, and the trace is served by a different
endpoint that never saw that rule. The consequence is structural: the same variable
that `get_pipeline` shows as `<redacted>` can be read in full from the log of a job on
that pipeline. Protecting the variables API while the value flows through the trace is
a rule that describes an endpoint instead of a value — and key and certificate bodies
are exactly what a payload echo puts there.

**2. Aggregate timing could never have a sample.** `GET /projects/:id/pipelines`
returns `created_at` and `updated_at` — and no `finished_at`, no `duration`. Every
aggregate is built from that listing, so wall clock was always `None`, `duration`
always `0.0`, and the median printed `n/a` on every run regardless of input. ADR 048
claimed wall clock "every pipeline has"; that is true of the pipeline *object* and
false of the *list* response, which is the only thing the aggregate sees. The
`updated_at` fallback had been considered and dismissed as mattering only to running
pipelines — exactly backwards.

**3. The status-code triage missed the commonest shape.** ADR 049 required a status
word beside the number to keep bare integers from being read as codes. The word list
omitted `error`, and allowed only four characters of separation — so
`curl: (22) The requested URL returned error: 422`, the single most common form in a
shell-based pipeline, fell through to `unknown` on the first live run after shipping.

## Decision

**`redact_log` on every trace fetch**, applied before the text is used for anything —
including root-cause extraction, since the signature becomes a visible cluster label.
Widest rule first: PEM blocks, secret-ish assignments, bare token shapes, then any
long base64 run, which catches key material whose field name gave nothing away
(`*_CRT`, `payload`, an array element). `CRT` and `PEM` join the secret-ish key list.

**Short values are deliberately left visible.** A credential is not six characters
long, and blanking short values erases the evidence for the commonest CI fault there
is: the variable was never set. An empty token that has been redacted looks exactly
like a present one, and an unset credential is diagnosable *only* while the empty
value stays visible.

**`pipeline_end_ts`** prefers `finished_at` and falls back to `updated_at` for
pipelines in a terminal state, where that field is the completion write. For a running
pipeline it returns nothing: `updated_at` is a heartbeat, an elapsed-so-far, and
feeding it to a duration median would widen the sample with a different quantity.

**`error` joins the status-code context words.**

## Consequences

- A secret echoed into a log is masked no matter which tool surfaces it.
- Medians report a real number instead of a permanent `n/a`.
- Shell pipelines — the majority of infrastructure CI — get their failures classified.
- Method note, fifth in this series: **the first two defects were both cases of a rule
  that was written about one code path and believed about the system.** "Variables are
  redacted" and "wall clock is always available" were each true where they were
  written and false where they were relied on. Live output disagreed with both; the
  unit tests could not, because they exercised the path the rule was written for.
