use std::collections::HashSet;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser, Point};

use crate::hybrid::Symbol;

#[must_use]
pub fn supports_tree_sitter_path(path: &str) -> bool {
    language_for_path(path).is_some()
}

fn named_child<'tree>(node: &Node<'tree>, index: usize) -> Option<Node<'tree>> {
    node.named_child(u32::try_from(index).ok()?)
}

fn language_for_path(path: &str) -> Option<Language> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" | "pyi" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" | "mjs" | "cjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" | "mts" | "cts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "cs" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "ipp" | "tpp" => {
            Some(tree_sitter_cpp::LANGUAGE.into())
        }
        _ => None,
    }
}

fn declaration_kind(node: &Node<'_>) -> Option<&'static str> {
    match node.kind() {
        // Rust
        "function_item" => Some("function"),
        "struct_item" => Some("struct"),
        "enum_item" => Some("enum"),
        "trait_item" => Some("trait"),
        "type_item" => Some("type"),
        "const_item" | "static_item" => Some("constant"),
        "mod_item" => Some("module"),
        "macro_definition" => Some("macro"),
        // Python
        "function_definition" => Some("function"),
        "class_definition" => Some("class"),
        // JavaScript / TypeScript / Go (shared node names where applicable)
        "function_declaration"
        | "function_signature"
        | "generator_function_declaration"
        | "method_definition"
        | "method_signature"
        | "abstract_method_signature"
        | "method_declaration" => Some("function"),
        "class_declaration" => Some("class"),
        "interface_declaration" => Some("interface"),
        "type_alias_declaration" | "type_spec" => Some("type"),
        "enum_declaration" => Some("enum"),
        // Java / C# / C / C++ declarations not covered by the shared names above.
        "constructor_declaration"
        | "local_function_statement"
        | "function_definition"
        | "function_declarator" => Some("function"),
        "struct_declaration" | "struct_specifier" => Some("struct"),
        "class_specifier" | "record_declaration" => Some("class"),
        "union_specifier" => Some("union"),
        "enum_specifier" => Some("enum"),
        "annotation_type_declaration" => Some("interface"),
        "delegate_declaration" | "type_definition" => Some("type"),
        "namespace_definition" => Some("module"),
        // Common JavaScript/TypeScript style: `const authenticate = (...) => ...`.
        "variable_declarator" => node
            .child_by_field_name("value")
            .filter(|value| {
                matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "generator_function"
                )
            })
            .map(|_| "function"),
        _ => None,
    }
}

fn is_identifier_like(node: Node<'_>) -> bool {
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

const AST_PARSE_FILE_BUDGET: Duration = Duration::from_millis(500);
const AST_TRAVERSAL_CHECK_INTERVAL: usize = 256;
const MAX_AST_VISITED_NODES: usize = 500_000;

fn traversal_must_abort(
    visited_nodes: &mut usize,
    cancellation: Option<&AtomicBool>,
    deadline: Instant,
) -> bool {
    *visited_nodes = visited_nodes.saturating_add(1);
    if *visited_nodes > MAX_AST_VISITED_NODES {
        return true;
    }
    if *visited_nodes % AST_TRAVERSAL_CHECK_INTERVAL != 0 {
        return false;
    }
    cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) || Instant::now() >= deadline
}

/// Parse only already-ranked candidate files. Unsupported extensions return `None`, allowing the
/// caller to fall back to the dependency-free heuristic extractor. Parsing is abortable so a
/// pathological supported file cannot bypass the structural stage wall-clock/cancellation guard.
#[must_use]
pub fn extract_ast_symbols_bounded(
    path: &str,
    text: &str,
    max_symbols: usize,
    cancellation: Option<&AtomicBool>,
    overall_deadline: Option<Instant>,
) -> Option<Vec<Symbol>> {
    let language = language_for_path(path)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;

    let file_deadline = Instant::now() + AST_PARSE_FILE_BUDGET;
    let deadline = overall_deadline
        .map(|overall| overall.min(file_deadline))
        .unwrap_or(file_deadline);
    let bytes = text.as_bytes();
    let mut input = |byte: usize, _position: Point| -> &[u8] { bytes.get(byte..).unwrap_or(&[]) };
    let mut progress = |_state: &ParseState| -> ControlFlow<()> {
        if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed))
            || Instant::now() >= deadline
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(&mut input, None, Some(options))?;
    if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) || Instant::now() >= deadline {
        return None;
    }

    let lines = text.lines().collect::<Vec<_>>();
    let mut stack = vec![tree.root_node()];
    let mut symbols = Vec::new();
    let mut seen = HashSet::new();
    let mut visited_nodes = 0usize;

    while let Some(node) = stack.pop() {
        if traversal_must_abort(&mut visited_nodes, cancellation, deadline) {
            return None;
        }
        if let Some(kind) = declaration_kind(&node) {
            if let Some(name_node) = identifier_child(node) {
                if let Ok(name) = name_node.utf8_text(text.as_bytes()) {
                    let name = name.trim();
                    if !name.is_empty() && seen.insert(name.to_string()) {
                        let row = node.start_position().row;
                        let signature = lines
                            .get(row)
                            .map(|line| line.trim_start().chars().take(220).collect::<String>())
                            .unwrap_or_default();
                        symbols.push(Symbol {
                            name: name.to_string(),
                            kind: kind.to_string(),
                            line: (row + 1) as u32,
                            signature,
                        });
                        if symbols.len() >= max_symbols {
                            break;
                        }
                    }
                }
            }
        }

        // Reverse push preserves source order during depth-first traversal.
        for index in (0..node.named_child_count()).rev() {
            if let Some(child) = named_child(&node, index) {
                stack.push(child);
            }
        }
    }

    Some(symbols)
}

