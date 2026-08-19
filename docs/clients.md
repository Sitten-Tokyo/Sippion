# Client setup

The installers configure user-scoped MCP entries for Codex, Claude Code, and
Antigravity while preserving unrelated settings. Existing client sessions must
be restarted after setup because MCP configuration is normally loaded at
startup.

## Install

macOS / Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/main/scripts/install.sh | sh
```

Windows PowerShell:

```powershell
irm 'https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/main/scripts/install.ps1' | iex
```

Both installers download the platform binary and its matching
`<artifact>.sha256`, verify SHA-256, install in the current user scope, and run
`sippion setup`. Override the release source with
`SIPPION_RELEASE_BASE_URL` when using a fork or private distribution.

For an existing binary:

```sh
sippion setup
sippion doctor
sippion uninstall
```

`setup` is idempotent. `doctor` reports missing or mismatched registrations,
and `uninstall` removes only Sippion-managed entries and rules; it does not
remove the binary or unrelated settings.

## Run locally

Bind each process to one trusted project root:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

The default adaptive ceiling is 512 MiB and retrieval normally starts at
32 MiB. Use an absolute root when a client does not start in the active
project directory.

## Manual client registration

Codex user configuration (`~/.codex/config.toml`):

```toml
[mcp_servers.sippion]
command = "/ABSOLUTE/PATH/TO/sippion"
args = ["mcp", "--root", "."]
cwd = "."
enabled_tools = ["repo_context"]
```

Claude Code:

```sh
claude mcp add --transport stdio --scope user sippion -- \
  /ABSOLUTE/PATH/TO/sippion mcp --root .
```

Use `claude mcp list` or `/mcp` to verify the registration. Antigravity uses
`~/.gemini/config/mcp_config.json` for a user-wide registration or
`.agents/mcp_config.json` for one workspace:

```json
{
  "mcpServers": {
    "sippion": {
      "command": "/ABSOLUTE/PATH/TO/sippion",
      "args": ["mcp", "--root", "."],
      "cwd": "."
    }
  }
}
```

For automatic repository discovery, Codex reads `AGENTS.md` and Claude Code
reads `CLAUDE.md`. Both files carry the same short rule: call Sippion before
broad repository exploration, keep it project-scoped and read-only, and fall
back honestly when it is unavailable. `AGENTS.md` additionally documents
sharing a `session_id` and distinct `agent_id` values for cooperating agents;
that extra instruction is intentionally client-specific.
