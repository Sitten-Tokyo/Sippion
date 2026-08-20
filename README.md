# Sippion

**English** | [日本語](README.ja.md)

Sippion is a local, read-only MCP server that helps AI coding agents find the
right parts of a repository before they start opening source files broadly.

It exposes one tool, `repo_context`, which combines bounded lexical search,
structural context, and source-only semantic ranking to return a small,
relevant set of code excerpts.

## Quick start

Install Sippion with one command. The installer verifies checksums and runs
`sippion setup` automatically. No GitHub login is required.

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/e69e13c9f34710e953722c11628b9f50df93bb7f/scripts/bootstrap.ps1 | iex
```

After installation:

```text
Sippion installed
    ↓
Codex + Claude Code + Antigravity pre-registered
    ↓
Restart those AI clients
    ↓
Open a project and use Sippion
```

Sippion pre-registers **all three clients**, even if one is not installed yet.
Each client launches Sippion with `--root .`, so the project opened by that
client becomes Sippion's read-only project root. You do not need to register
Sippion separately for every repository.

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

For the full trust boundary and the stricter authenticated artifact-attestation
install path, see [Security and trust boundary](docs/security.md).

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

`setup` is idempotent. `doctor` checks registration health. `uninstall` removes
Sippion-managed client configuration and rules but does not remove unrelated
settings.

See [Client setup](docs/clients.md) for manual configuration and diagnostics.

## Run Sippion manually

To bind Sippion explicitly to one project root:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

To lower the adaptive scan ceiling:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

## How it works

Retrieval starts with a RAM-only lexical index, parses only ranked candidates,
adds bounded source-only semantic evidence, and packs verified excerpts into a
bounded response.

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
full commit SHAs, and the release supply-chain smoke workflow exercises build,
attestation, artifact upload/download, and installer attestation without
publishing a release.

For an automated prerelease after a version bump reaches `main`, create a
one-shot `release/vX.Y.Z[-prerelease]` branch that points exactly at current
`main`. The release workflow validates the version, creates or verifies the tag,
publishes the prerelease, and deletes the one-shot branch after success.

## Documentation

- [日本語 README](README.ja.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
