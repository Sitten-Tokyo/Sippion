# Contributing to Sippion

Thank you for helping improve Sippion. Contributions should preserve the core product goal: give AI coding agents a small, relevant, trustworthy repository context before they broadly open source files.

## Design invariants

Changes must preserve these defaults unless the pull request explicitly proposes a reviewed change to the trust model:

- repository-context serving is local, read-only, and no-network;
- repository code, build scripts, compilers, LSP servers, and shell commands are not executed during retrieval;
- filesystem authority comes from the configured project root, not model-supplied paths;
- source reads, parsing, concurrency, scan size, and model-visible output stay bounded;
- source text is untrusted data and high-confidence secrets are redacted before model output;
- persistent repository indexes or cross-process source caches are not introduced implicitly.

See `docs/security.md`, `docs/architecture.md`, `docs/integrations.md`, and `docs/quality.md` before changing retrieval or trust-boundary code.

## Development environment

Sippion pins Rust 1.85.0 and commits `Cargo.lock`.

```sh
cargo fmt --check
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Dependency changes must also pass the repository RustSec and `cargo-deny` policy. Do not relax a dependency/license/source rule merely to make a new dependency pass; document and narrowly scope any necessary exception.

## Retrieval and language changes

Tree-sitter and semantic extraction run only on already-ranked candidates and are subject to explicit time/node budgets. New language support should include extension-to-grammar mapping, declaration tests, safe semantic/import evidence, bounded pathological-input behavior, and no compiler/LSP/repository-code execution.

Heuristic or semantic evidence must not be described as compiler-authoritative.

Retrieval changes should preserve the committed fixture/self-hosted gates and should also be checked against the pinned external OSS holdout when they materially change ranking, parsing, semantic expansion, or packing. Query-input changes should update deterministic property tests and the relevant fuzz target. Performance-sensitive changes should inspect the base/head comparison instead of relying on a single absolute CI duration. See `docs/quality.md` for the commands and workflow boundaries.

## Pull requests

Keep pull requests focused and explain the user or agent behavior being improved, trust-boundary/resource-budget impact, tests added, and release/distribution impact when applicable.

Distribution-path changes may require bootstrap or release supply-chain smoke workflows in addition to normal CI. Trusted-author automation only merges an exact tested head after all path-applicable gates succeed.

## Releases

Do not create release tags manually as part of ordinary development. The release workflow builds and tests all supported platform artifacts, creates checksums and attestations, generates the SBOM, materializes the tag only after validation, publishes the release, and performs post-publication verification.

## Security reports

Do not include exploit details, credentials, or unredacted secrets in a public issue. Follow `SECURITY.md` for private-reporting guidance.
