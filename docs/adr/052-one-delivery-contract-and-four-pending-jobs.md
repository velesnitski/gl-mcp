# ADR 052: One delivery contract for credentials, and four ways to be pending

## Status

Accepted (2026-08-25)

## Context

**1. A safe default that only one tool followed is not a default.** ADR 051 gave
`create_project_access_token` a delivery contract: name a CI/CD variable and get back
metadata, or ask explicitly to have the value revealed. Its neighbour
`create_deploy_token` went on printing the token into the response. Two tools that
mint a credential, sitting in the same file, disagreeing about whether a secret may be
written to a logged channel — and the unsafe one was the older, more-used path. A
convention that holds in the tool where it was introduced and nowhere else has not
been adopted; it has been demonstrated.

The contract was also enforced only by prose. Nothing stopped a later edit to the
response builder from interpolating the value back in, and no test could have caught
it, because the value was in scope throughout.

**2. `pending (0s)` is four different problems.** A job no runner can accept produces
output byte-identical to a job waiting in a busy queue. Neither `list_pipelines` nor
`get_pipeline` can say *no runner matches this job*, though GitLab knows it. Worse,
the project settings that would fix it were unreachable: there was no `update_project`
tool at all, so the CI toggles — along with default branch, visibility and merge
method — could not be changed through this server.

The end state was a complete-looking automation that silently does nothing: a project
created here can be given CI config, variables and a schedule, and still never run,
with every response reporting success.

## Decision

**Delivery is expressed as a type, not a convention.** `CredentialDelivery` is an enum
with `Stored { key, protected }` and `Revealed { value }`. In the stored arm the secret
is **not in scope** for the renderer, so no future edit can leak it by accident, and a
test asserts precisely that: render a stored credential while a token value exists
nearby, and require that neither the value nor even its prefix appears. The choice of
route, the key-shape check and the rollback-on-failure now live in one shared place
that both credential tools call, so they cannot drift apart again.

**Four pending states are named.** `runner_verdict` distinguishes *no runners
attached*, *all offline*, *no tag match* and *eligible* — four different fixes.
`list_project_runners` reports the verdict for a given tag set, and `create_project`
says at creation time when nothing is attached, which is the cheapest possible moment
to learn it. Eligibility requires all of a job's tags on a single runner, and an
untagged job requires a runner that accepts untagged work — the two failure modes that
look like a tag typo but are not.

**`update_project`** covers the CI toggles first, plus default branch, visibility,
merge method and description.

## Consequences

- The safe path for a secret is the same in every tool that creates one, and it is
  checkable rather than merely recommended.
- A stuck pipeline can be diagnosed in one call instead of by elimination.
- A project created through this server can be made able to run.
- Method note, seventh in this series: the recurring defect has been **a state the
  tool cannot observe rendered as a state the reader recognises** — `n/a` as zero,
  empty as absent, unmatched as queued. The fix is always the same shape: give the
  unobservable state a name of its own and say it out loud.
