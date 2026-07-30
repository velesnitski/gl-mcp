# ADR 048: DAG tracing, wall-clock timing, and a state-drift class

## Status

Accepted (2026-07-30)

## Context

Three gaps surfaced while using `analyze_pipeline_failures` (ADR 046/047) against a
real provisioning fleet.

**1. Child pipelines were analyzed blind.** In a multi-project setup, a provisioning
run spans several projects via bridge jobs (`strategy: depend`). In one live sample
**half of all runs** had `source=pipeline` — they *were* children — yet each was
reported in isolation. A failure two hops down surfaced only as a red parent with no
explanation, and the tool offered no way to follow the chain.

**2. `duration` is the wrong clock.** GitLab's `duration` counts job execution only.
One observed run was created at `10:28` and finished at `12:06` — **1h37m** elapsed
for **27s** of execution. Reporting 27s answers "how much CPU did this consume", not
"how long until the customer's resource existed". Worse, bridge/child pipelines
report a **null** duration, so aggregate timing had no sample at all.

**3. State drift was labelled as configuration.** Two failure shapes dominated:
create → `400 already exists`, and delete → `404 not found`. Both were bucketed as
`config`. They share the "do not retry" verdict but nothing else: configuration is
*edited*, whereas drift is *reconciled* (import the object, drop it from state, fix
delete ordering). Labelling drift as config sends people to the wrong fix — and in
the observed case the two shapes were halves of one loop, where a partial destroy
leaves an orphan that the next apply then collides with.

## Decision

**Downstream DAG resolution in `get_pipeline`.** List the pipeline's bridge jobs and
show each downstream pipeline's id, status and URL. A bridge with *no* downstream is
called out explicitly — that means the trigger itself failed, which is otherwise
invisible. Uses `trigger_jobs` (which superseded `bridges` in GitLab 19.2) and falls
back to `bridges`, so it works across instance versions.

**Wall clock and queue time.** `get_pipeline` reports created→finished alongside job
time, plus `queued_duration` when non-trivial, and flags runs that spent an order of
magnitude longer waiting than executing — the signature of runner starvation.
Aggregate timing now uses wall clock, which every pipeline has.

**A `state` class**, separate from `config`, classified against the **whole log**
rather than the cluster signature: the decisive evidence ("already exists",
`status-code=404`) sits below the `Error:` header that names the cluster. Its retry
guidance names the reconciliation actions rather than telling people to edit config.

## Consequences

- A failure can be followed across the projects it actually spans.
- Timing answers the question operators and customers care about, and is no longer
  absent for bridge pipelines.
- The two "don't retry" classes are distinguishable, so the advice matches the fix.
- General point, third time in this series: **an aggregate is only as honest as its
  population.** Counting child pipelines as independent runs, or job time as elapsed
  time, produces numbers that are individually defensible and collectively
  misleading.
