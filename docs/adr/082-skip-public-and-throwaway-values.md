# ADR 082: Skip public and throwaway values, with a reason

**Status:** Accepted
**Date:** 2026-11-12

## Context

After ADR 080 and 081, three kinds of finding still reached the report, and none
of them needs a rotation:

- Front-end build variables. Nuxt, Next.js, Vite, Create React App, Vue CLI, Expo
  and Gatsby inline variables with these prefixes into the JavaScript they ship.
  A client-side DSN or a payment provider's client token is meant to be public;
  masking it in CI protects nothing.
- An access key ID in a CI file. The variable check already skipped it ("names a
  key, signs nothing"); the CI-file check still reported it as MEDIUM.
- A password for a test service container (the official database images'
  initialisation variables, and the job's connection that reuses the same value)
  whose value is also trivial. Both conditions say it is a throwaway.

Each one appeared repeatedly and pushed real findings down the list.

## Decision

- `browser_public_key()` recognises the front-end prefixes. Such a name is not a
  secret name, in CI files and in variables, unless it says SECRET, PRIVATE,
  PASSWORD or PASSWD: that is a secret about to be published, and stays reported.
- `SecretHit::not_reported()` returns a reason for an identifier, and for a
  service-container password with trivial evidence. Callers drop those hits:
  `audit_ci_security` lists them on its skip line as `file:line KEY (reason)`,
  and the weekly report leaves them out.
- A service-container password that is not trivial is still MEDIUM: a real-looking
  value may be reused somewhere that matters.
- The skip line covers both sources: "Secret-named variables and CI-file values not
  reported, and why (value not shown)".

## Consequences

- Fewer MEDIUM findings, and every skip can still be checked by name and reason.
- A front-end variable that holds a real server-side secret under a public prefix
  without a secret word in its name is not reported. That value ships to every
  browser anyway; the fix is in the application, not in CI masking.
