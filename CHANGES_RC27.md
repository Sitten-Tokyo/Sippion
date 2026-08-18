# RC27 completeness and safety hardening

- Bumped package version to `0.1.0-rc.27`.
- Candidate generation now records when the ranked candidate list is pruned before exact source verification. Any such pruning forces `SearchOutcome.truncated = true`, so an n-gram/path candidate cap can never be misreported as a complete `NO_MATCH`.
- Discovery now treats metadata-known oversized sources as deliberate policy exclusions instead of unfinished index coverage. Stable non-UTF-8 sources discovered during a search are cached for the remainder of that adaptive call and likewise removed from the effective searchable denominator. `SearchCoverage.policy_excluded_files` reports the count.
- Tree-sitter post-parse AST traversal now shares the per-file deadline/cancellation guard and has a 500,000-node hard budget. Source import scanning is deadline/cancellation bounded as well.
- Source verification now compares a stronger open-handle stamp before and after reading. On Unix this includes device, inode, status-change time, and hard-link count in addition to length and modification time. Indexed documents store the verified post-read stamp rather than stale discovery metadata.
- Unix reads reject any regular file whose hard-link count is greater than one, preventing a repository entry from aliasing an out-of-root inode through a hard link. Discovery reports such files as policy exclusions. Non-Unix retains the documented trusted-root requirement because stable Rust does not expose a portable hard-link count on every platform.
- Added regression tests for candidate-pruning completeness, oversized/non-UTF-8 policy exclusions, same-length file replacement stamps, Unix hard-link rejection, and the AST node budget.

## Intentionally not completed in this archive

RC27 does **not** close the separate release-validation gate from RC26: `Cargo.lock` generation, compilation, tests, Clippy, rustfmt, and MCP conformance still need to be run in a Rust-enabled environment before release.
