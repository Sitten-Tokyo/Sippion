# RC26 packaging validation

Performed in the packaging environment:

- parsed `Cargo.toml` successfully with a TOML parser;
- verified package version is `0.1.0-rc.26` and Rust minimum remains `1.85`;
- checked Rust source delimiter/string/comment balance with a local static scanner;
- scanned `src/` for newly introduced shell-command, process-spawn, network-socket/client, unsafe-block, or symlink-following primitives;
- confirmed the direct dependency set is unchanged from RC25;
- regenerated `SHA256SUMS` after all source/document changes;
- verified every checksum with `sha256sum -c`;
- verified the final ZIP with `unzip -t`.

Not available in this packaging environment:

- `cargo`;
- `rustc`;
- `rustfmt`.

Therefore the following remain mandatory release gates in a Rust-enabled environment:

```sh
cargo generate-lockfile
cargo build --release --locked
cargo test --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --check
```

MCP conformance/integration testing should also be run before publishing the RC as release-ready.
