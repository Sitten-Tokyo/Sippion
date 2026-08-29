#!/usr/bin/env python3
from pathlib import Path


def one(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    return text.replace(old, new, 1)

hybrid_path = Path('src/hybrid.rs')
hybrid = hybrid_path.read_text(encoding='utf-8')
hybrid = one(
    hybrid,
    '''    for (index, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        for &(marker, kind) in markers {
            if let Some(name) = identifier_after(trimmed, marker) {
''',
    '''    for (index, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        // Treat restricted Rust visibility as declaration metadata, just like structural scoring.
        // Keep the original line as the rendered signature but parse the declaration after pub(...).
        let declaration = if trimmed.starts_with("pub(") {
            trimmed
                .find(") ")
                .map(|end| trimmed[end + 2..].trim_start())
                .unwrap_or(trimmed)
        } else {
            trimmed
        };
        for &(marker, kind) in markers {
            if let Some(name) = identifier_after(declaration, marker) {
''',
    'restricted visibility symbol extraction',
)
hybrid = one(
    hybrid,
    '''    #[test]
    fn compacting_keeps_non_empty_lines() {
''',
    '''    #[test]
    fn symbol_extraction_handles_restricted_rust_visibility() {
        let text = "pub(super) fn late_context_target() {}\\npub(crate) const PROJECT_TABLE: usize = 1;\\n";
        let symbols = extract_symbols(text, 8);
        assert!(symbols.iter().any(|symbol| symbol.name == "late_context_target"));
        assert!(symbols.iter().any(|symbol| symbol.name == "PROJECT_TABLE"));
    }

    #[test]
    fn compacting_keeps_non_empty_lines() {
''',
    'restricted visibility extraction test',
)
hybrid_path.write_text(hybrid, encoding='utf-8')

map_path = Path('src/repo/map.rs')
text = map_path.read_text(encoding='utf-8')
text = one(
    text,
    '''    let mut symbols = analysis
        .symbols
        .iter()
        .map(|symbol| RepoMapSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
            line: symbol.line,
            signature: signature_from_lines(&safe_lines, symbol.line),
        })
        .collect::<Vec<_>>();
''',
    '''    let mut symbols = analysis
        .symbols
        .iter()
        .map(|symbol| RepoMapSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind.clone(),
            line: symbol.line,
            signature: signature_from_lines(&safe_lines, symbol.line),
        })
        .collect::<Vec<_>>();
    // The shared AST cache is intentionally compact and query-independent. Supplement it with a
    // bounded source-wide declaration scan before query ranking so late definitions are not lost
    // merely because they occur after the cache's source-order retention window. This remains
    // bounded (source <= 2 MiB, at most 1024 fallback declarations) and retains no source cross-request.
    for symbol in extract_symbols(safe, 1024) {
        if symbols
            .iter()
            .any(|existing| existing.name == symbol.name && existing.line == symbol.line)
        {
            continue;
        }
        symbols.push(RepoMapSymbol {
            name: symbol.name,
            kind: symbol.kind,
            line: symbol.line,
            signature: symbol.signature,
        });
    }
''',
    'bounded late-definition fallback',
)
map_path.write_text(text, encoding='utf-8')

# Keep the oversize-redaction marker with the module that owns and emits it. The parent still
# imports it through `use redaction::*`, so existing repo-level tests retain access without making
# the frozen evaluation assert an implementation symbol against the wrong source file.
repo_path = Path('src/repo.rs')
repo = repo_path.read_text(encoding='utf-8')
repo = one(
    repo,
    'const REDACTED_OVERSIZE_LINE: &str = "[SIPPION_REDACTED_OVERSIZE_LINE]";\n',
    '',
    'move oversize redaction marker from parent',
)
repo_path.write_text(repo, encoding='utf-8')

redaction_path = Path('src/repo/redaction.rs')
redaction = redaction_path.read_text(encoding='utf-8')
redaction = one(
    redaction,
    'use super::*;\n',
    'use super::*;\n\npub(super) const REDACTED_OVERSIZE_LINE: &str = "[SIPPION_REDACTED_OVERSIZE_LINE]";\n',
    'own oversize redaction marker in redaction module',
)
redaction_path.write_text(redaction, encoding='utf-8')
