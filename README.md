# Sippion v0.1 RC30 — Multi-Agent Shared Context


> **RC30 package identity:** the post-RC29 hardening snapshot is packaged as `0.1.0-rc.30` so it cannot be confused with an earlier RC29 payload. The 2026-08-16 hardening strengthens credential redaction (including multiline JSON/YAML secrets and short explicit Authorization credentials), rejects non-UTF-8/non-Unicode repository paths without lossy normalization, and prevents Windows RAM-index reuse across top-level searches when metadata-only freshness could be ambiguous. See `PATCH_NOTES_20260816.md`.

> **2026-08-17 completeness follow-up:** ignore-rule-hidden content now prevents absolute repository-wide `NO_MATCH` claims, and queries that match only redacted secret material return a safe content-withheld existence marker.

> **2026-08-17 Windows cache follow-up:** Windows now serializes top-level structural-map construction and clears the prior shared analysis/graph caches before each map. This closes the same-size/same-mtime stale-structure gap left after the RAM-index hardening. A Windows regression test seeds stale same-stamp symbols and graph centrality and verifies that neither is reused.

Sippion is a local, project-scoped, read-only MCP server that narrows a repository before an AI coding agent performs native file reads.

The name **Sippion** combines “sip” and “ion”: AI consumes tokens in small sips,
with the goal of shrinking the token footprint to something as small as an ion.
It reflects Sippion’s purpose of reducing unnecessary context and token usage while
preserving the information an agent needs.

RC30 keeps the one-tool surface and optional coordination metadata for cooperating agents:

```text
repo_context {"q":"authentication token validation"}
repo_context {"q":"token refresh implementation","session_id":"bugfix-123","agent_id":"implementation"}
repo_context {"q":"token refresh tests","session_id":"bugfix-123","agent_id":"tests"}
```

RC30 retains RC29's completeness/cache-privacy hardening, RC28's bounded multi-agent coordination, and RC27's completeness/safety hardening. It closes two completeness/privacy gaps: policy-excluded files can no longer yield an absolute repository-wide `NO_MATCH`, and shared analysis caches retain only structural symbol metadata rather than source-line signatures.

## Internal service boundary

The stdio MCP transport is separated from repository retrieval through a small internal
`RepositoryService` interface:

```text
MCP / JSON-RPC
      │
      ▼
RepositoryService
      │
      ▼
LocalRepositoryService
      │
      ▼
RepositoryAccess
```

Sippion still ships only the local in-process implementation. No daemon, socket, background
service, persistent index, or network transport is added. The boundary is an internal seam for
testing and possible future local IPC; it is not a daemon protocol or an external stable API.

## RC30 pipeline

```text
repo_context(q)
  │
  ├─ Tier 0: lexical retrieval
  │  ├─ RAM-only incremental hashed index
  │  ├─ BM25 + path ranking + session/agent memory
  │  ├─ sibling-agent diversity adjustment
  │  └─ adaptive scan: 32 → 64 → 128 → 256 → 512 MiB max
  │
  ├─ Tier 1: syntax
  │  ├─ Tree-sitter on already-ranked candidates
  │  ├─ declaration/symbol extraction
  │  ├─ verified-stamp AST/symbol cache
  │  ├─ per-file single-flight parsing across concurrent agents
  │  └─ heuristic fallback for unsupported languages
  │
  ├─ Tier 2: source-only semantics
  │  ├─ exact identifier references
  │  ├─ call/type/implementation contexts
  │  ├─ import/module hints
  │  ├─ shared semantic-fact cache
  │  └─ verified-stamp structural graph cache + weighted PageRank
  │
  └─ adaptive context pack
     ├─ deduplicated bounded excerpts
     ├─ structural/semantic summary
     └─ 8 / 16 / 24 / 32 KiB hard output tier
```

## Why semantic analysis stops before compiler execution

RC30 preserves semantic ranking without launching an LSP, compiler, build script, procedural macro, shell command, or repository code. This preserves Sippion's existing trust boundary.

The semantic graph uses these default edge strengths:

| Evidence | Weight |
|---|---:|
| implementation context | 0.95 |
| call context | 0.90 |
| type context | 0.85 |
| exact AST identifier reference | 0.80 |
| import/module hint | 0.40 |
| raw lexical symbol occurrence | 0.15 |

