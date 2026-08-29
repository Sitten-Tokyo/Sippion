#!/usr/bin/env python3
from pathlib import Path
import re

def replace_once(text, old, new, label):
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, got {count}")
    return text.replace(old, new, 1)

def write(path, text):
    Path(path).write_text(text, encoding="utf-8")

# 1) Identifier-aware matching shared by the context packer.
core_path = Path("src/core.rs")
core = core_path.read_text(encoding="utf-8")
split_block = '''pub(crate) fn split_search_tokens(folded_text: &str) -> impl Iterator<Item = &str> {
    folded_text
        .split(|ch: char| !is_search_token_char(ch))
        .filter(|part| {
            part.chars()
                .filter(|ch| ch.is_alphanumeric())
                .take(2)
                .count()
                >= 2
        })
}
'''
identifier_helper = split_block + r'''
/// Counts query terms that match identifier components, rather than arbitrary substrings.
///
/// Whole identifiers and delimiter-free compound forms are retained so exact symbol queries such
/// as `source_content_fingerprint` and `AuthToken` still match. Component matching additionally
/// understands snake/kebab case and lower->upper/acronym CamelCase boundaries, while avoiding
/// false positives such as `auth` in `author` or `map` in `bitmap`.
#[must_use]
pub(crate) fn identifier_query_match_count(text: &str, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 0;
    }

    let mut forms = Vec::<String>::new();
    for identifier in text
        .split(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '$')))
        .filter(|identifier| !identifier.is_empty())
    {
        let whole = unicode_search_fold(identifier);
        if !whole.is_empty() {
            forms.push(whole.clone());
        }
        let joined = whole
            .chars()
            .filter(|ch| !matches!(ch, '_' | '-' | '$'))
            .collect::<String>();
        if !joined.is_empty() && joined != whole {
            forms.push(joined);
        }

        let chars = identifier.chars().collect::<Vec<_>>();
        let mut component = String::new();
        for (index, ch) in chars.iter().copied().enumerate() {
            if matches!(ch, '_' | '-' | '$') {
                if !component.is_empty() {
                    forms.push(unicode_search_fold(&component));
                    component.clear();
                }
                continue;
            }
            let boundary = if component.is_empty() {
                false
            } else {
                let previous = chars[index - 1];
                let next = chars.get(index + 1).copied();
                ((previous.is_lowercase() || previous.is_numeric()) && ch.is_uppercase())
                    || (previous.is_uppercase()
                        && ch.is_uppercase()
                        && next.is_some_and(|next| next.is_lowercase()))
            };
            if boundary {
                forms.push(unicode_search_fold(&component));
                component.clear();
            }
            component.push(ch);
        }
        if !component.is_empty() {
            forms.push(unicode_search_fold(&component));
        }
    }
    forms.sort();
    forms.dedup();
    terms
        .iter()
        .filter(|term| forms.iter().any(|form| form == *term))
        .count()
}
'''
core = replace_once(core, split_block, identifier_helper, "identifier helper insertion")
test_anchor = '''    #[test]
    fn coordination_ids_are_bounded_and_sanitized() {
'''
identifier_test = r'''    #[test]
    fn identifier_query_matching_respects_component_boundaries() {
        let query = vec!["auth".to_string(), "token".to_string()];
        assert_eq!(
            identifier_query_match_count("fn AuthToken() {}", &query),
            2
        );
        assert_eq!(
            identifier_query_match_count("fn auth_token() {}", &query),
            2
        );
        assert_eq!(
            identifier_query_match_count("fn AuthorTokenizer() {}", &query),
            0
        );
        assert_eq!(
            identifier_query_match_count("let bitmap = author;", &vec!["map".into(), "auth".into()]),
            0
        );
        assert_eq!(
            identifier_query_match_count("AuthToken", &vec!["authtoken".into()]),
            1
        );
    }

    #[test]
    fn coordination_ids_are_bounded_and_sanitized() {
'''
core = replace_once(core, test_anchor, identifier_test, "identifier helper test")
write(core_path, core)

