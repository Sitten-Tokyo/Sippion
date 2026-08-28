from pathlib import Path


def replace(path: str, old: str, new: str) -> None:
    target = Path(path)
    text = target.read_text()
    if old not in text:
        raise SystemExit(f"missing patch anchor in {path}: {old[:100]!r}")
    target.write_text(text.replace(old, new, 1))


replace(
    "src/syntax.rs",
    '''        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        _ => None,
''',
    '''        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "ipp" | "tpp" => {
            Some(tree_sitter_cpp::LANGUAGE.into())
        }
        _ => None,
''',
)

replace(
    "src/syntax.rs",
    '''        "enum_declaration" => Some("enum"),
        // Common JavaScript/TypeScript style: `const authenticate = (...) => ...`.
''',
    '''        "enum_declaration" => Some("enum"),
        // Java / C# / C / C++ declarations not covered by the shared names above.
        "constructor_declaration" | "local_function_statement" | "function_definition"
        | "function_declarator" => Some("function"),
        "struct_declaration" | "struct_specifier" => Some("struct"),
        "class_specifier" | "record_declaration" => Some("class"),
        "union_specifier" => Some("union"),
        "enum_specifier" => Some("enum"),
        "annotation_type_declaration" => Some("interface"),
        "delegate_declaration" | "type_definition" => Some("type"),
        "namespace_definition" => Some("module"),
        // Common JavaScript/TypeScript style: `const authenticate = (...) => ...`.
''',
)

replace(
    "src/syntax.rs",
    '''fn identifier_child(node: Node<'_>) -> Option<Node<'_>> {
    if let Some(name) = node.child_by_field_name("name") {
        return Some(name);
    }
    for index in 0..node.named_child_count() {
        let child = named_child(&node, index)?;
        if matches!(
            child.kind(),
            "identifier" | "type_identifier" | "field_identifier" | "property_identifier"
        ) {
            return Some(child);
        }
    }
    None
}
''',
    '''fn is_identifier_like(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "namespace_identifier"
            | "operator_name"
    )
}

fn identifier_descendant(node: Node<'_>) -> Option<Node<'_>> {
    let mut stack = vec![node];
    let mut visited = 0usize;
    while let Some(current) = stack.pop() {
        visited = visited.saturating_add(1);
        if visited > 64 {
            return None;
        }
        if is_identifier_like(current) {
            return Some(current);
        }
        for field in ["name", "declarator"] {
            if let Some(child) = current.child_by_field_name(field) {
                if is_identifier_like(child) {
                    return Some(child);
                }
                stack.push(child);
            }
        }
        for index in (0..current.named_child_count()).rev() {
            if let Some(child) = named_child(&current, index) {
                stack.push(child);
            }
        }
    }
    None
}

fn identifier_child(node: Node<'_>) -> Option<Node<'_>> {
    for field in ["name", "declarator"] {
        if let Some(child) = node.child_by_field_name(field) {
            if let Some(identifier) = identifier_descendant(child) {
                return Some(identifier);
            }
        }
    }
    identifier_descendant(node)
}
''',
)

replace(
    "src/syntax.rs",
    '''            "impl_item" | "implements_clause" | "extends_clause" | "superclass" | "trait_bounds"
''',
    '''            "impl_item"
                | "implements_clause"
                | "extends_clause"
                | "superclass"
                | "super_interfaces"
                | "extends_interfaces"
                | "base_list"
                | "type_list"
                | "trait_bounds"
''',
)

replace(
    "src/syntax.rs",
    '''            "call_expression" | "call" | "macro_invocation" | "await_expression"
''',
    '''            "call_expression"
                | "call"
                | "macro_invocation"
                | "await_expression"
                | "method_invocation"
                | "invocation_expression"
                | "object_creation_expression"
''',
)

replace(
    "src/syntax.rs",
    '''                | "array_type"
''',
    '''                | "array_type"
                | "type_list"
                | "base_list"
                | "superclass"
                | "object_creation_expression"
''',
)

