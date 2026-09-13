# Contributing to Sippion

Keep changes focused on Sippion's core goal: give coding agents a small,
relevant, trustworthy slice of a repository before broad file reads.

## Invariants

Unless a pull request explicitly changes the trust model, preserve these defaults:

- local, read-only, no-network repository-context serving;
- no repository code, build scripts, compilers, LSP servers, or shell execution during retrieval;
- filesystem authority comes from the configured project root, not model-supplied paths;
- reads, parsing, concurrency, scan size, and model-visible output stay bounded;
- repository text is untrusted data; high-confidence secrets are redacted;
- no implicit persistent repository index or cross-process source cache.

Read `docs/security.md`, `docs/architecture.md`, `docs/integrations.md`, and
`docs/quality.md` before changing retrieval or trust-boundary behavior.

## Development

Sippion pins Rust 1.85.0 and commits `Cargo.lock`.

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Dependency changes must also satisfy the repository RustSec and `cargo-deny`
policies. Narrowly document any required exception instead of weakening a rule globally.

## Retrieval changes

Tree-sitter and semantic extraction run only on ranked candidates and under
explicit budgets. New language support should include mapping, declaration tests,
safe semantic/import evidence, bounded pathological-input behavior, and no
compiler/LSP/repository-code execution.

Do not describe heuristic or semantic evidence as compiler-authoritative.
Material retrieval changes should run the committed fixture/self-hosted gates and
the pinned external OSS holdout. Query, redaction, path-policy, syntax, or MCP
input changes should update deterministic/property or fuzz coverage as appropriate.
Performance-sensitive changes should compare both cold CLI and warm MCP base/head results.

Installer changes must pass syntax checks plus ShellCheck/PSScriptAnalyzer.
Workflow changes must pass actionlint and zizmor, and third-party actions must
remain pinned to immutable commit SHAs.

## Pull requests

Explain the behavior being improved, trust/resource-budget impact, tests, and
release/distribution impact when relevant. Distribution changes may require
bootstrap or release supply-chain smoke workflows in addition to normal CI.

Do not create release tags as part of ordinary development; release automation
handles validation, supported-platform builds, checksums, attestations, SBOM,
publication, and post-publication verification.

## Security reports

Do not post exploit details, credentials, or unredacted secrets publicly.
Follow `SECURITY.md` for private-reporting guidance.