# 2) Typed packing diagnostics.
service_path = Path("src/service.rs")
service = service_path.read_text(encoding="utf-8")
ranked_struct = '''#[derive(Debug, Clone, serde::Serialize)]
pub struct RankedFileDiagnostic {
    pub path: String,
    pub rank: f64,
}

'''
diag_struct = ranked_struct + '''#[derive(Debug, Clone, serde::Serialize)]
pub struct PackedAtomDiagnostic {
    pub kind: String,
    pub path: String,
    pub utility: f64,
    pub token_cost: usize,
    pub utility_per_token: f64,
    pub query_symbol_matches: usize,
    pub selected: bool,
    pub reason: String,
}

'''
service = replace_once(service, ranked_struct, diag_struct, "packed atom diagnostic struct")
service = replace_once(
    service,
    '''    /// Unique paths that actually survived the model-visible context packer.
    pub packed_paths: Vec<String>,
''',
    '''    /// Unique paths that actually survived the model-visible context packer.
    pub packed_paths: Vec<String>,
    /// CLI-only atom decisions. Never rendered into model-visible context.
    pub packed_atoms: Vec<PackedAtomDiagnostic>,
''',
    "context diagnostics atom field",
)
service = replace_once(
    service,
    '''        assert_eq!(result.diagnostics.returned_bytes, result.model_text.len());

        drop(service);
''',
    '''        assert_eq!(result.diagnostics.returned_bytes, result.model_text.len());
        assert!(!result.diagnostics.packed_atoms.is_empty());
        assert!(
            result
                .diagnostics
                .packed_atoms
                .iter()
                .any(|atom| atom.selected && !atom.reason.is_empty())
        );

        drop(service);
''',
    "typed diagnostics test",
)
write(service_path, service)