#[cfg(test)]
#[must_use]
fn extract_ast_symbols(path: &str, text: &str, max_symbols: usize) -> Option<Vec<Symbol>> {
    extract_ast_symbols_bounded(path, text, max_symbols, None, None)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticReference {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SemanticFacts {
    /// Exact identifier references observed in the syntax tree. These are stronger than raw
    /// substring matches but deliberately stop short of compiler/LSP type inference.
    pub references: Vec<SemanticReference>,
    /// Module/package paths extracted from import/use/from/require forms without executing code.
    pub import_paths: Vec<String>,
}

fn semantic_identifier_kind(node: Node<'_>) -> &'static str {
    let mut parent = node.parent();
    for _ in 0..3 {
        let Some(ancestor) = parent else {
            break;
        };
        if matches!(
            ancestor.kind(),
            "impl_item"
                | "implements_clause"
                | "extends_clause"
                | "superclass"
                | "super_interfaces"
                | "extends_interfaces"
                | "base_list"
                | "type_list"
                | "trait_bounds"
        ) {
            return "implementation";
        }
        parent = ancestor.parent();
    }

    let mut parent = node.parent();
    for _ in 0..2 {
        let Some(ancestor) = parent else {
            break;
        };
        if matches!(
            ancestor.kind(),
            "call_expression"
                | "call"
                | "macro_invocation"
                | "await_expression"
                | "method_invocation"
                | "invocation_expression"
                | "object_creation_expression"
        ) {
            return "call";
        }
        parent = ancestor.parent();
    }

    if node.kind() == "type_identifier" {
        return "type";
    }
    if node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            "generic_type"
                | "type_annotation"
                | "type_arguments"
                | "type_parameter"
                | "reference_type"
                | "pointer_type"
                | "slice_type"
                | "array_type"
                | "type_list"
                | "base_list"
                | "superclass"
                | "object_creation_expression"
        )
    }) {
        return "type";
    }
    "reference"
}

fn quoted_fragment(line: &str) -> Option<&str> {
    let single = line.find('\'');
    let double = line.find('"');
    let start = match (single, double) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let quote = line.as_bytes()[start];
    let tail = &line[start + 1..];
    let end = tail.as_bytes().iter().position(|byte| *byte == quote)?;
    Some(&tail[..end])
}

fn include_fragment(line: &str) -> Option<&str> {
    quoted_fragment(line).or_else(|| {
        let start = line.find('<')?;
        let tail = &line[start + 1..];
        let end = tail.find('>')?;
        Some(&tail[..end])
    })
}

