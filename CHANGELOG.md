# Changelog

All notable user-visible changes to Sippion are tracked here. Historical detailed RC notes remain under `docs/history/`.

## [Unreleased]

### Changed

- Repository context packing now explicitly optimizes utility per estimated model-input token, discounts redundant same-file atoms, and exposes typed retrieval/packing diagnostics without reparsing model-visible text.
- Adaptive retrieval confidence now uses exact evidence plus RAM-index document-frequency rarity instead of treating shorter queries as inherently more specific.
- Semantic/import expansion now resolves ambiguous same-stem candidates deterministically using module match, directory proximity, language/extension affinity, and path tie-breaking.
- Retrieval evaluation now separates search ranking from packed model-visible paths and compares token cost against full-source, top-ranked-full-file, and deterministic grep-window baselines.
- Release-sensitive pull requests now keep the required `RustSec dependency audit` check pending until the exact-head release supply-chain smoke succeeds, so manual and automated merges share the same release gate.
- Official MCP Registry publishing now verifies the exact published record through the stable `/v0.1` API, including active status and package metadata, and confirms newly released versions are `latest`.
- Retrieval evaluation now covers natural-language queries, ambiguous lexical noise, and cross-file evidence, with expected-path recall, unnecessary-file ratio, latency, and context-size regression gates.
- MCPB package inputs now normalize metadata, and CI requires repeated packs of identical inputs to be byte-for-byte reproducible.
- Generated `server.json` metadata now includes the stable GitHub repository ID, project website, and publisher icon metadata.

### Documentation

- State explicitly that Sippion organizes and bounds repository context before it is passed to AI models in order to reduce unnecessary model-input token consumption.
- Document the Official MCP Registry name and per-platform MCPB distribution in the README.

## [0.1.0-rc.36] - 2026-08-28

### Added

- Retrieval evaluation with Recall@5/MRR and model-visible byte/token regression gates.
- Opt-in `query`, `inspect`, and machine-readable/verbose Doctor diagnostics without expanding `repo_context`.
- Black-box MCP conformance checks using the official MCP client implementation.
- Per-platform MCPB release packaging, generated `server.json`, and post-release Official MCP Registry publication via GitHub OIDC.
- Bounded Tree-sitter and source-only semantic support for Java, C#, C, and C++ in addition to Rust, Python, JavaScript/TypeScript, and Go.
- CycloneDX JSON SBOM generation as a checksummed, provenance-attested release asset with post-publication verification.
- Top-level contributor and security-reporting guidance.

### Changed

- Dependency changes require the release supply-chain smoke gate because they change the generated SBOM.

## [0.1.0-rc.35] - 2026-08-28

### Added

- Complete post-publication verification of release assets, checksums, and provenance attestations.
- Deterministic property-style coverage for secret redaction, denied path variants, and Unicode canonical-equivalence tokenization.

### Changed

- Release tags are materialized only after all platform builds, tests, checksums, and attestations succeed.
- Trusted-author merges wait for every path-applicable smoke workflow and bind validation to the tested base/head pair.
- Unicode retrieval tokenization folds before token boundaries and uses Unicode-scalar semantic minimum lengths.
- Duplicate dependency versions are denied by default with narrow documented exceptions for unavoidable locked transitive versions.

### Fixed

- Added a `workflow_run` backstop so releases published by GitHub Actions still trigger strict post-publication verification despite `GITHUB_TOKEN` recursive-trigger suppression.

[Unreleased]: https://github.com/Sitten-Tokyo/Sippion/compare/v0.1.0-rc.36...HEAD
[0.1.0-rc.36]: https://github.com/Sitten-Tokyo/Sippion/compare/v0.1.0-rc.35...v0.1.0-rc.36
[0.1.0-rc.35]: https://github.com/Sitten-Tokyo/Sippion/releases/tag/v0.1.0-rc.35
