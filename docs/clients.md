# Client setup

The recommended bootstrap installs Sippion for the current user and then runs
`sippion setup`. Setup pre-registers user-scoped MCP entries for Codex, Claude
Code, and Antigravity while preserving unrelated settings. Existing client
sessions must be restarted after setup because MCP configuration is normally
loaded at startup.

## Install

The default path requires the GitHub CLI (`gh`) with `gh attestation` support
and working GitHub authentication. Installation fails closed if either the
release installer or selected release binary cannot be verified against the
expected Sippion GitHub artifact attestation provenance.

macOS / Linux:

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.ps1 | iex
```

The bootstrap URL is pinned to a specific Git commit instead of `main`. It
selects one non-draft published Sippion release, resolves its tag to an exact
commit SHA, verifies the release installer checksum, and **verifies the
installer GitHub artifact attestation before executing it**. The attestation is
bound to the Sippion repository, the expected release-draft signer workflow,
and the selected release commit SHA.

The release installer then verifies the matching platform binary checksum and
GitHub artifact attestation against the expected release-build signer workflow
and the same source commit, installs Sippion in the current user scope, and runs
transactional `sippion setup`.

A checksum-only direct-installer mode is retained only as an explicit opt-out
for controlled environments where provenance was verified by another trusted
mechanism. See [Security and trust boundary](security.md) for the exact trust
model.

For an existing binary:

```sh
sippion setup
sippion doctor
sippion uninstall
```

`setup` is idempotent and transactional across its managed client files.
Existing Sippion-managed text blocks are rewritten only when exactly one ordered
BEGIN/END marker pair is present; malformed or duplicate markers cause a
fail-closed error instead of risking unrelated settings. Managed-file symlinks
are refused. On Unix, MCP configuration files are owner-only `0600` and rollback
restores the previous permission bits. Setup does not create persistent
`.sippion-backup` copies; legacy copies are removed transactionally. If any
setup operation fails, files touched by that setup attempt are restored to their
pre-attempt state. `doctor` reports missing, mismatched, malformed, or unreadable
registrations and exits non-zero when any expected registration is unhealthy.
`uninstall` removes only Sippion-managed entries and rules; it does not remove
the binary or unrelated settings.

## Run locally

Use guarded automatic discovery when the process starts inside the active
project:

```sh
sippion mcp --root-auto
```

Automatic discovery prefers an enclosing Git repository, then a nearby project
manifest, and fails closed rather than selecting the user's home directory or
filesystem root.

You can also bind each process explicitly to one trusted project root:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

The default adaptive ceiling is 512 MiB and retrieval normally starts at
32 MiB. Explicit home-directory, filesystem-root, or home-ancestor scans are
rejected unless a manual invocation also supplies `--allow-broad-root`. Setup
never enables that override.

## Manual client registration

Codex user configuration (`~/.codex/config.toml`):

```toml
[mcp_servers.sippion]
command = "/ABSOLUTE/PATH/TO/sippion"
args = ["mcp", "--root-auto"]
cwd = "."
enabled_tools = ["repo_context"]
```

Claude Code:

```sh
claude mcp add --transport stdio --scope user sippion -- \
  /ABSOLUTE/PATH/TO/sippion mcp --root-auto
```

Use `claude mcp list` or `/mcp` to verify the registration. Antigravity uses
`~/.gemini/config/mcp_config.json` for a user-wide registration or
`.agents/mcp_config.json` for one workspace:

```json
{
  "mcpServers": {
    "sippion": {
      "command": "/ABSOLUTE/PATH/TO/sippion",
      "args": ["mcp", "--root-auto"],
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
