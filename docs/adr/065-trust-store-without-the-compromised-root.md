# ADR 065: Trust store without the compromised root

## Status

Accepted (v1.14.1)

## Context

A certificate authority generated weak keys for eight intermediate
certificates; they were revoked and browsers blocked them. The `webpki-roots`
crate, which this binary compiles in as its trust store, carried the root
those intermediates chain to up to 1.0.10 (the same root the Python `certifi`
advisory GHSA-6q48-6c7w-9gw2 covers). A party on the network path could
present a certificate chained to that root and read or alter this server's
HTTPS traffic, credentials included.

## Decision

Update `webpki-roots` to 1.0.11, which removes the affected root, and rebuild.
No code change; the test suite is the evidence the update is inert.