fn import_paths_from_source_bounded(
    path: &str,
    text: &str,
    max_imports: usize,
    cancellation: Option<&AtomicBool>,
    deadline: Instant,
) -> Option<Vec<String>> {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut imports = Vec::new();
    let mut seen = HashSet::new();
    for (line_index, line) in text.lines().enumerate() {
        if imports.len() >= max_imports {
            break;
        }
        if line_index % AST_TRAVERSAL_CHECK_INTERVAL == 0
            && (cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed))
                || Instant::now() >= deadline)
        {
            return None;
        }
        let trimmed = line.trim();
        let candidate = match extension.as_str() {
            "rs" => {
                if let Some(rest) = trimmed.strip_prefix("use ") {
                    Some(
                        rest.trim_end_matches(';')
                            .split("::{")
                            .next()
                            .unwrap_or(rest)
                            .trim(),
                    )
                } else {
                    trimmed
                        .strip_prefix("mod ")
                        .map(|rest| rest.trim_end_matches(';').trim())
                }
            }
            "py" | "pyi" => {
                if let Some(rest) = trimmed.strip_prefix("from ") {
                    rest.split_whitespace().next()
                } else if let Some(rest) = trimmed.strip_prefix("import ") {
                    rest.split(|ch: char| ch == ',' || ch.is_whitespace())
                        .next()
                } else {
                    None
                }
            }
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => {
                if trimmed.starts_with("import ") || trimmed.contains("require(") {
                    quoted_fragment(trimmed)
                } else {
                    None
                }
            }
            "go" => {
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
                    let rest = rest
                        .trim_start_matches("static ")
                        .trim_end_matches(';')
                        .trim();
                    rest.split_once('=')
                        .map_or(rest, |(_, target)| target.trim())
                })
            }
            "c" | "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "ipp" | "tpp" => {
                trimmed
                    .strip_prefix("#include")
                    .and_then(|rest| include_fragment(rest.trim()))
            }
            _ => None,
        };
        if let Some(candidate) = candidate {
            let normalized = candidate
                .trim_matches(|ch: char| ch == '"' || ch == '\'' || ch == '`')
                .replace("::", "/");
            let normalized = if matches!(
                extension.as_str(),
                "c" | "cc" | "cpp" | "cxx" | "c++" | "h" | "hh" | "hpp" | "hxx" | "ipp" | "tpp"
            ) {
                normalized
            } else {
                normalized.replace('.', "/")
            };
            if !normalized.is_empty() && seen.insert(normalized.clone()) {
                imports.push(normalized);
            }
        }
    }
    Some(imports)
}

