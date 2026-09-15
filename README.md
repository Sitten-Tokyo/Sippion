# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

**English** | [日本語](README.ja.md)

**Use less AI context. Make API tokens — and subscription usage allowances — last longer.**

Sippion is a local, read-only MCP server that helps AI coding agents consume fewer tokens. Instead of letting an agent open large parts of a repository, Sippion's single tool, `repo_context`, returns a small set of relevant, bounded source excerpts first.

This reduces unnecessary context per task. It can lower usage for API-based workflows, but that is not the only benefit: **Sippion can also help subscription-based AI coding plans last longer by reducing how much context the agent needs to consume.**

## How it works

```text
AI coding agent
    ↓ ask for focused repository context
Sippion repo_context
    ↓ return a few relevant excerpts
AI reads only the files it actually needs
```

Example:

```text
repo_context {"q":"authentication token validation"}
```

Sippion uses bounded lexical, structural, and semantic retrieval while staying local and read-only.

## Install

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

Then restart your AI client. `sippion setup` registers Sippion for Codex, Claude Code, Antigravity, and OpenCode.

Useful commands:

```sh
sippion setup
sippion doctor
sippion uninstall
sippion mcp --root-auto
```

## Why Sippion

- **Fewer tokens:** gives the agent focused repository context instead of broad file reads.
- **Useful beyond API billing:** can help subscription usage allowances last longer too.
- **Local and private:** no network access while serving repository context.
- **Read-only:** never modifies the repository or executes repository code.
- **No persistent index:** retrieval state stays in RAM.
- **Small surface area:** one MCP tool, `repo_context`.

Sippion is a repository-context tool, not a compiler or language server. Structural parsing currently covers Rust, Python, JavaScript/TypeScript, Go, Java, C#, C, and C++.

## Safety

Sippion does not run repository code, build scripts, compilers, LSP servers, or shell commands during retrieval. Repository text is treated as untrusted data, and high-confidence secrets are redacted before model output.

See [Security and trust boundary](docs/security.md) for details.

## Documentation

- [Client setup](docs/clients.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Quality and validation](docs/quality.md)
- [Contributing](CONTRIBUTING.md)

## Credits

The compact efficiency behavior is inspired by [Ponytail](https://github.com/DietrichGebert/ponytail) and [i-have-adhd](https://github.com/ayghri/i-have-adhd). License notes are in [Third-party notices](THIRD_PARTY_NOTICES.md).