These edges are **ranking evidence**, not authoritative compiler facts. RC30 does not claim complete namespace/type inference, macro expansion, compiler type checking, or LSP-grade go-to-definition/find-references. Those capabilities should remain an explicit trusted/on-demand tier if added later.

## Multi-agent coordination

`session_id` and `agent_id` are optional. Both are bounded to 96 ASCII bytes and accept only letters, digits, `.`, `_`, `:`, and `-` (with an alphanumeric first character). They are volatile metadata and are never persisted.

Recommended use:

```text
Parent task: session_id=bugfix-123
  implementation agent: agent_id=implementation
  callers agent:        agent_id=callers
  tests agent:          agent_id=tests
  security agent:       agent_id=security
```

Within one `session_id`, Sippion mildly favors continuity for repeated queries from the same `agent_id` and mildly de-prioritizes paths already surfaced to sibling agents when query terms overlap. Strong lexical/semantic evidence still dominates; diversity is a bounded ranking adjustment, not a hard exclusion.

Structural reuse is process-wide where the platform exposes a strong enough cross-request freshness identity. Windows deliberately disables cross-request reuse for the metadata-sensitive caches:

- cold-start RAM-index growth uses file-level single-flight so concurrent queries do not repeatedly read/index the same unchanged source file;
- on non-Windows targets, AST/symbol and source-only semantic facts are cached by project-relative path plus verified `SourceStamp`;
- concurrent requests for the same file use single-flight analysis, so one agent parses while siblings wait for that bounded result instead of repeating Tree-sitter work;
- on non-Windows targets, semantic/lexical structural graphs are cached by the canonical candidate file set plus verified stamps, so agents that reach the same set in different rank orders can reuse the graph;
- on Windows, the RAM lexical index is deliberately discarded at the start of each top-level search and those searches are serialized; structural-map calls are also serialized and clear the prior analysis/graph caches before reading candidates. This avoids trusting size + modification time as a cross-request content identity when a same-size replacement can preserve them, while adaptive work inside one top-level operation can still share freshly built state;
- caches store structural facts and stamps only: cached symbols contain `name`, `kind`, and `line`, while source-line signatures are reconstructed from the freshly verified/redacted source for each call; source bodies and source-line signatures are never retained in the shared analysis cache; capacities remain bounded in RAM (256 analyzed files / 64 graph entries);
- where cross-request caches are enabled, changed source stamps invalidate reuse automatically.

Concurrency is raised from 2 to 4 in-flight tool calls. Rate limiting is two-level rather than disabled: up to 8 calls/60s per agent identity and 24 calls/60s process-wide. Expired actor buckets are removed so arbitrary agent IDs cannot grow limiter state without bound.

## Adaptive retrieval budget

The old fixed 128 MiB per-call budget is replaced by a bounded adaptive ceiling.

- normal start: 32 MiB;
- expansion sequence: 32 → 64 → 128 → 256 → 512 MiB;
- expansion occurs only while the result is incomplete, confidence is below the stop threshold, time remains, and the configured ceiling has not been reached;
- `--scan-budget-mib 16..512` now sets the **maximum adaptive ceiling**, not an amount that every call must consume;
- each round reserves roughly 75% for changed/unindexed RAM-index growth and 25% for candidate verification;
- source reads remain capped at 2 MiB per individual source file;
- files deliberately excluded by source policy (including ignore-rule-hidden content, oversized files, stable non-UTF-8 input, and Unix/Windows hard-linked files) are reported conservatively as `policy_excluded` and do not trigger futile adaptive rescans; any such exclusion prevents an absolute repository-wide `NO_MATCH` claim and is surfaced as `NO_MATCH_IN_SEARCHABLE_SET` when no searchable hit exists;
- candidate-list pruning marks the result incomplete, so discarded candidates can never produce a complete `NO_MATCH`;
- the RAM index stores hashed lexical statistics and file stamps, never source bodies.

The response reports:

```text
budget_bytes=<adaptive allowance granted>
budget_cap_bytes=<configured ceiling>
rounds=<adaptive rounds>
confidence=<0.000..1.000>
policy_excluded=<count>
```

The confidence score combines query-term coverage, top-result separation, evidence depth, index coverage, and completion state. It is a retrieval heuristic rather than a correctness probability.

## Adaptive model-visible context

The old fixed 8,000-byte response cap is replaced by bounded context tiers:

