# Changelog

All notable user-visible changes to Sippion are tracked here. Historical detailed RC notes remain under `docs/history/`.

## [Unreleased]

### Added

- Bounded Tree-sitter and source-only semantic support for Java, C#, C, and C++ in addition to Rust, Python, JavaScript/TypeScript, and Go.
- CycloneDX JSON SBOM generation as a checksummed, provenance-attested release asset with post-publication verification.
- Top-level contributor and security-reporting guidance.

### Changed

- Dependency changes now require the release supply-chain smoke gate because they change the generated SBOM.

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

[Unreleased]: https://github.com/Sitten-Tokyo/Sippion/compare/v0.1.0-rc.35...HEAD
[0.1.0-rc.35]: https://github.com/Sitten-Tokyo/Sippion/releases/tag/v0.1.0-rc.35
