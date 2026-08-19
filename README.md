# Sippion

Sippion is a local, project-scoped, read-only MCP server that narrows a
repository before an AI coding agent reads source files natively. It exposes
one tool, `repo_context`, which combines bounded lexical retrieval, structural
context, and source-only semantic ranking.

## What it does

```text
repo_context {"q":"authentication token validation"}
```

Queries return bounded excerpts and structural evidence. Optional
`session_id` and `agent_id` values coordinate cooperating agents in volatile
process memory; they are never persisted.

## Security and trust boundary

Sippion remains local stdio MCP, project-scoped, read-only, and no-network. It
does not handle provider credentials, proxy model traffic, run repository
code, start a daemon, write a persistent index, or modify the repository.
Repository reads are bounded, refuse symlinks and unsafe hard links, verify
source identity around reads, and redact high-confidence secrets before output.
See [Security and trust boundary](docs/security.md) for the complete boundary.

## Install

macOS / Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/main/scripts/install.sh | sh
```

Windows PowerShell:

```powershell
irm 'https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/main/scripts/install.ps1' | iex
```

The installers verify the matching per-platform SHA-256 file, install in the
current user scope, and run `sippion setup`. See [client setup](docs/clients.md)
for manual registration, diagnostics, and uninstall details.

## Basic usage

Build or run a local binary against one trusted project root:

```sh
cargo build --release --locked
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
sippion doctor
```

Lower the adaptive scan ceiling when needed:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

## Supported clients

The installer and `sippion setup` support Codex, Claude Code, and Antigravity.
Keep one Sippion process bound to one intended project root. The repository
also includes the client-specific discovery rules in [AGENTS.md](AGENTS.md) and
[CLAUDE.md](CLAUDE.md).

## How it works

Retrieval starts with a RAM-only lexical index, parses only ranked candidates,
adds bounded source-only semantic evidence, and packs verified excerpts into a
bounded response. It does not claim compiler-authoritative type resolution or
LSP-grade references. See [Architecture](docs/architecture.md) and
[integration boundaries](docs/integrations.md).

## Development

The project pins Rust 1.85.0 and commits `Cargo.lock`. Run the complete local
quality gate:

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

The native binary is `target/release/sippion` or
`target/release/sippion.exe` on Windows.

## Release artifacts

The supported artifacts are:

```text
sippion-linux-x86_64
sippion-windows-x86_64.exe
sippion-macos-aarch64
sippion-macos-x86_64
```

Run `.github/workflows/release-draft.yml` manually with an existing tag to
build, checksum, and assemble a GitHub draft release for review. The separate
`.github/workflows/release-artifacts.yml` workflow intentionally remains as a
manual artifact-only path for reviewing or attaching the four build outputs;
both workflows share the reusable build definition in
`.github/workflows/release-build.yml`.

Release checksums are generated in Actions as per-artifact `.sha256` files.
The old root source-archive manifest is not part of the current release path.

## Documentation

- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