| Tier | Hard cap | Soft local token target |
|---|---:|---:|
| narrow/high-confidence | 8 KiB | ~1,800 |
| moderate | 16 KiB | ~3,600 |
| broad | 24 KiB | ~5,400 |
| ambiguous/multi-file | 32 KiB | ~7,200 |

The hard maximum remains 32 KiB; output is never unbounded. The pack tier is selected from confidence, evidence count, structural breadth, and query breadth.

## Supported syntax grammars

Tree-sitter parsing is used only for already-ranked candidates and currently supports:

- Rust;
- Python / `.pyi`;
- JavaScript / JSX;
- TypeScript / TSX;
- Go.

Other languages use the conservative common-declaration fallback. Tree-sitter parsing and all subsequent AST walks share a per-file deadline; AST traversal additionally has a hard node budget, and bounded import scanning checks the same deadline/cancellation signal.

## Safety boundary

Sippion remains:

- local STDIO MCP only;
- project-scoped;
- read-only;
- no network client;
- no provider credentials or model/API proxy;
- no shell-command execution;
- no LSP/compiler subprocess execution;
- no build-script or procedural-macro execution;
- no persistent repository index;
- no embedding/vector database;
- no production file writes;
- symlink-refusing for repository reads;
- Unix and Windows repository reads also reject files with more than one hard link;
- source verification compares size, modification time, and (on Unix) device/inode/status-change/link-count identity before and after reading;
- bounded by file, discovery, scan, AST-node, wall-clock, result-count, and output limits;
- model-visible secret redaction for high-confidence credential/private-key patterns; if the query matches only redacted secret material, the result preserves a safe existence marker without exposing the value or source line.

These controls reduce exposure but do not replace a dedicated secret scanner or sandbox. The project root is still a **trusted root**. Stable Rust does not expose a portable hard-link count on every platform. Sippion enforces hard-link rejection on Unix directly from file metadata and on Windows from the already-open file handle via `GetFileInformationByHandle`; other targets still require the trusted-root boundary and an OS sandbox when hostile local filesystem mutation is in scope.

## Query bounds

`repo_context` accepts 1–8 distinct likely technical terms and at most 512 UTF-8 bytes.

Result breadth is also adaptive by query width:

- 1 term: up to 8 search hits / 6 structural files;
- 2–4 terms: up to 16 / 12;
- 5–8 terms: up to 24 / 16.

## Build

The project pins Rust 1.85.0 in `rust-toolchain.toml` and declares `rust-version = "1.85"`.

```sh
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --check
```

Binary:

```text
target/release/sippion
```

Windows:

```text
target/release/sippion.exe
```

The application commits `Cargo.lock`. Release builds and tests therefore use
`--locked` so dependency resolution is reproducible.

## Build and run on Linux, Windows, and macOS

The Rust source is shared across platforms, but each platform needs its own
native executable. Rust 1.85.0 is pinned in `rust-toolchain.toml`.

### Linux x86_64

```sh
rustup show active-toolchain
cargo build --release --locked
cargo test --locked
./target/release/sippion mcp --root "/home/user/my project"
```

### Windows x86_64 (MSVC)

Use the MSVC Rust toolchain with the Visual Studio C++ build tools installed.
PowerShell and `cmd.exe` both accept the executable path below; quote paths
that contain spaces or non-ASCII characters.

```powershell
cargo build --release --locked
cargo test --locked
& "C:\Tools\sippion.exe" mcp --root "C:\Users\User Name\日本語 project"
```

In TOML strings, a Windows backslash is escaped as `\\`; in JSON strings it
is also escaped as `\\`:

```toml
[mcp_servers.sippion]
command = "C:\\Tools\\sippion.exe"
args = ["mcp", "--root", "C:\\Users\\User Name\\日本語 project"]
cwd = "C:\\Users\\User Name\\日本語 project"
```

```json
{
  "command": "C:\\Tools\\sippion.exe",
  "args": ["mcp", "--root", "C:\\Users\\User Name\\日本語 project"]
}
```

### macOS Apple Silicon and Intel

Build on Apple Silicon (`aarch64-apple-darwin`) for Apple Silicon Macs and on
Intel (`x86_64-apple-darwin`) for Intel Macs. The binaries are not
interchangeable at the CPU level; use the matching artifact, or build locally
with the matching native toolchain.

