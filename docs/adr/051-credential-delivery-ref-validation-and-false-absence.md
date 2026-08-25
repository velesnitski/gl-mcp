# ADR 051: Credential delivery, ref validation, and refusing a false absence

## Status

Accepted (2026-08-25)

## Context

Three gaps, each one where the obvious implementation is the wrong one.

**1. The only credential this server could create was the one that cannot push.**
Deploy tokens have no `write_repository` scope. Any workflow that automates its way up
to a push therefore breaks to the UI at precisely the step that matters, after
everything around it was automated — the least useful place to stop.

The obvious fix, a `create_project_access_token` that returns the token, reproduces a
worse problem. Output from an MCP server lands in a model's context and in the
conversation transcript, both of which are retained and logged. A credential that
arrives there has to be treated as disclosed from that moment, whatever the caller
then does with it. "Returns the value once" is a sane contract for a web UI shown to
one human; it is not one for a channel that is recorded by construction.

**2. A schedule on a ref that does not exist is accepted and silently never runs.**
GitLab returns success, the schedule appears in the UI, and nothing fires. Reporting
success for something that cannot run is worse than reporting an error, because an
error gets investigated and a green result does not.

**3. `search_code` advertised regex support it does not have.** GitLab search is term
and substring matching: `foo|bar` is matched literally and finds nothing. The failure
direction is the damaging one — a zero-result sweep reads as "this string appears
nowhere in the group", which is the precise claim such sweeps are run to establish. A
live query returned 0 while a single-term retry returned dozens, including a match in
a file that had already been read in the same session.

## Decision

**Delivery is chosen before the credential exists.** `create_project_access_token`
returns no value by default. The caller either names a CI/CD variable, in which case
the token is written there masked and only metadata comes back, or explicitly asks to
have it revealed and is told in the response that it is now disclosed. Choosing
neither is refused *before* creation: a token nobody can reach is litter, and cleaning
it up is strictly worse than never issuing it. All validation — scopes, access level,
variable-key shape — is pre-flight for the same reason.

**A failed variable write revokes the token.** A credential that was created but could
not be delivered is pure liability, so it is deleted rather than orphaned. If the
revocation *also* fails, the response says so and names the id, because the one thing
worse than a dangling token is a dangling token nobody was told about.

**The schedule ref is resolved first**, through the commits endpoint so one call
covers branches, tags and SHAs alike. An unresolvable ref is refused, and the created
schedule's response points at `play_pipeline_schedule` — a schedule's first real proof
is a run, and waiting an interval to discover a misconfiguration is the slowest
feedback loop available.

**Alternation is detected rather than advertised away.** The query runs exactly as
given, because `a|b` may legitimately be the text being searched for and a literal
query that works is never second-guessed. Only when it returns nothing *and* looks
like alternation is it split, each alternative searched, and the substitution stated
plainly in the output. The parameter description now says what the search actually
does instead of promising regex.

## Consequences

- A workflow that needs to push can be completed without dropping to the UI.
- The safe path for a secret is the default path, and the unsafe one announces itself.
- A schedule that is reported as created can actually run.
- A sweep can no longer report absence it did not establish.
- Method note, sixth in this series: **the previous five defects were all a tool
  describing itself more confidently than it behaved.** This one adds the inverse
  obligation — where the honest answer is unavailable, say so in the output rather
  than choosing the reading that looks like success.
