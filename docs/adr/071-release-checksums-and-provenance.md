# ADR 071: Release checksums and build provenance

## Status

Accepted (2026-10-26)

## Context

gl-mcp can run unattended inside self-managed GitLab instances that GitHub cannot
reach. These runs are the weekly AI-adoption and
dependency reports. The instance's own CI fetches the latest release binary on a
schedule and keeps it in its package registry.

That binary runs with a read token for every repository in the group, so the
consumer must be able to check two things:

- the file it stores is the file that was published;
- the file was built by this repository's release workflow from the tagged source,
  not uploaded by hand.

Releases had neither.

## Decision

1. Every release publishes `SHA256SUMS` covering all binaries. This catches a
   corrupted or substituted download.
2. Every binary gets a **build provenance attestation** (`actions/attest-build-provenance`,
   signed through Sigstore with the workflow's OIDC identity). This binds the
   artifact digest to this repository, workflow and tag. Consumers verify with
   `gh attestation verify <file> --repo velesnitski/gl-mcp`.
3. The attestation needs `id-token: write` and `attestations: write`. These are
   granted to the release job only. The build jobs keep read-only defaults.

## Consequences

- A checksum hosted next to the binary does not protect against a compromised
  release, because whoever can replace one can replace both. Provenance does: it
  cannot be produced outside this workflow.
- Syncing consumers should verify the checksum always, and the attestation where
  the `gh` CLI and Sigstore are reachable.
- Releases before v2.1.1 have neither file. Consumers start at v2.1.1.

## Addendum (2026-11-11): Intel macOS built on the Apple-silicon runner

The Intel macOS binary was built on a runner image GitHub has since retired; its
job was cancelled before starting, the release step was skipped, and two tags
(2.7.0, 2.7.1) were left without published binaries. It is now cross-compiled for
`x86_64-apple-darwin` on `macos-latest`, which the Apple toolchain supports
natively. No other target changes. A tagged release that publishes nothing should
be noticed: the `release` job is skipped, not failed, when a build is cancelled.
