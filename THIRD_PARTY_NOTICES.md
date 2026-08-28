# Third-Party Notices

Sippion uses third-party Rust crates. The source archive does not vendor those crates; Cargo resolves them during the build. The project itself is licensed under `MIT OR Apache-2.0`.

## Direct dependencies

| Dependency | Purpose | Declared license family |
|---|---|---|
| `aho-corasick` | bounded multi-pattern structural matching | Unlicense OR MIT |
| `serde` | data serialization | MIT OR Apache-2.0 |
| `serde_json` | JSON serialization / MCP frames | MIT OR Apache-2.0 |
| `cap-std` | capability-scoped filesystem access | permissive MIT/Apache-family licenses; verify resolved crate metadata |
| `cap-fs-ext` | no-follow filesystem extensions | permissive MIT/Apache-family licenses; verify resolved crate metadata |
| `ignore` 0.4.23 | ignore-aware repository walking; conservative pin while targeting Rust 1.85 | Unlicense OR MIT |
| `tree-sitter` | bounded syntax-tree parsing of top-ranked files | MIT |
| `tree-sitter-rust` | Rust grammar | MIT |
| `tree-sitter-python` | Python grammar | MIT |
| `tree-sitter-javascript` | JavaScript/JSX grammar | MIT |
| `tree-sitter-typescript` | TypeScript/TSX grammar | MIT |
| `tree-sitter-go` | Go grammar | MIT |
| `tree-sitter-java` 0.23.5 | Java grammar | MIT |
| `tree-sitter-c-sharp` 0.23.5 | C# grammar | MIT |
| `tree-sitter-c` 0.24.2 | C grammar | MIT |
| `tree-sitter-cpp` 0.23.4 | C++ grammar | MIT |
| `winapi-util` 0.1.11 (Windows only) | safe open-handle file metadata / hard-link count | Unlicense OR MIT |

## Release requirement

Before distributing a release binary, generate and commit `Cargo.lock`, then produce a dependency/license report from the exact locked dependency graph. This file records the direct dependency boundary; `Cargo.lock` is authoritative for exact transitive versions.

The names RTK, codebase-memory-mcp, RepoMapper, Repomix, Engram, and Agent Capability Registry are design references documented in `docs/integrations.md`. Their source code is not vendored by this archive.