```sh
cargo build --release --locked
cargo test --locked
./target/release/sippion mcp --root "/Users/user/project with spaces"
```

For every platform, the complete local quality gate is:

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

## MCP client setup

Use an absolute binary path and bind one Sippion process to one trusted project root.

Default adaptive ceiling is 512 MiB, while normal retrieval starts at 32 MiB. You can lower the ceiling, for example:

```sh
sippion mcp --root /ABSOLUTE/PATH/TO/PROJECT --scan-budget-mib 128
```

For a user-wide client configuration, `--root .` and `cwd = "."` mean the
client's current project directory. This makes Sippion available across
projects while preserving the one-process/one-trusted-root boundary. Use an
absolute `--root` and `cwd` when a client does not start MCP processes from the
active project directory.

### Codex

To make Sippion available in every Codex project, add this to the user-scoped
`~/.codex/config.toml`:

```toml
[mcp_servers.sippion]
command = "/ABSOLUTE/PATH/TO/sippion"
args = ["mcp", "--root", "."]
cwd = "."
enabled_tools = ["repo_context"]
```

For one project only, use `.codex/config.toml` with an absolute root:

```toml
[mcp_servers.sippion]
command = "/ABSOLUTE/PATH/TO/sippion"
args = ["mcp", "--root", "/ABSOLUTE/PATH/TO/PROJECT"]
cwd = "/ABSOLUTE/PATH/TO/PROJECT"
enabled_tools = ["repo_context"]
```

Linux all-projects example:

```toml
[mcp_servers.sippion]
command = "/home/user/bin/sippion"
args = ["mcp", "--root", "."]
cwd = "."
enabled_tools = ["repo_context"]
```

Windows all-projects example (`%USERPROFILE%\.codex\config.toml`):

```toml
[mcp_servers.sippion]
command = "C:\\Tools\\sippion.exe"
args = ["mcp", "--root", "."]
cwd = "."
enabled_tools = ["repo_context"]
```

macOS all-projects example (`~/.codex/config.toml`):

```toml
[mcp_servers.sippion]
command = "/Users/user/bin/sippion"
args = ["mcp", "--root", "."]
cwd = "."
enabled_tools = ["repo_context"]
```

### Claude Code

```sh
claude mcp add --transport stdio --scope user sippion -- \
  /ABSOLUTE/PATH/TO/sippion mcp --root .
```

Use `--scope user` for all projects on the current machine. Use `--scope local`
or `--scope project` when the server should be limited to one project.

Windows PowerShell example:

```powershell
claude mcp add --transport stdio --scope user sippion -- `
  "C:\Tools\sippion.exe" mcp --root .
```

Verify the registration with `claude mcp list` or `/mcp`. Claude Code's MCP
transport is local STDIO; it does not require a network endpoint for Sippion.

### Antigravity

Antigravity IDE and CLI support local STDIO MCP servers. For all projects on
the current machine, edit `~/.gemini/config/mcp_config.json`. For one workspace,
use `.agents/mcp_config.json` instead.

macOS/Linux:

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

Windows:

```json
{
  "mcpServers": {
    "sippion": {
      "command": "C:\\Tools\\sippion.exe",
      "args": ["mcp", "--root", "."],
      "cwd": "."
    }
  }
}
```

Reload the MCP configuration and inspect `/mcp` in Antigravity. If an
Antigravity project contains multiple unrelated folders, configure one
Sippion process per trusted repository root rather than broadening `--root`.

### Automatic repository discovery

Registering an MCP server makes its tool available; it does not force an agent
to call it for every prompt. To make repository discovery automatic when it is
useful, add this rule to the client's persistent instructions:

```md
When repository understanding or search is required, call the Sippion
`repo_context` tool before broad recursive searches or reading many files.
Keep Sippion read-only and scoped to the current project root. If it is
unavailable, do not claim it was used; fall back to native tools.
```

Codex uses `AGENTS.md`, Claude Code uses `CLAUDE.md`, and Antigravity supports
global `~/.gemini/GEMINI.md` or workspace `.agents/rules/` rules. Keep this
instruction short so simple tasks do not incur an unnecessary Sippion call.

## One-command installation and setup

After Sippion is published in a GitHub repository, the canonical release should expose
`scripts/install.sh`, `scripts/install.ps1`, each platform binary, and a matching
`<artifact>.sha256` file. The installer downloads the platform-specific binary, verifies
its SHA-256 checksum, installs it in the current user scope, and runs `sippion setup`.
It does not require administrator/root access.

The repository owner must replace `OWNER/REPOSITORY` in the installer defaults before
publishing. Until that is done, the URL is intentionally rejected instead of silently
downloading from an unknown location.

macOS / Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/OWNER/REPOSITORY/main/scripts/install.sh | SIPPION_RELEASE_BASE_URL=https://github.com/OWNER/REPOSITORY/releases/latest/download sh
```