replace(
    "src/syntax.rs",
    '''fn import_paths_from_source_bounded(
''',
    '''fn include_fragment(line: &str) -> Option<&str> {
    quoted_fragment(line).or_else(|| {
        let start = line.find('<')?;
        let tail = &line[start + 1..];
        let end = tail.find('>')?;
        Some(&tail[..end])
    })
}

fn import_paths_from_source_bounded(
''',
)

replace(
    "src/syntax.rs",
    '''            "go" => {
                if trimmed.starts_with("import ")
                    || trimmed.starts_with('"')
                    || trimmed.starts_with('`')
                {
                    quoted_fragment(trimmed).or_else(|| {
                        trimmed
                            .strip_prefix('`')
                            .and_then(|rest| rest.split('`').next())
                    })
                } else {
                    None
                }
            }
            _ => None,
''',
    '''            "go" => {
                if trimmed.starts_with("import ")
                    || trimmed.starts_with('"')
                    || trimmed.starts_with('`')
                {
                    quoted_fragment(trimmed).or_else(|| {
                        trimmed
                            .strip_prefix('`')
                            .and_then(|rest| rest.split('`').next())
                    })
                } else {
                    None
                }
            }
            "java" => trimmed.strip_prefix("import ").map(|rest| {
                rest.trim_start_matches("static ")
                    .trim_end_matches(';')
                    .trim()
            }),
            "cs" => {
                let rest = trimmed
                    .strip_prefix("global using ")
                    .or_else(|| trimmed.strip_prefix("using "));
                rest.map(|rest| {
                    let rest = rest.trim_start_matches("static ").trim_end_matches(';').trim();
                    rest.split_once('=').map_or(rest, |(_, target)| target.trim())
                })
            }
            "c" | "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx"
            | "ipp" | "tpp" => trimmed
                .strip_prefix("#include")
                .and_then(|rest| include_fragment(rest.trim())),
            _ => None,
''',
)

replace(
    "src/syntax.rs",
    '''            let normalized = candidate
                .trim_matches(|ch: char| ch == '"' || ch == '\\'' || ch == '`')
                .replace("::", "/")
                .replace('.', "/");
''',
    '''            let normalized = candidate
                .trim_matches(|ch: char| ch == '"' || ch == '\\'' || ch == '`')
                .replace("::", "/");
            let normalized = if matches!(
                extension.as_str(),
                "c" | "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "ipp" | "tpp"
            ) {
                normalized
            } else {
                normalized.replace('.', "/")
            };
''',
)

replace(
    "src/syntax.rs",
    '''            "identifier" | "type_identifier" | "field_identifier"
''',
    '''            "identifier" | "type_identifier" | "field_identifier" | "namespace_identifier" | "operator_name"
''',
)

replace(
    "src/syntax.rs",
    '''    #[test]
    fn unsupported_language_uses_caller_fallback() {
''',
    '''    #[test]
    fn java_ast_extracts_class_and_method() {
        let source = "class AuthService { boolean validate(String token) { return !token.isEmpty(); } }\n";
        let symbols = extract_ast_symbols("src/AuthService.java", source, 8).expect("java grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "AuthService"));
        assert!(symbols.iter().any(|symbol| symbol.name == "validate"));
    }

    #[test]
    fn csharp_ast_extracts_class_and_method() {
        let source = "class AuthService { bool Validate(string token) { return token.Length > 0; } }\n";
        let symbols = extract_ast_symbols("src/AuthService.cs", source, 8).expect("csharp grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "AuthService"));
        assert!(symbols.iter().any(|symbol| symbol.name == "Validate"));
    }

    #[test]
    fn c_and_cpp_ast_extract_functions_and_types() {
        let c_source = "struct Session { int value; }; int validate_token(int token) { return token > 0; }\n";
        let c_symbols = extract_ast_symbols("src/auth.c", c_source, 8).expect("c grammar");
        assert!(c_symbols.iter().any(|symbol| symbol.name == "Session"));
        assert!(c_symbols.iter().any(|symbol| symbol.name == "validate_token"));

        let cpp_source = "class Validator { public: bool validate(int token) { return token > 0; } };\n";
        let cpp_symbols = extract_ast_symbols("src/auth.hpp", cpp_source, 8).expect("cpp grammar");
        assert!(cpp_symbols.iter().any(|symbol| symbol.name == "Validator"));
        assert!(cpp_symbols.iter().any(|symbol| symbol.name == "validate"));
    }

    #[test]
    fn unsupported_language_uses_caller_fallback() {
''',
)