/// Source-only semantic extraction for already-ranked candidate files. It identifies exact AST
/// references, call/type/implementation contexts, and import paths without invoking a compiler,
/// language server, build script, procedural macro, or repository code.
#[must_use]
pub fn extract_semantic_facts_bounded(
    path: &str,
    text: &str,
    max_references: usize,
    max_imports: usize,
    cancellation: Option<&AtomicBool>,
    overall_deadline: Option<Instant>,
) -> Option<SemanticFacts> {
    let language = language_for_path(path)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;

    let file_deadline = Instant::now() + AST_PARSE_FILE_BUDGET;
    let deadline = overall_deadline
        .map(|overall| overall.min(file_deadline))
        .unwrap_or(file_deadline);
    let bytes = text.as_bytes();
    let mut input = |byte: usize, _position: Point| -> &[u8] { bytes.get(byte..).unwrap_or(&[]) };
    let mut progress = |_state: &ParseState| -> ControlFlow<()> {
        if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed))
            || Instant::now() >= deadline
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    let tree = parser.parse_with_options(&mut input, None, Some(options))?;
    if cancellation.is_some_and(|flag| flag.load(Ordering::Relaxed)) || Instant::now() >= deadline {
        return None;
    }

    let mut definition_ranges = HashSet::new();
    let mut stack = vec![tree.root_node()];
    let mut visited_nodes = 0usize;
    while let Some(node) = stack.pop() {
        if traversal_must_abort(&mut visited_nodes, cancellation, deadline) {
            return None;
        }
        if declaration_kind(&node).is_some() {
            if let Some(name_node) = identifier_child(node) {
                definition_ranges.insert((name_node.start_byte(), name_node.end_byte()));
            }
        }
        for index in (0..node.named_child_count()).rev() {
            if let Some(child) = named_child(&node, index) {
                stack.push(child);
            }
        }
    }

    let mut references = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if references.len() >= max_references {
            break;
        }
        if traversal_must_abort(&mut visited_nodes, cancellation, deadline) {
            return None;
        }
        if matches!(
            node.kind(),
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "namespace_identifier"
                | "operator_name"
        ) && !definition_ranges.contains(&(node.start_byte(), node.end_byte()))
        {
            if let Ok(name) = node.utf8_text(bytes) {
                let name = name.trim();
                if name.len() >= 2 {
                    let kind = semantic_identifier_kind(node);
                    let key = (
                        name.to_string(),
                        kind.to_string(),
                        node.start_position().row,
                    );
                    if seen.insert(key) {
                        references.push(SemanticReference {
                            name: name.to_string(),
                            kind: kind.to_string(),
                        });
                    }
                }
            }
        }
        for index in (0..node.named_child_count()).rev() {
            if let Some(child) = named_child(&node, index) {
                stack.push(child);
            }
        }
    }

    let import_paths =
        import_paths_from_source_bounded(path, text, max_imports, cancellation, deadline)?;
    Some(SemanticFacts {
        references,
        import_paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_traversal_node_budget_is_fail_closed() {
        let mut visited = MAX_AST_VISITED_NODES;
        assert!(traversal_must_abort(
            &mut visited,
            None,
            Instant::now() + Duration::from_secs(1),
        ));
    }

    #[test]
    fn rust_ast_extracts_real_declarations() {
        let source = "pub(crate) fn authenticate() {}\npub struct Session { value: u8 }\n";
        let symbols = extract_ast_symbols("src/auth.rs", source, 8).expect("rust grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "authenticate"));
        assert!(symbols.iter().any(|symbol| symbol.name == "Session"));
    }

    #[test]
    fn javascript_ast_extracts_arrow_function_variable() {
        let source = "const authenticate = (token) => token.length > 0;\n";
        let symbols = extract_ast_symbols("src/auth.js", source, 8).expect("javascript grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "authenticate"));
    }

    #[test]
    fn java_ast_extracts_class_and_method() {
        let source =
            "class AuthService { boolean validate(String token) { return !token.isEmpty(); } }
";
        let symbols = extract_ast_symbols("src/AuthService.java", source, 8).expect("java grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "AuthService"));
        assert!(symbols.iter().any(|symbol| symbol.name == "validate"));
    }

    #[test]
    fn csharp_ast_extracts_class_and_method() {
        let source =
            "class AuthService { bool Validate(string token) { return token.Length > 0; } }
";
        let symbols = extract_ast_symbols("src/AuthService.cs", source, 8).expect("csharp grammar");
        assert!(symbols.iter().any(|symbol| symbol.name == "AuthService"));
        assert!(symbols.iter().any(|symbol| symbol.name == "Validate"));
    }

    #[test]
    fn c_and_cpp_ast_extract_functions_and_types() {
        let c_source =
            "struct Session { int value; }; int validate_token(int token) { return token > 0; }
";
        let c_symbols = extract_ast_symbols("src/auth.c", c_source, 8).expect("c grammar");
        assert!(c_symbols.iter().any(|symbol| symbol.name == "Session"));
        assert!(
            c_symbols
                .iter()
                .any(|symbol| symbol.name == "validate_token")
        );

        let cpp_source =
            "class Validator { public: bool validate(int token) { return token > 0; } };
";
        let cpp_symbols = extract_ast_symbols("src/auth.hpp", cpp_source, 8).expect("cpp grammar");
        assert!(cpp_symbols.iter().any(|symbol| symbol.name == "Validator"));
        assert!(cpp_symbols.iter().any(|symbol| symbol.name == "validate"));
    }

    #[test]
    fn unsupported_language_uses_caller_fallback() {
        assert!(extract_ast_symbols("notes.txt", "function nope() {}", 8).is_none());
    }
}

#[cfg(test)]
mod semantic_tests {
    use super::*;

    #[test]
    fn rust_semantics_extract_call_type_and_import_without_execution() {
        let source = "use crate::auth::Validator;\nfn login(v: Validator) { validate_token(v); }\n";
        let facts = extract_semantic_facts_bounded("src/login.rs", source, 64, 16, None, None)
            .expect("rust grammar");
        assert!(
            facts
                .import_paths
                .iter()
                .any(|path| path.contains("crate/auth/Validator"))
        );
        assert!(
            facts
                .references
                .iter()
                .any(|reference| reference.name == "Validator")
        );
        assert!(
            facts.references.iter().any(|reference| {
                reference.name == "validate_token" && reference.kind == "call"
            })
        );
    }

    #[test]
    fn typescript_semantics_extract_module_path() {
        let source = "import { validate } from './auth/session';\nvalidate(token);\n";
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
        let java = "import com.example.Auth;\nclass Login { void run() { validate(token); } }
";
        let java_facts = extract_semantic_facts_bounded("src/Login.java", java, 64, 16, None, None)
            .expect("java grammar");
        assert!(
            java_facts
                .import_paths
                .iter()
                .any(|path| path == "com/example/Auth")
        );
        assert!(
            java_facts
                .references
                .iter()
                .any(|reference| reference.name == "validate" && reference.kind == "call")
        );

        let csharp = "using Acme.Auth;\nclass Login { void Run() { Validate(token); } }
";
        let csharp_facts =
            extract_semantic_facts_bounded("src/Login.cs", csharp, 64, 16, None, None)
                .expect("csharp grammar");
        assert!(
            csharp_facts
                .import_paths
                .iter()
                .any(|path| path == "Acme/Auth")
        );
        assert!(
            csharp_facts
                .references
                .iter()
                .any(|reference| reference.name == "Validate" && reference.kind == "call")
        );
    }

    #[test]
    fn c_family_semantics_extract_include_and_call() {
        let source = r#"#include "auth/session.h"
int login(void) { return validate_token(); }
"#;
        let facts = extract_semantic_facts_bounded("src/login.cpp", source, 64, 16, None, None)
            .expect("cpp grammar");
        assert!(
            facts
                .import_paths
                .iter()
                .any(|path| path == "auth/session.h")
        );
        assert!(
            facts
                .references
                .iter()
                .any(|reference| reference.name == "validate_token" && reference.kind == "call")
        );
    }
}