context_path = Path("src/service/context.rs")
context = context_path.read_text(encoding="utf-8")
context = replace_once(
    context,
    "use std::collections::HashSet;",
    "use std::collections::{HashMap, HashSet};",
    "context collections import",
)
context = replace_once(
    context,
    '''use crate::core::{
    NormalizedQuery, RenderExcerpt, adaptive_context_budget, heuristic_v3_estimated_tokens,
    truncate_utf8_prefix,
};''',
    '''use crate::core::{
    NormalizedQuery, RenderExcerpt, adaptive_context_budget, heuristic_v3_estimated_tokens,
    identifier_query_match_count, truncate_utf8_prefix,
};''',
    "context identifier import",
)
context = replace_once(
    context,
    '''    pub(super) packed_paths: Vec<String>,
    pub(super) target_estimated_tokens: usize,
''',
    '''    pub(super) packed_paths: Vec<String>,
    pub(super) atom_diagnostics: Vec<super::PackedAtomDiagnostic>,
    pub(super) target_estimated_tokens: usize,
''',
    "packed context diagnostics field",
)
old_symbol_logic = '''    let mut visible_symbol_text = String::new();
    for symbol in entry.symbols.iter().take(3) {
        text.push_str(&format!(
            "  {} {}:{} {}\\n",
            symbol.kind,
            escaped(&symbol.name),
            symbol.line,
            escaped(symbol.signature.trim())
        ));
        visible_symbol_text.push_str(&symbol.name);
        visible_symbol_text.push(' ');
        visible_symbol_text.push_str(&symbol.signature);
        visible_symbol_text.push(' ');
    }
    let folded_symbols = crate::core::unicode_search_fold(&visible_symbol_text);
    let query_symbol_matches = query
        .terms
        .iter()
        .filter(|term| folded_symbols.contains(term.as_str()))
        .count();
'''
new_symbol_logic = '''    let mut query_symbol_matches = 0usize;
    for symbol in entry.symbols.iter().take(3) {
        text.push_str(&format!(
            "  {} {}:{} {}\\n",
            symbol.kind,
            escaped(&symbol.name),
            symbol.line,
            escaped(symbol.signature.trim())
        ));
        let symbol_text = format!("{} {}", symbol.name, symbol.signature);
        query_symbol_matches = query_symbol_matches.max(identifier_query_match_count(
            &symbol_text,
            &query.terms,
        ));
    }
'''
context = replace_once(context, old_symbol_logic, new_symbol_logic, "identifier-aware reservation")
context = replace_once(
    context,
    '''    let mut selected = HashSet::new();
    let mut selected_paths = HashSet::new();
    let mut order = Vec::new();
''',
    '''    let atom_token_budget = remaining_tokens;
    let atom_byte_budget = remaining_bytes;
    let mut selected = HashSet::new();
    let mut selected_paths = HashSet::new();
    let mut selected_reasons = HashMap::<usize, &'static str>::new();
    let mut order = Vec::new();
''',
    "packing reason state",
)
context = replace_once(
    context,
    '''        selected.insert(index);
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    // Preserve one query-relevant''',
    '''        selected.insert(index);
        selected_reasons.insert(index, "reserved-strongest-evidence");
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    // Preserve one query-relevant''',
    "evidence selection reason",
)
context = replace_once(
    context,
    '''            selected.insert(index);
            selected_paths.insert(atom.path.clone());
            remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
            remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
            order.push(index);
        }
    }

    while order.len()''',
    '''            selected.insert(index);
            selected_reasons.insert(index, "reserved-query-identifier");
            selected_paths.insert(atom.path.clone());
            remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
            remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
            order.push(index);
        }
    }

    while order.len()''',
    "structure selection reason",
)
context = replace_once(
    context,
    '''        selected.insert(index);
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    let mut packed_paths''',
    '''        selected.insert(index);
        selected_reasons.insert(index, "utility-per-token");
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    let atom_limit_reached = selected.len() >= MAX_PACKED_ATOMS;
    let atom_diagnostics = atoms
        .iter()
        .enumerate()
        .map(|(index, atom)| {
            let selected_atom = selected.contains(&index);
            let reason = selected_reasons.get(&index).copied().unwrap_or_else(|| {
                if atom.token_cost > atom_token_budget || atom.text.len() > atom_byte_budget {
                    "cannot-fit-budget"
                } else if atom_limit_reached {
                    "atom-limit-or-lower-utility"
                } else {
                    "lower-marginal-utility"
                }
            });
            super::PackedAtomDiagnostic {
                kind: match atom.kind {
                    ContextAtomKind::Structure => "structure",
                    ContextAtomKind::Evidence => "evidence",
                }
                .to_string(),
                path: atom.path.clone(),
                utility: atom.utility,
                token_cost: atom.token_cost,
                utility_per_token: atom.utility / atom.token_cost.max(1) as f64,
                query_symbol_matches: atom.query_symbol_matches,
                selected: selected_atom,
                reason: reason.to_string(),
            }
        })
        .collect::<Vec<_>>();

    let mut packed_paths''',
    "greedy selection diagnostics",
)
context = replace_once(
    context,
    '''    PackedContext {
        text: output,
        packed_paths,
        target_estimated_tokens: budget.target_estimated_tokens,
''',
    '''    PackedContext {
        text: output,
        packed_paths,
        atom_diagnostics,
        target_estimated_tokens: budget.target_estimated_tokens,
''',
    "packed diagnostics return",
)
context_test_anchor = '''    #[test]
    fn novelty_discount_ablation_prefers_new_path_over_redundant_same_path() {
'''
context_test = r'''    #[test]
    fn identifier_aware_structure_matching_avoids_substring_false_positives() {
        let query = McpToolInput {
            q: "auth token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query");
        let false_positive = RepoMapEntry {
            relative_path: "src/noise.rs".into(),
            score: 1.0,
            symbols: vec![RepoMapSymbol {
                name: "AuthorTokenizer".into(),
                kind: "function".into(),
                line: 1,
                signature: "fn AuthorTokenizer()".into(),
            }],
            links_to: Vec::new(),
            semantic_links: Vec::new(),
        };
        let exact = RepoMapEntry {
            relative_path: "src/auth.rs".into(),
            score: 1.0,
            symbols: vec![RepoMapSymbol {
                name: "AuthToken".into(),
                kind: "function".into(),
                line: 1,
                signature: "fn AuthToken()".into(),
            }],
            links_to: Vec::new(),
            semantic_links: Vec::new(),
        };
        assert_eq!(
            structure_atom(&query, &false_positive, DEFAULT_PACKER_WEIGHTS).query_symbol_matches,
            0
        );
        assert_eq!(
            structure_atom(&query, &exact, DEFAULT_PACKER_WEIGHTS).query_symbol_matches,
            2
        );
    }

    #[test]
    fn novelty_discount_ablation_prefers_new_path_over_redundant_same_path() {
'''
context = replace_once(context, context_test_anchor, context_test, "context identifier test")
write(context_path, context)

