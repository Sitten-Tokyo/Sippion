# Client setup

The recommended bootstrap installs Sippion for the current user and then runs
`sippion setup`. Setup pre-registers user-scoped MCP entries for Codex, Claude
Code, and Antigravity while preserving unrelated settings. Existing client
sessions must be restarted after setup because MCP configuration is normally
loaded at startup.

## Install

macOS / Linux:

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.ps1 | iex
```

The bootstrap URL is pinned to a specific Git commit instead of `main`. It
selects one published Sippion release, verifies the release installer checksum,
and runs that installer. The release installer verifies the matching platform
binary checksum, installs Sippion in the current user scope, and runs
`sippion setup`.

The normal one-command path does not require GitHub login. See
[Security and trust boundary](security.md) for the stricter authenticated
artifact-attestation path and the exact trust model.

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
