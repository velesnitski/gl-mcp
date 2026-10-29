# ADR 074: Audit every top-level group; an unreachable host is not an alert

## Status

Accepted (2026-10-29)

## Context

1. **Coverage depends on naming every group.** `group_path` takes one group or a
   comma-separated list (ADR 072). An instance whose code is spread over many
   top-level groups needs every one listed, and a group created later is silently
   missing from the weekly email until someone notices. The list also became the
   email subject verbatim, which gets long fast.
2. **Unreachable hosts paged the team.** A tool call against an instance that can
   only be reached over a VPN, made while the VPN was off, failed with "error
   sending request for url" and went to error tracking as a high-priority issue.
   Nothing was wrong with the server or the instance.

## Decision

1. **`*` means every top-level group the token can see**, and `-name` excludes one:
   `group_path = "*,-sandbox"`. Works for the MCP tool and `--dependency-audit`.
2. **The scope label for `*` does not list the groups** ("all top-level groups",
   plus exclusions). The weekly diff restarts its baseline when the scope label
   changes. A group appearing later is new coverage, and its findings should read as
   new, not reset every other finding's history.
3. **Short names in subjects and titles:** one group keeps its name, several read
   "first + N groups", and `*` reads as the instance host.
4. **"error sending request for url" is not actionable.** The request never left
   the caller's machine: DNS, a refused connection, or a VPN-only host. A server
   that was reached and then failed (timeouts, 5xx, 408, 429) still alerts.

## Consequences

- Switching an existing weekly job from a named group to `*` changes its scope
  label once, so that week is a baseline.
- With an admin token, `*` includes every group on the instance. Exclude sandboxes
  and personal playgrounds with `-name`.
- A GitLab instance that is down for everyone also stops alerting through this
  path; that is monitored where the instance runs, not by its API clients.
