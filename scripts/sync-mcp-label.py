#!/usr/bin/env python3
"""Sync the gl-mcp entry's config key to "gl-mcp v<version>".

Why this exists: Claude Code's /mcp dialog labels each server by its *config
key*, NOT by the serverInfo.name the server reports during initialize. So
gl-mcp embedding the version in serverInfo.name surfaces it nowhere in the
dialog — the only lever is the config key, and a hand-typed version goes stale
the moment the binary is bumped.

This keeps the key truthful automatically: it finds the gl-mcp entry by its
*binary path* (not the key, which may already carry a version), asks that exact
binary its version (`--version`), and renames the key to "gl-mcp v<version>".
Idempotent, atomic write per file, keeps a .bak.

gl-mcp is registered in a project `.mcp.json` (e.g. ~/Downloads/.mcp.json), not
necessarily ~/.claude.json, so both are searched (plus $GL_MCP_CONFIG if set).

Run via `make install` (build + sync), then restart Claude Code / `/mcp` to see
the new label. Mirrors slk-mcp's sync-mcp-label.py (fleet-wide pattern).
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

BINARY_MATCH = "gl-mcp"  # path fragment identifying the gl-mcp server entry

CONFIG_PATHS = [
    os.path.expanduser("~/.claude.json"),
    os.path.expanduser("~/Downloads/.mcp.json"),
]
if os.environ.get("GL_MCP_CONFIG"):
    CONFIG_PATHS.insert(0, os.path.expanduser(os.environ["GL_MCP_CONFIG"]))


def mcp_containers(cfg):
    """Every mcpServers dict in a config: the root one plus per-project ones."""
    out = []
    if isinstance(cfg.get("mcpServers"), dict):
        out.append(cfg["mcpServers"])
    for proj in (cfg.get("projects") or {}).values():
        if isinstance(proj, dict) and isinstance(proj.get("mcpServers"), dict):
            out.append(proj["mcpServers"])
    return out


def binary_version(command):
    try:
        r = subprocess.run([command, "--version"], capture_output=True, text=True, timeout=10)
        return r.stdout.strip()
    except Exception as e:  # noqa: BLE001 — best-effort tooling
        print(f"  ! could not read version from {command}: {e}")
        return ""


def rename_in(container):
    """Rename the gl-mcp entry's key to 'gl-mcp v<version>'. Returns True if changed."""
    changed = False
    for key in list(container.keys()):
        entry = container[key]
        command = entry.get("command", "") if isinstance(entry, dict) else ""
        if BINARY_MATCH not in command:
            continue
        version = binary_version(command)
        if not version:
            continue
        new_key = f"gl-mcp v{version}"
        if key == new_key:
            print(f"  = already '{new_key}'")
            continue
        # Preserve insertion order: rebuild with just this key renamed.
        rebuilt = {(new_key if k == key else k): v for k, v in container.items()}
        container.clear()
        container.update(rebuilt)
        print(f"  ✓ '{key}' → '{new_key}'")
        changed = True
    return changed


def sync_file(path):
    if not os.path.exists(path):
        return False
    try:
        with open(path) as f:
            cfg = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        print(f"  ! skipping {path}: {e}")
        return False
    if not any(rename_in(c) for c in mcp_containers(cfg)):
        return False
    shutil.copy2(path, path + ".bak")
    # Atomic replace so a crash never leaves the config half-written.
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path) or ".", suffix=".tmp")
    with os.fdopen(fd, "w") as f:
        json.dump(cfg, f, indent=2, ensure_ascii=False)
    os.replace(tmp, path)
    print(f"updated {path} (backup: {path}.bak)")
    return True


def main():
    touched = False
    for path in CONFIG_PATHS:
        print(f"· {path}")
        if sync_file(path):
            touched = True
    if touched:
        print("→ restart Claude Code (or '/mcp' reconnect) to see the new label")
    else:
        print("nothing to update (gl-mcp entry not found or label already current)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
