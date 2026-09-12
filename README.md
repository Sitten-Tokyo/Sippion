# Sippion

[![Release](https://img.shields.io/github/v/release/Sitten-Tokyo/Sippion?label=release)](https://github.com/Sitten-Tokyo/Sippion/releases)
[![CI](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml/badge.svg)](https://github.com/Sitten-Tokyo/Sippion/actions/workflows/ci.yml)
[![MCP Registry](https://img.shields.io/badge/MCP_Registry-io.github.Sitten--Tokyo%2Fsippion-blue)](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green)](THIRD_PARTY_NOTICES.md)
[![Rust](https://img.shields.io/badge/rust-1.85-orange)](rust-toolchain.toml)

**English** | [日本語](README.ja.md)

**Ask what matters, then read 3 files instead of 300.**

Sippion is a local, read-only MCP server that hands coding agents the smallest
useful slice of a repository before they start reading files. One tool,
`repo_context`, returns bounded code excerpts with structural evidence, plus
one always-on efficiency rule that stops agents from over-asking,
over-abstracting, and over-explaining.

Sippion exposes one MCP tool, `repo_context`, which combines bounded lexical
search, structural context, and source-only semantic ranking to return a small,
relevant set of code excerpts. `sippion setup` also installs one compact,
always-on efficiency rule for supported clients; there are no separate modes or
runtime prompt downloads.

## Quick start

One command. No GitHub account, no extra tools. Checksums are verified before
anything runs.

### macOS / Linux

```sh
curl -fsSL --proto '=https' --proto-redir '=https' --tlsv1.2 https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.sh | sh
```

### Windows PowerShell

```powershell
irm https://raw.githubusercontent.com/Sitten-Tokyo/Sippion/75d6b27e83b86bec00297cd5b5c05bb014e16904/scripts/bootstrap.ps1 | iex
```

After installation:

```text
Sippion installed
    ↓
Codex + Claude Code + Antigravity + OpenCode pre-registered
    ↓
Restart those AI clients
```

Need supply-chain provenance instead? Prefix `SIPPION_STRICT_PROVENANCE=1`
(sh) or set `$env:SIPPION_STRICT_PROVENANCE="1"` (PowerShell) to verify GitHub
artifact attestations before anything runs. Details in
[Security and trust boundary](docs/security.md).

Sippion pre-registers **all four clients**, even if one is not installed yet.
Each client launches Sippion with `--root-auto`: the nearest Git/project
boundary becomes the root, never your home directory or filesystem root. You
do not need to register Sippion separately for every repository. See
[Security and trust boundary](docs/security.md) for the full boundary rules.

## Try it in 60 seconds

Open an unfamiliar repository and ask your agent: *"where is auth token
validation?"*

Without Sippion, the agent globs the repo, opens a dozen files, and burns
context before answering. With Sippion, it asks one question first:

```text
repo_context {"q":"authentication token validation"}
```

and gets back a handful of bounded excerpts —
`src/auth/validate.rs:42-89`, `src/middleware/session.rs:12-40` — opens two
files, and answers. That is the whole product: narrow first, read second.

|  | Grep / Glob | Persistent indexers | Sippion |
|---|---|---|---|
| Setup | none | daemon + disk index | one command |
| Freshness | always fresh | reindex lag | always fresh (RAM-only) |
| What the model sees | raw matches | whole-repo dump risk | bounded excerpts |
| Network while serving | n/a | varies | none |
| Writes to your repo | no | sometimes | never |

## Official MCP Registry

Sippion is published in the Official MCP Registry as
`io.github.Sitten-Tokyo/sippion`. The canonical Registry record can be inspected
through the stable API at
[the latest Sippion Registry entry](https://registry.modelcontextprotocol.io/v0.1/servers/io.github.Sitten-Tokyo%2Fsippion/versions/latest).

Each release intended for Registry distribution contains four checksummed,
provenance-attested MCPB bundles alongside the native binaries:

```text
sippion-linux-x86_64.mcpb
sippion-windows-x86_64.mcpb
sippion-macos-aarch64.mcpb
sippion-macos-x86_64.mcpb
```

The MCPB manifest asks the host for an explicit project root and launches the
same local stdio server. The bootstrap + `sippion setup` path above remains the
recommended route when you want Sippion to configure Codex, Claude Code,
Antigravity, and OpenCode automatically; Registry/MCPB distribution is an
additional standards-based installation and discovery channel.

## What Sippion does

A client can ask Sippion for focused repository context such as:

```text
repo_context {"q":"authentication token validation"}
```

Sippion returns bounded excerpts and structural evidence instead of dumping a
large part of the repository into the model context. Internal ranking and
budget metadata stay internal unless they are needed for correctness; the model
sees compact paths, line ranges, evidence, and minimal incomplete-search status.

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

## Efficiency layer

Sippion separates three responsibilities so the same instruction is not paid
for repeatedly:

1. MCP server instructions tell the client when to use `repo_context` and when
   to switch back to native file reads.
2. `repo_context` returns the smallest useful repository evidence while keeping
   ranking details internal.
3. The managed global rule asks the coding agent to build the smallest correct
   solution, reuse existing code, avoid speculative abstractions and choices,
   preserve safety checks, and keep user-facing output concise.

The rule is always on and intentionally has no lite/full/ultra modes, Node
hooks, or runtime upstream fetches. Its design is documented in
[Efficiency layer](docs/efficiency-layer.md).

Token changes are evaluated only when correctness is preserved. The committed
[efficiency benchmark pilot](eval/efficiency/README.md) uses four arms, five
runs per task/arm, deterministic correctness checks, and total model tokens. No
LLM judge participates in scoring. Sippion does not publish a token-reduction
claim until real model runs have completed under that protocol.

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
- OpenCode

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

Retrieval starts with a RAM-only lexical index, expands scan work only while the
previous round is still yielding useful evidence, parses ranked candidates, and can add a
bounded set of deterministic import/semantic neighbors. Verified excerpts and structural
facts are then selected by utility per estimated token, with redundant same-file context
discounted. The estimated-token target is a soft packing goal; an independent byte cap is
the hard model-visible output guard. Structural parsing currently covers Rust, Python,
JavaScript/TypeScript, Go, Java, C#, C, and C++. Search-term matching is Unicode-aware
while filesystem safety policy remains deliberately separate and conservative.

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
python3 eval/efficiency/score_test.py
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

To cut a stable release (e.g. `v0.1.0`): bump `Cargo.toml` to the exact
version, merge to `main`, push a one-shot `release/v0.1.0` branch at current
`main`, wait for the workflow to publish, then flip the prerelease flag off:

```sh
gh release edit v0.1.0 --repo Sitten-Tokyo/Sippion --prerelease=false
```

## Credits

The compact efficiency behavior is inspired by
[Ponytail](https://github.com/DietrichGebert/ponytail) and
[i-have-adhd](https://github.com/ayghri/i-have-adhd). Sippion rewrites the ideas
into its own always-on rule rather than vendoring either project. Reviewed
upstream commits are pinned in `upstream.toml`; license notes are in
[Third-party notices](THIRD_PARTY_NOTICES.md).

## Documentation

- [日本語 README](README.ja.md)
- [Architecture](docs/architecture.md)
- [Security and trust boundary](docs/security.md)
- [Client setup](docs/clients.md)
- [Efficiency layer](docs/efficiency-layer.md)
- [Efficiency benchmark pilot](eval/efficiency/README.md)
- [Integration boundaries](docs/integrations.md)
- [Historical RC changes and validation](docs/history/README.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