engine_path = Path("src/service/engine.rs")
engine = engine_path.read_text(encoding="utf-8")
engine = replace_once(
    engine,
    '''            ranked_files,
            packed_paths: packed.packed_paths,
        };''',
    '''            ranked_files,
            packed_paths: packed.packed_paths,
            packed_atoms: packed.atom_diagnostics,
        };''',
    "engine packed atom diagnostics",
)
write(engine_path, engine)

# 3) Exact-content graph cache identity, including case-only rewrites.
repo_path = Path("src/repo.rs")
repo = repo_path.read_text(encoding="utf-8")
repo = replace_once(
    repo,
    '''struct GraphCacheNode {
    path: String,
    stamp: SourceStamp,
}''',
    '''struct GraphCacheNode {
    path: String,
    stamp: SourceStamp,
    fingerprint: (u64, u64),
}''',
    "graph cache fingerprint field",
)
repo = replace_once(
    repo,
    '''struct MapCandidate {
    relative_path: String,
    stamp: SourceStamp,
    search_score: f64,
    source_lower: String,
''',
    '''struct MapCandidate {
    relative_path: String,
    stamp: SourceStamp,
    content_fingerprint: (u64, u64),
    search_score: f64,
    source_lower: String,
''',
    "map candidate fingerprint field",
)
write(repo_path, repo)

map_path = Path("src/repo/map.rs")
map_text = map_path.read_text(encoding="utf-8")
old_graph = '''                .map(|candidate| GraphCacheNode {
                    // Graph reuse is content-keyed as well as stamp-keyed. This is required on
                    // Windows, where an in-place same-size/same-mtime rewrite can preserve the
                    // metadata identity visible to the stable API.
                    path: content_keyed_analysis_path(
                        &candidate.relative_path,
                        &candidate.source_lower,
                    ),
                    stamp: candidate.stamp.clone(),
                })
'''
new_graph = '''                .map(|candidate| GraphCacheNode {
                    // Graph reuse is keyed by the exact redacted source fingerprint, not by the
                    // case-folded graph scan buffer. Case-only rewrites must therefore invalidate
                    // the graph even on Windows when size and mtime remain unchanged.
                    path: candidate.relative_path.clone(),
                    stamp: candidate.stamp.clone(),
                    fingerprint: candidate.content_fingerprint,
                })
'''
map_text = replace_once(map_text, old_graph, new_graph, "graph cache exact fingerprint")
map_text = replace_once(
    map_text,
    '''        let safe = redaction.text;
        // The source was verified and read before this point. Key structural analysis by a
''',
    '''        let safe = redaction.text;
        let content_fingerprint = source_content_fingerprint(&safe);
        // The source was verified and read before this point. Key structural analysis by a
''',
    "exact fingerprint capture",
)
map_text = replace_once(
    map_text,
    '''                relative_path: path.to_string(),
                stamp: source.stamp,
                search_score,
''',
    '''                relative_path: path.to_string(),
                stamp: source.stamp,
                content_fingerprint,
                search_score,
''',
    "map candidate exact fingerprint",
)
map_text = replace_once(
    map_text,
    '''    fn structural_cache_key_changes_with_content_and_preserves_extension() {
        let first = content_keyed_analysis_path("src/main.rs", "fn first() {}");
        let second = content_keyed_analysis_path("src/main.rs", "fn second() {}");
        assert_ne!(first, second);
        assert!(first.ends_with(".rs"));
        assert!(second.ends_with(".rs"));
    }
''',
    '''    fn structural_cache_key_changes_with_content_and_preserves_extension() {
        let first = content_keyed_analysis_path("src/main.rs", "fn first() {}");
        let second = content_keyed_analysis_path("src/main.rs", "fn second() {}");
        let case_upper = content_keyed_analysis_path("src/main.rs", "fn AuthToken() {}");
        let case_lower = content_keyed_analysis_path("src/main.rs", "fn authtoken() {}");
        assert_ne!(first, second);
        assert_ne!(case_upper, case_lower);
        assert_eq!(
            crate::core::unicode_search_fold("fn AuthToken() {}"),
            crate::core::unicode_search_fold("fn authtoken() {}")
        );
        assert!(first.ends_with(".rs"));
        assert!(second.ends_with(".rs"));
    }
''',
    "case-only cache test",
)
write(map_path, map_text)

