# ADR 059: Two counts that answer different questions

## Status

Accepted (v1.12.1)

## Context

A full group sweep for a string that appeared nowhere printed
`0 matches in 0 of 104 repos searched`. The sentence put the number of
repositories holding a match in the slot a reader parses as "how many were
searched", so a thorough negative read as no search at all. ADR 058 made
results say what they left out; this one made a complete result sound
incomplete.

`list_projects` had the mirror problem. Asked for twenty projects it printed
`Found: 20 projects` whether the instance had twenty or two hundred, and a
caller searching for a repository that sat on page two concluded it did not
exist.

## Decision

The search summary states scope and finding as two clauses: `N repos
searched, M match(es) in K repo(s)`, and an empty result names the scope:
`No matches in the N repos searched`. A project listing exactly as long as
its page cap says it is a first page and how to see more.

## Consequences

Wording only; no API call changes. A test pins both sentences so the
ambiguity cannot return under a later rewording.