replace(
    "src/syntax.rs",
    '''    fn typescript_semantics_extract_module_path() {
        let source = "import { validate } from './auth/session';\\nvalidate(token);\\n";
        let facts = extract_semantic_facts_bounded("src/login.ts", source, 64, 16, None, None)
            .expect("typescript grammar");
        assert!(
            facts
                .import_paths
                .iter()
                .any(|path| path.contains("/auth/session"))
        );
    }
}
''',
    '''    fn typescript_semantics_extract_module_path() {
        let source = "import { validate } from './auth/session';\\nvalidate(token);\\n";
        let facts = extract_semantic_facts_bounded("src/login.ts", source, 64, 16, None, None)
            .expect("typescript grammar");
        assert!(
            facts
                .import_paths
                .iter()
                .any(|path| path.contains("/auth/session"))
        );
    }

    #[test]
    fn java_and_csharp_semantics_extract_imports_and_calls() {
        let java = "import com.example.Auth; class Login { void run() { validate(token); } }\n";
        let java_facts = extract_semantic_facts_bounded("src/Login.java", java, 64, 16, None, None)
            .expect("java grammar");
        assert!(java_facts.import_paths.iter().any(|path| path == "com/example/Auth"));
        assert!(java_facts.references.iter().any(|reference| reference.name == "validate" && reference.kind == "call"));

        let csharp = "using Acme.Auth; class Login { void Run() { Validate(token); } }\n";
        let csharp_facts = extract_semantic_facts_bounded("src/Login.cs", csharp, 64, 16, None, None)
            .expect("csharp grammar");
        assert!(csharp_facts.import_paths.iter().any(|path| path == "Acme/Auth"));
        assert!(csharp_facts.references.iter().any(|reference| reference.name == "Validate" && reference.kind == "call"));
    }

    #[test]
    fn c_family_semantics_extract_include_and_call() {
        let source = "#include \"auth/session.h\"\nint login(void) { return validate_token(); }\n";
        let facts = extract_semantic_facts_bounded("src/login.cpp", source, 64, 16, None, None)
            .expect("cpp grammar");
        assert!(facts.import_paths.iter().any(|path| path == "auth/session.h"));
        assert!(facts.references.iter().any(|reference| reference.name == "validate_token" && reference.kind == "call"));
    }
}
''',
)

replace(
    "docs/architecture.md",
    "2. Tree-sitter parses only already-ranked candidates for supported languages;\n",
    "2. Tree-sitter parses only already-ranked Rust, Python, JavaScript/TypeScript, Go, Java, C#, C, and C++ candidates;\n",
)

replace(
    "docs/integrations.md",
    '''The Tier 2 resolver is intentionally **source-only**. It records exact
syntax-tree identifier references, call/type/implementation contexts, and
import paths, then connects those references to declarations already present in
the bounded candidate set.
''',
    '''The Tier 2 resolver is intentionally **source-only**. It records exact
syntax-tree identifier references, call/type/implementation contexts, and
import paths for Rust, Python, JavaScript/TypeScript, Go, Java, C#, C, and C++,
then connects those references to declarations already present in the bounded
candidate set.
''',
)

replace(
    "README.md",
    '''Retrieval starts with a RAM-only lexical index, parses only ranked candidates,
adds bounded source-only semantic evidence, and packs verified excerpts into a
bounded response. Search-term matching is Unicode-aware while filesystem safety
policy remains deliberately separate and conservative.
''',
    '''Retrieval starts with a RAM-only lexical index, parses only ranked candidates,
adds bounded source-only semantic evidence, and packs verified excerpts into a
bounded response. Structural parsing currently covers Rust, Python,
JavaScript/TypeScript, Go, Java, C#, C, and C++. Search-term matching is
Unicode-aware while filesystem safety policy remains deliberately separate and
conservative.
''',
)

