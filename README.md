# Sippion

**English** | [日本語](README.ja.md)

Sippion is a local, read-only MCP server that helps AI coding agents find the
right parts of a repository before they start opening source files broadly.

It exposes one tool, `repo_context`, which combines bounded lexical search,
structural context, and source-only semantic ranking to return a small,
relevant set of code excerpts.

## Quick start

Install Sippion with one command. The bootstrap verifies its downloaded
installer checksum **and GitHub artifact attestation before executing the
installer**. The installer then verifies the selected binary checksum and its
GitHub artifact attestation before installing it and running transactional
`sippion setup`.

The default path fails closed unless the GitHub CLI (`gh`) is installed, has
`gh attestation` support, and can authenticate to GitHub. This deliberately
keeps release provenance verification enabled on the primary install path.

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/a28b611f169a2731ca89dd59db89ccf00940185f/scripts/bootstrap.ps1 | iex
```

After installation:

```text
verify installer checksum + GitHub artifact attestation
    ↓
verify binary checksum + GitHub artifact attestation
    ↓
Sippion installed
    ↓
Codex + Claude Code + Antigravity pre-registered
    ↓
Restart those AI clients
```

Both attestation checks are bound to the Sippion repository, the expected
release signer workflow, and the exact commit SHA resolved from the selected
release tag.

Sippion pre-registers **all three clients**, even if one is not installed yet.
Each client launches Sippion with `--root-auto`. Sippion selects the nearest
recognized Git/project boundary and refuses automatic selection of the user's
home directory or filesystem root. On Unix, shared group/other-writable
ancestor directories are not trusted as automatic boundaries. On Windows,
`--root-auto` is deliberately limited to projects under the canonical current
user profile because Sippion cannot safely verify arbitrary shared-directory
ACLs through stable safe Rust APIs; trusted projects elsewhere can use an
explicit `--root`. You do not need to register Sippion separately for every
repository under the normal automatic scope.

A checksum-only direct installer mode remains available as an explicit opt-out
for controlled environments where provenance was verified by another trusted
mechanism. See [Security and trust boundary](docs/security.md).

## What Sippion does

A client can ask Sippion for focused repository context such as:

```text
repo_context {"q":"authentication token validation"}
```

Sippion returns bounded excerpts and structural evidence instead of dumping a
large part of the repository into the model context.

Typical flow:

```text
AI coding agent
    ↓ asks what part of the repo matters
Sippion repo_context
    ↓ returns focused evidence
AI opens the relevant source files normally
```

This is useful for large repositories, unfamiliar codebases, and multi-agent
workflows where broad source exploration would otherwise consume time and
context.

Optional `session_id` and `agent_id` values can coordinate cooperating agents
in process memory. They are not persisted.

## Safety model

Sippion itself is:

- local stdio MCP
- project-scoped
- read-only
- no-network while serving repository context
- RAM-only for retrieval state; it does not create a persistent index

It does **not** run repository code, proxy model traffic, store provider
credentials, start a daemon, or modify the repository.

Repository reads are bounded, reject symlinks and unsafe hard links, revalidate
source identity around reads, and redact high-confidence secrets before output.
Repository text is treated as **untrusted data**, not as instructions to the AI.

For the full trust boundary and installation trust model, see
[Security and trust boundary](docs/security.md).

## Supported clients

`sippion setup` configures the current user for:

- Codex
- Claude Code
- Antigravity

Restart an already-running client after installation so it reloads its MCP
configuration.

Useful commands:

```sh
sippion setup
sippion doctor
sippion uninstall
```

`setup` is idempotent and transactional across the managed client files. It
refuses to rewrite a Sippion-managed text block if its management markers are
missing, duplicated, or out of order, rather than risking unrelated user
settings. Managed files and their managed parent directories are refused when
they are symlinks. On Unix, MCP client configuration files are created or
repaired as owner-only `0600`; rollback also restores the previous permission
bits. Persistent `.sippion-backup` copies are not created, and legacy copies
from older releases are removed transactionally. If any client update fails,
files touched by that setup attempt are restored.

`doctor` checks registration health and exits non-zero when any expected
registration is unhealthy. `uninstall` is transactional too: it snapshots the
managed configuration/rule files before removal and restores the pre-attempt
state if any removal fails. It removes Sippion-managed client configuration and
rules but does not remove unrelated settings or the binary.

See [Client setup](docs/clients.md) for manual configuration and diagnostics.

## Run Sippion manually

To infer a safe project root from the current directory:

```sh
sippion mcp --root-auto
```

Automatic discovery uses the nearest recognized Git/project marker. It does not
continue past a nearer project manifest merely to find a farther `.git` marker;
on Unix it also stops before trusting a group/other-writable shared directory.
Resolving the current user's home directory is part of the safety check, so
failure to resolve it stops automatic discovery instead of silently disabling
the home/ancestor guard.

On Windows, `--root-auto` is limited to projects under the canonical current
user profile. To use a trusted project elsewhere, bind it explicitly:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

Home-directory, filesystem-root, and home-ancestor scans are rejected by
default. An intentional broad manual scan requires the explicit
`--allow-broad-root` opt-in. Setup never enables that override.

To lower the adaptive scan ceiling:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

## How it works

Retrieval starts with a RAM-only lexical index, parses only ranked candidates,
adds bounded source-only semantic evidence, and packs verified excerpts into a
bounded response. Search-term matching is Unicode-aware while filesystem safety
policy remains deliberately separate and conservative.

Sippion is a repository-context tool, not a compiler or language server. It
does not claim compiler-authoritative type resolution or LSP-grade references.

See [Architecture](docs/architecture.md) and
[Integration boundaries](docs/integrations.md) for details.

## Development

The project pins Rust 1.85.0 and commits `Cargo.lock`.

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

CI also audits `Cargo.lock` against the RustSec advisory database.

The native binary is `target/release/sippion` or
`target/release/sippion.exe` on Windows.

## For maintainers: releases

Supported release binaries:

```text
sippion-linux-x86_64
sippion-windows-x86_64.exe
sippion-macos-aarch64
sippion-macos-x86_64
```

Release workflows build all four targets, generate portable SHA-256 files, and
produce GitHub artifact attestations. Third-party GitHub Actions are pinned to
full commit SHAs. Pull-request supply-chain smoke builds and assembles the
release payload without minting distributable attestations, then separately
verifies a published installer and binary with the same strict repository,
signer-workflow, and source-SHA policy used by the installers.

For an automated prerelease after a version bump reaches `main`, create a
one-shot `release/vX.Y.Z[-prerelease]` branch that points exactly at current
`main`. The release workflow validates the version, creates or verifies the tag,
publishes the prerelease, and deletes the one-shot branch after success.
Manual draft-release dispatches must be run from the exact tag ref supplied as
input so the workflow source SHA and built source SHA cannot diverge.

## Documentation

- [日本語 README](README.ja.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
