# ADR 069: rmcp 3 and a guarded HTTP transport

## Status

Accepted (2026-10-23)

## Context

The dependency audit (ADR 066–068) flagged the MCP SDK the server is built on.
rmcp 0.11 has five advisories. Two of them affect gl-mcp, both only in
`--transport http` mode:

- **DNS rebinding.** The Streamable HTTP server did not validate `Host`. A web page
  could rebind its name to the local address and drive the server from a browser.
- **Unauthenticated session-table growth.** A malformed request inserted a session
  that was never removed, so anyone who could reach the port could exhaust memory.

The other three (OAuth metadata, SSRF, redirect header leak) are on the client side.
The server does not use rmcp as a client outside tests.

Reading the transport code turned up a larger problem than either advisory. HTTP mode
bound `0.0.0.0` with no authentication at all. Every tool runs with the server's
GitLab token, write tools included. Anyone who could reach the port held that token's
powers. The shipped Dockerfile ran exactly this configuration.

## Decision

1. **Upgrade rmcp from 0.11 to 3.7.** That fixes all five advisories.
2. **Pin the pruned tool router.** In rmcp 3, `#[tool_handler]` defaults to a fresh
   `Self::tool_router()` rather than the instance's router. Our router is pruned at
   construction for read-only mode, `GITLAB_TOOLSET` and `DISABLED_TOOLS`. With the
   default, all three would silently stop hiding tools, and read-only mode would list
   write tools again. The handler now names `router = self.tool_router` explicitly.
   The end-to-end protocol test (`read_only_mode_hides_write_tools_on_the_wire`)
   caught this during the upgrade.
3. **Bind loopback by default.** `--host` selects another address.
4. **Off loopback, authentication is mandatory, and the server fails closed.**
   - Binding anything but loopback requires `GL_MCP_HTTP_TOKEN` (at least 24
     characters). Without it, the server refuses to start and says why.
   - Clients send `Authorization: Bearer <token>`. The comparison is constant-time.
   - There is no "insecure" switch. A token costs one line; an open listener costs
     the GitLab token.
5. **Host checking:**
   - On loopback, rmcp's default allow-list (loopback names only) blocks DNS
     rebinding.
   - Off loopback, the bearer token does that job. A rebinding page cannot add a
     header it does not know.
   - `GL_MCP_ALLOWED_HOSTS` can narrow `Host` further for public deployments.
6. **Docker:**
   - The image passes `--host 0.0.0.0`, so it needs `GL_MCP_HTTP_TOKEN`.
   - The compose file publishes on the host's loopback.
   - Secrets come from the environment rather than being written into the file.

## Consequences

- **Breaking change, hence 2.0.0.** HTTP deployments that relied on the open default
  must set `--host` and a token, and their clients must send the header. Stdio use,
  which is every Claude Code and desktop client, is unaffected.
- **The published tool contract changed in representation only.** Optional fields are
  now `type: [T, "null"]` instead of `nullable: true`, and schemas no longer carry the
  Rust struct name as `title`. Names, descriptions and required fields are unchanged.
- **The policy is pure** (`http_guard::policy`, `http_guard::authorized`) and
  unit-tested. The listener itself was also checked live:
  - refused to start off loopback without a token;
  - 403 for a rebinding `Host` on loopback;
  - 401 without a token or with a wrong one, 200 with the right one.
