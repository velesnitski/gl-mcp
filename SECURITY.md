# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.x     | Yes       |

## Reporting a Vulnerability

If you discover a security vulnerability, please report it responsibly:

1. **Do NOT open a public issue**
2. Use [GitHub Security Advisories](https://github.com/velesnitski/gl-mcp/security/advisories/new)
3. Or email: velesnitski@gmail.com

We will acknowledge receipt within 48 hours and provide a timeline for a fix.

## Security Protections

### Credential Handling
- GitLab tokens are loaded from environment variables, never embedded in code
- Tokens are never logged or included in analytics events
- `~/.gl-mcp/teams.json` contains usernames only, no tokens

### Network Security
- HTTPS required by default for all GitLab connections
- HTTP only allowed for `localhost`/`127.0.0.1` or with explicit `GITLAB_ALLOW_HTTP=1`
- URL validation runs at startup before any API calls

### Write Protection
- `GITLAB_READ_ONLY=1` blocks all write operations at the tool level
- `write_guard!` macro enforces this before any API call
- `update_file` hard-blocks writes to protected branches (main/master/develop/release/production)
- `DISABLED_TOOLS` allows granular tool-level access control

### Analytics & Logging
- Analytics log (`~/.gl-mcp/analytics.log`) records tool names and durations only
- Parameters containing tokens, passwords, or content are never logged
- Safe parameter extraction via allowlist (`SAFE_PARAMS`)

### Error Handling
- Error messages from GitLab API are truncated to prevent credential leakage
- Internal errors use generic messages without exposing implementation details