replace(
    "THIRD_PARTY_NOTICES.md",
    "| `tree-sitter-go` | Go grammar | MIT |\n",
    "| `tree-sitter-go` | Go grammar | MIT |\n| `tree-sitter-java` 0.23.5 | Java grammar | MIT |\n| `tree-sitter-c-sharp` 0.23.5 | C# grammar | MIT |\n| `tree-sitter-c` 0.24.2 | C grammar | MIT |\n| `tree-sitter-cpp` 0.23.4 | C++ grammar | MIT |\n",
)

replace(
    ".github/workflows/release-draft.yml",
    '''          persist-credentials: false

      - name: Download platform artifacts
''',
    '''          persist-credentials: false

      - name: Install pinned SBOM generator
        run: cargo install cargo-cyclonedx --version '=0.5.9' --locked

      - name: Generate CycloneDX SBOM
        shell: bash
        run: |
          set -euo pipefail
          cargo cyclonedx --format json --spec-version 1.5 --override-filename sippion.cdx.json
          test -s sippion.cdx.json
          jq -e '.bomFormat == "CycloneDX" and .metadata.component.name == "sippion"' sippion.cdx.json >/dev/null

      - name: Download platform artifacts
''',
)

replace(
    ".github/workflows/release-draft.yml",
    '''          mkdir -p release-assets
          : > release-assets/SHA256SUMS
''',
    '''          mkdir -p release-assets
          cp sippion.cdx.json release-assets/sippion.cdx.json
          : > release-assets/SHA256SUMS
''',
)

replace(
    ".github/workflows/release-draft.yml",
    '''            sha256sum install.sh > install.sh.sha256
            sha256sum install.ps1 > install.ps1.sha256
            cat install.sh.sha256 install.ps1.sha256 >> SHA256SUMS
            sha256sum -c SHA256SUMS
''',
    '''            sha256sum install.sh > install.sh.sha256
            sha256sum install.ps1 > install.ps1.sha256
            sha256sum sippion.cdx.json > sippion.cdx.json.sha256
            cat install.sh.sha256 install.ps1.sha256 sippion.cdx.json.sha256 >> SHA256SUMS
            sha256sum -c SHA256SUMS
''',
)

replace(
    ".github/workflows/release-draft.yml",
    '''          done

      - name: Attest installer provenance
''',
    '''          done
          test -s release-assets/sippion.cdx.json
          test -s release-assets/sippion.cdx.json.sha256

      - name: Attest installer and SBOM provenance
''',
)

replace(
    ".github/workflows/release-draft.yml",
    '''          release-assets/install.sh
          release-assets/install.ps1
''',
    '''          release-assets/install.sh
          release-assets/install.ps1
          release-assets/sippion.cdx.json
''',
)

replace(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''      - .github/workflows/release-build.yml
      - .github/workflows/release-draft.yml
''',
    '''      - Cargo.toml
      - Cargo.lock
      - .github/workflows/release-build.yml
      - .github/workflows/release-draft.yml
''',
)

replace(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''          persist-credentials: false

      - name: Download platform artifacts
''',
    '''          persist-credentials: false

      - name: Generate and validate CycloneDX SBOM
        shell: bash
        run: |
          set -euo pipefail
          cargo install cargo-cyclonedx --version '=0.5.9' --locked
          cargo cyclonedx --format json --spec-version 1.5 --override-filename sippion.cdx.json
          test -s sippion.cdx.json
          jq -e '.bomFormat == "CycloneDX" and .metadata.component.name == "sippion"' sippion.cdx.json >/dev/null

      - name: Download platform artifacts
''',
)

replace(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''            SHA256SUMS
            sippion-linux-x86_64
''',
    '''            SHA256SUMS
            sippion.cdx.json
            sippion.cdx.json.sha256
            sippion-linux-x86_64
''',
)

replace(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''            install.ps1.sha256 \\
            sippion-linux-x86_64.sha256 \\
''',
    '''            install.ps1.sha256 \\
            sippion.cdx.json.sha256 \\
            sippion-linux-x86_64.sha256 \\
''',
)