# Evaluation/tuning suites are replaced as cohesive tools below.

RETRIEVAL_EVAL = " + repr(retrieval_eval) + "
TUNER = " + repr(tuner) + "
HOLDOUT = " + repr(holdout_text) + "
write("scripts/retrieval-eval.py", RETRIEVAL_EVAL)
write("scripts/tune-context.py", TUNER)
write("eval/self_holdout_cases.json", HOLDOUT)

architecture_path = Path("docs/architecture.md")
architecture = architecture_path.read_text(encoding="utf-8")
architecture_note = r'''

### Evaluation independence and packing decisions

Whole-repository holdout cases are versioned and frozen independently from the training suite.
Required evidence is validated against the actual model-visible atom that owns it (`path`, atom
kind, and anchor), rather than by searching the complete output for an unscoped substring. CI
warms the filesystem once and measures each query three times, using the per-case median before
computing suite p95 latency. The evaluator also reports an independent Python BM25 file-ranking
baseline and requires Sippion holdout Recall@5 not to regress below that baseline.

Local CLI diagnostics expose each candidate context atom's utility, estimated token cost,
utility-per-token, identifier match count, selection state, and selection/rejection reason. These
diagnostics are typed side-channel data and are never added to model-visible repository context.

Identifier-aware structure reservation uses whole-identifier and snake/kebab/CamelCase components;
it does not treat arbitrary substrings such as `auth` in `author` as definition matches. Structural
graph caches are keyed by exact redacted-source fingerprints in addition to file identity, so
case-only same-size/same-mtime rewrites cannot reuse a stale graph.
'''
if "### Evaluation independence and packing decisions" not in architecture:
    architecture = architecture.rstrip() + architecture_note + "\n"
write(architecture_path, architecture)

changelog_path = Path("CHANGELOG.md")
changelog = changelog_path.read_text(encoding="utf-8")
changelog_anchor = "### Changed\n\n"
changelog_bullet = (
    "- Context packing now uses identifier-component-aware structure reservation and exposes typed "
    "CLI-only atom selection diagnostics; evaluation scopes required evidence to its owning path/atom, "
    "uses warmup-plus-median latency measurements, compares an independent BM25 baseline, expands the "
    "frozen whole-repository holdout suite, and explores pairwise train-only tuner candidates before "
    "one post-selection holdout. Graph cache identity now includes the exact redacted-source fingerprint "
    "so case-only rewrites invalidate safely on Windows.\n"
)
if changelog_bullet not in changelog:
    changelog = replace_once(
        changelog,
        changelog_anchor,
        changelog_anchor + changelog_bullet,
        "changelog improvement bullet",
    )
write(changelog_path, changelog)
