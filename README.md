# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

**English** | [日本語](README.ja.md)

**Ask what matters, then read 3 files instead of 300.**

Sippion is a local, read-only MCP server for coding agents. Its single tool,
`repo_context`, narrows a repository to a small set of relevant, bounded source
excerpts before the agent starts opening files broadly.

- one MCP tool: `repo_context`
- local stdio, read-only, no network while serving repository context
- RAM-only retrieval state; no persistent repository index
- bounded lexical + structural + semantic retrieval
- high-confidence secret redaction
- setup for Codex, Claude Code, Antigravity, and OpenCode

## Install

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

Then restart your AI client. `sippion setup` pre-registers all four supported
clients and launches Sippion with `--root-auto`, which selects the nearest safe
project boundary instead of your home directory or filesystem root.

For strict GitHub artifact-attestation verification, set
`SIPPION_STRICT_PROVENANCE=1` before running the installer. See
[Security and trust boundary](docs/security.md).

Sippion is also published in the Official MCP Registry as
`io.github.Sitten-Tokyo/sippion`:
[latest Registry entry](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest).

## Use

A coding agent asks Sippion for focused context:

```text
repo_context {"q":"authentication token validation"}
```

Sippion returns a few bounded excerpts and structural evidence. The agent then
opens only the source files it actually needs.

```text
AI coding agent
    ↓ narrow the search
Sippion repo_context
    ↓ focused evidence
AI reads the relevant source files
```

Optional `session_id` and `agent_id` values coordinate cooperating agents in
process memory; they are not persisted.

Useful commands:

```sh
sippion setup
sippion doctor
sippion uninstall
sippion mcp --root-auto
```

To bind a trusted project explicitly:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT
```

Broad roots such as a home directory or filesystem root are rejected by
default. Manual broad scans require the explicit `--allow-broad-root` opt-in;
setup never enables it.

## Why Sippion

|  | Grep / Glob | Persistent indexers | Sippion |
|---|---|---|---|
| Setup | none | daemon + disk index | one command |
| Freshness | always fresh | reindex lag | always fresh |
| Model input | raw matches | can be broad | bounded excerpts |
| Network while serving | n/a | varies | none |
| Repository writes | no | varies | never |

Sippion is a repository-context tool, not a compiler or language server. Its
semantic evidence is intentionally bounded and is not compiler-authoritative.
Structural parsing currently covers Rust, Python, JavaScript/TypeScript, Go,
Java, C#, C, and C++.

## Safety

Sippion does not run repository code, build scripts, compilers, LSP servers, or
shell commands during retrieval. It does not proxy model traffic, store provider
credentials, start a daemon, or modify the repository.

Repository text is treated as **untrusted data**, never as instructions to the
AI. Reads are project-scoped and bounded; unsafe links are rejected, source
identity is revalidated around reads, and high-confidence secrets are redacted
before model output.

See [Security and trust boundary](docs/security.md) for the complete model.

## Efficiency rule

`sippion setup` installs one compact, always-on rule that asks supported coding
agents to prefer the smallest correct solution, reuse existing code, avoid
speculative abstraction, preserve safety checks, and keep user-facing output
concise. There are no lite/full/ultra modes or runtime prompt downloads.

Correctness comes first: token efficiency is evaluated only when deterministic
checks still pass. See [Efficiency layer](docs/efficiency-layer.md) and the
[benchmark pilot](eval/efficiency/README.md).

## Development

Sippion pins Rust 1.85.0 and commits `Cargo.lock`.

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
python3 eval/efficiency/score_test.py
```

The release binary is `target/release/sippion` (`sippion.exe` on Windows).
Release workflows build Linux x86_64, Windows x86_64 MSVC, macOS Apple Silicon,
and macOS Intel artifacts with checksums and provenance attestations.

See [CONTRIBUTING.md](CONTRIBUTING.md) before changing retrieval, security,
distribution, or workflow behavior.

## Documentation

- [日本語 README](README.ja.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Quality and validation](docs/quality.md)
- [Integration boundaries](docs/integrations.md)
- [Historical changes](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

## Credits

The compact efficiency behavior is inspired by
[Ponytail](https://github.com/DietrichGebert/ponytail) and
[i-have-adhd](https://github.com/ayghri/i-have-adhd). Reviewed upstream commits
are pinned in `upstream.toml`; license notes are in
[Third-party notices](THIRD_PARTY_NOTICES.md).
