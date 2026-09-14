# Client setup

The recommended bootstrap installs Sippion for the current user and then runs
`sippion setup`. Setup pre-registers user-scoped MCP entries for Codex, Claude
Code, Antigravity, and OpenCode while preserving unrelated settings. Existing
client sessions must be restarted after setup because MCP configuration is
normally loaded at startup.

## Install

The default path needs no GitHub CLI or authentication. It verifies the
published SHA-256 checksums of the release installer and the selected release
binary before running anything. For GitHub artifact-attestation provenance,
set `SIPPION_STRICT_PROVENANCE=1` (sh) or
`$env:SIPPION_STRICT_PROVENANCE="1"` (PowerShell); strict mode requires the
GitHub CLI (`gh`) with `gh attestation` support and working GitHub
authentication, and fails closed when provenance cannot be verified.

macOS / Linux:

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

Windows PowerShell:

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

The bootstrap URL is pinned to a specific Git commit instead of `main`. It
selects one non-draft published Sippion release and verifies the release
installer checksum before executing it. With `SIPPION_STRICT_PROVENANCE=1` it
additionally resolves the tag to an exact commit SHA and verifies the
installer GitHub artifact attestation, bound to the Sippion repository, the
expected release-draft signer workflow, and the selected release commit SHA.

The release installer then verifies the matching platform binary checksum
(plus its GitHub artifact attestation in strict mode), installs Sippion in the
current user scope, and runs transactional `sippion setup`.

Direct use of the release installer keeps strict attestation verification on
by default; `SIPPION_REQUIRE_ATTESTATION=0` remains an explicit opt-out for
controlled environments where provenance was verified by another trusted
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
fail-closed error instead of risking unrelated settings. Managed files and the
managed parent directories (`~/.codex`, `~/.claude`, `~/.gemini`,
`~/.gemini/config`, and `~/.config/opencode`) are refused when they are
symlinks. On Unix, MCP configuration files are owner-only `0600` and rollback
restores the previous permission bits. Setup does not create persistent
`.sippion-backup` copies; legacy copies are removed transactionally. If any
setup operation fails, files touched by that setup attempt are restored to
their pre-attempt state. `doctor` reports missing, mismatched, malformed, or
unreadable registrations and exits non-zero when any expected registration is
unhealthy.

`uninstall` is transactional as well: it snapshots the same eight managed files
and legacy backup siblings before mutation and restores the pre-attempt state if
any removal fails. It removes only Sippion-managed entries and rules; it does
not remove the binary or unrelated settings.

## Run locally

Use guarded automatic discovery when the process starts inside the active
project:

```sh
sippion mcp --root-auto
```

Automatic discovery selects the nearest recognized Git/project boundary. It
does not continue past a nearer project manifest merely to prefer a farther
`.git` marker, and on Unix it refuses to trust group/other-writable shared
directories as automatic boundaries. Home-directory and filesystem-root
selection also fails closed. Home resolution itself is part of this guard: if
Sippion cannot resolve the current user's home directory, automatic discovery
stops rather than silently dropping the home/ancestor check.

On Windows, stable safe Rust APIs do not expose enough DACL information for
Sippion to prove that an arbitrary ancestor is not shared-writable while the
crate keeps `unsafe` code forbidden. Therefore `--root-auto` is deliberately
limited to projects under the canonical current-user profile. For a trusted
project elsewhere (for example another drive or a shared workspace), configure
an explicit `--root` instead.

You can bind each process explicitly to one trusted project root:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

The default adaptive ceiling is 512 MiB and retrieval normally starts at
32 MiB. Explicit home-directory, filesystem-root, or home-ancestor scans are
rejected unless a manual invocation also supplies `--allow-broad-root`. When
that broad-root override is not supplied, home resolution also fails closed so
the guard cannot silently disappear. Setup never enables that override.

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

OpenCode uses `~/.config/opencode/opencode.json` for the user-wide MCP
registration:

```json
{
  "mcp": {
    "sippion": {
      "type": "local",
      "command": ["/ABSOLUTE/PATH/TO/sippion", "mcp", "--root-auto"],
      "cwd": "."
    }
  }
}
```

Sippion keeps repository-discovery guidance in both the MCP server instructions
and the managed client rule, so clients use `repo_context` before broad
exploration and switch to native reads after narrowing. The managed rule does
not prescribe coding style, implementation minimalism, question cadence, or
response formatting. Codex and OpenCode read the managed rule from `AGENTS.md`,
Claude Code from `CLAUDE.md`, and Antigravity from `GEMINI.md`. Cooperating
agents can share a `session_id` and use distinct `agent_id` values for retrieval
coordination.