Windows PowerShell:

```powershell
$env:SIPPION_RELEASE_BASE_URL='https://github.com/OWNER/REPOSITORY/releases/latest/download'; irm 'https://raw.githubusercontent.com/OWNER/REPOSITORY/main/scripts/install.ps1' | iex
```

The installer configures the user-wide MCP entry and the short repository-discovery
rule for Codex, Claude Code, and Antigravity. It preserves unrelated settings and
creates a `.sippion-backup` file before changing an existing client configuration.
A client that is not installed is still prepared for later use; the client must be
installed separately and restarted after setup. Existing client sessions must be
restarted because MCP configuration is normally loaded at startup.

For an already downloaded binary, run:

```sh
sippion setup
sippion doctor
```

`sippion setup` is idempotent. `sippion doctor` reports missing or mismatched
registrations. `sippion uninstall` removes only Sippion's managed MCP entries and
global rules; it never deletes the binary or unrelated client settings.

The configured server remains local STDIO and is started by each AI client when needed.
The installer does not create a daemon, network listener, repository-wide home-directory
root, secret store, or automatic tool approval. Each client still binds one Sippion process
to the current project through `mcp --root .`.

## Release artifacts

Build and distribute separate artifacts per operating system and CPU
architecture:

```text
sippion-linux-x86_64
sippion-windows-x86_64.exe
sippion-macos-aarch64
sippion-macos-x86_64
```

The source remains shared, but one executable cannot serve all three operating
systems. Linux ARM64, Windows ARM64, and musl artifacts are optional follow-up
targets rather than required CI gates.

A maintainer can run `.github/workflows/release-draft.yml` manually with an existing
tag to build the same four artifacts and create a GitHub **draft** release containing
the binaries and checksums. It never creates a tag, runs on a tag push, or publishes
the release automatically; a maintainer must review and publish it in GitHub.

The manually triggered `.github/workflows/release-artifacts.yml` builds and
tests the four artifacts above, stages `SHA256SUMS` and a unique
`<artifact>.sha256` file, and uploads them as GitHub Actions artifacts. It does
not create or publish a GitHub Release on tag creation. A maintainer can review
the workflow output and attach approved artifacts and matching checksum files to
a release through the project's normal release process.

## Important implementation locations

- `src/repo.rs`: adaptive retrieval loop, confidence scoring, RAM index, session/agent diversity memory, bounded source verification, single-flight structural analysis cache, semantic graph cache, weighted ranking.
- `src/syntax.rs`: Tree-sitter declarations plus source-only semantic reference/import extraction.
- `src/hybrid.rs`: BM25, symbol extraction, weighted PageRank, conservative excerpt compaction.
- `src/main.rs`: stdio MCP/JSON-RPC framing, one-tool dispatch, bounded multi-agent concurrency/rate limiting, protocol validation, and cancellation handling.
- `src/setup.rs`: idempotent user-wide Codex, Claude Code, and Antigravity MCP configuration, global discovery rules, backups, diagnostics, and targeted uninstall.
- `scripts/install.sh` / `scripts/install.ps1`: checksum-verified per-platform installers that invoke `sippion setup`.
- `.github/workflows/release-draft.yml`: manually triggered draft-release assembly; it requires explicit review before publication.
- `src/service.rs`: internal `RepositoryService` boundary, current in-process `LocalRepositoryService`, adaptive context-tier selection, structural/semantic summary, and evidence packing.
- `src/core.rs`: MCP schema/capability declaration, optional `session_id`/`agent_id`, and 8/16/24/32 KiB model-visible budget policy.

See `CHANGES_RC30.md`, `CHANGES_RC29.md`, `CHANGES_RC28.md`, `CHANGES_RC27.md`, `CHANGES_RC26.md`, and `INTEGRATIONS.md` for the hardening delta, prior adaptive-semantic delta, and integration boundaries.
