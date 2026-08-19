# RC26 adaptive semantic changes (historical record)

- Bumped package version to `0.1.0-rc.26`.
- Kept the single public MCP tool: `repo_context`.
- Replaced the default fixed 128 MiB retrieval budget with bounded adaptive retrieval:
  - starts at 32 MiB;
  - expands through 64 / 128 / 256 / 512 MiB only when incomplete and low-confidence;
  - `--scan-budget-mib 16..512` is now the adaptive ceiling.
- Added deterministic retrieval confidence metadata and adaptive-round/cap reporting.
- Replaced the fixed ~8 KB model-visible response with bounded 8 / 16 / 24 / 32 KiB tiers.
- Added source-only semantic extraction on already-ranked Tree-sitter candidates:
  - exact identifier references;
  - call context;
  - type context;
  - implementation/inheritance context where represented by supported grammars;
  - import/module path hints.
- Added weighted repository edges:
  - implementation `0.95`;
  - call `0.90`;
  - type `0.85`;
  - exact reference `0.80`;
  - import `0.40`;
  - lexical fallback `0.15`.
- Added weighted PageRank so stronger semantic evidence contributes more than raw name coincidence.
- Updated structural output to expose semantic edge kind and weight.
- Preserved the safety boundary: no network client, shell execution, LSP/compiler subprocess, macro expansion, build script, procedural macro, persistent index, or repository mutation.
- Kept `Cargo.lock` absent rather than fabricating a lockfile without a Rust toolchain.

## Important semantic caveat

RC26 improves **semantic ranking**, but it does not claim compiler-authoritative type resolution or LSP-grade find-references/go-to-definition. Full compiler/LSP analysis should remain a separately authorized, sandboxed, on-demand tier because it can interact with build scripts, procedural macros, toolchains, and repository-controlled execution paths.

## Validation status

The packaging environment for this archive did not contain `cargo`, `rustc`, or `rustfmt`. Static source/TOML checks, safety-boundary scans, checksum verification, and ZIP integrity are performed during packaging; compilation, unit tests, Clippy, formatting, and MCP conformance remain release gates.
