# RC27 source-level validation

This archive contains source-level hardening only. The separate RC26 release-validation gate was intentionally not claimed as solved.

Checked in this packaging environment:

- candidate-pruning completeness flag is wired into `SearchOutcome.truncated`;
- metadata-known policy exclusions are removed before effective index-coverage calculation;
- stable non-UTF-8 policy skips persist across adaptive rounds within one call;
- verified post-read source stamps are stored in the RAM index;
- Unix hard-link rejection is applied at discovery and open-handle read time;
- AST traversal/import scanning contains deadline/cancellation checks and a node budget;
- regression tests for the new invariants are present in the source tree;
- no `Cargo.lock` was fabricated.

Not executed here and still mandatory before release:

```text
cargo generate-lockfile
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --check
MCP integration/conformance tests
```