replace(
    ".github/workflows/release-supply-chain-smoke.yml",
    '''          done
        for binary in \\
''',
    '''          done
        gh attestation verify "published/sippion.cdx.json" \\
          --repo "$REPOSITORY" \\
          --signer-workflow "$REPOSITORY/.github/workflows/release-draft.yml" \\
          --source-digest "$RELEASE_SHA" >/dev/null
        for binary in \\
''',
)

replace(
    ".github/workflows/author-auto-merge.yml",
    '''              scripts/bootstrap.sh|scripts/bootstrap.ps1|scripts/install.sh|scripts/install.ps1|.github/workflows/release-build.yml|.github/workflows/release-draft.yml|.github/workflows/release-supply-chain-smoke.yml)
''',
    '''              Cargo.toml|Cargo.lock|scripts/bootstrap.sh|scripts/bootstrap.ps1|scripts/install.sh|scripts/install.ps1|.github/workflows/release-build.yml|.github/workflows/release-draft.yml|.github/workflows/release-supply-chain-smoke.yml)
''',
)

Path("CONTRIBUTING.md").write_text("""# Contributing to Sippion

Thank you for helping improve Sippion. Contributions should preserve the core product goal: give AI coding agents a small, relevant, trustworthy repository context before they broadly open source files.

## Design invariants

Changes must preserve these defaults unless the pull request explicitly proposes a reviewed change to the trust model:

- repository-context serving is local, read-only, and no-network;
- repository code, build scripts, compilers, LSP servers, and shell commands are not executed during retrieval;
- filesystem authority comes from the configured project root, not model-supplied paths;
- source reads, parsing, concurrency, scan size, and model-visible output stay bounded;
- source text is untrusted data and high-confidence secrets are redacted before model output;
- persistent repository indexes or cross-process source caches are not introduced implicitly.

See `docs/security.md`, `docs/architecture.md`, and `docs/integrations.md` before changing retrieval or trust-boundary code.

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

## Pull requests

Keep pull requests focused and explain the user or agent behavior being improved, trust-boundary/resource-budget impact, tests added, and release/distribution impact when applicable.

Distribution-path changes may require bootstrap or release supply-chain smoke workflows in addition to normal CI. Trusted-author automation only merges an exact tested head after all path-applicable gates succeed.

## Releases

Do not create release tags manually as part of ordinary development. The release workflow builds and tests all supported platform artifacts, creates checksums and attestations, generates the SBOM, materializes the tag only after validation, publishes the release, and performs post-publication verification.

## Security reports

Do not include exploit details, credentials, or unredacted secrets in a public issue. Follow `SECURITY.md` for private-reporting guidance.
""")

Path("CHANGELOG.md").write_text("""# Changelog

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
""")

Path("SECURITY.md").write_text("""# Security Policy

Sippion treats repository contents as untrusted data and is designed to remain local, read-only, no-network, and project-scoped while serving repository context. The full trust model, filesystem policy, redaction behavior, installer provenance model, and known boundaries are documented in [`docs/security.md`](docs/security.md).

## Supported versions

Before 1.0, security fixes are supported on the latest published release candidate and on `main`. Older release candidates may not receive backports. After a fix is released, users should upgrade to the newest release rather than relying on an older RC.

## Reporting a vulnerability

Please do **not** disclose exploit details, credentials, private repository contents, or unredacted secrets in a public issue or discussion.

1. Use GitHub's private vulnerability-reporting flow for this repository when it is available from the repository **Security** tab.
2. If that private flow is unavailable, open a minimal public issue asking the maintainer to establish a private contact channel. Do not include vulnerability details in that issue.
3. Include the affected Sippion version/commit, platform, impact, minimal reproduction information, and whether the issue crosses a documented trust boundary.

Reports involving installer/release integrity should include the release tag, asset name, expected/observed SHA-256 digest, and attestation-verification result when available.

## Scope priorities

High-priority reports include unintended writes, repository-code execution during retrieval, network access while serving context, root/symlink/hard-link boundary escapes, secret-redaction bypasses with realistic credential material, release provenance/checksum bypasses, and denial-of-service inputs that escape documented resource bounds.

A parser or semantic ranking inaccuracy alone is generally a correctness issue rather than a security issue unless it creates a trust-boundary bypass or exposes data outside the authorized repository scope.
""")
