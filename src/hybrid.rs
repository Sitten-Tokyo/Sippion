use std::collections::HashSet;

pub const BM25_K1: f64 = 1.2;
pub const BM25_B: f64 = 0.75;

#[must_use]
pub fn term_statistics(text: &str, terms: &[String]) -> (usize, Vec<usize>) {
    let mut frequencies = vec![0usize; terms.len()];
    let mut document_len = 0usize;
    let folded_text = crate::core::unicode_search_fold(text);
    for part in crate::core::split_search_tokens(&folded_text) {
        document_len = document_len.saturating_add(1);
        for (index, term) in terms.iter().enumerate() {
            if part == term {
                frequencies[index] = frequencies[index].saturating_add(1);
            }
        }
    }

    // Code identifiers are often compounds (validate_token, AuthTokenValidator). Preserve the
    // old substring-recall behaviour as a one-hit fallback while BM25 rewards exact token repeats.
    for (index, term) in terms.iter().enumerate() {
        if frequencies[index] == 0 && folded_text.contains(term.as_str()) {
            frequencies[index] = 1;
        }
    }
    (document_len.max(1), frequencies)
}

#[must_use]
pub fn bm25_score(
    frequencies: &[usize],
    document_len: usize,
    average_document_len: f64,
    document_frequencies: &[usize],
    document_count: usize,
) -> f64 {
    if document_count == 0 || average_document_len <= 0.0 {
        return 0.0;
    }
    let n = document_count as f64;
    let dl = document_len as f64;
    frequencies
        .iter()
        .zip(document_frequencies)
        .filter(|(tf, _)| **tf > 0)
        .map(|(tf, df)| {
            let tf = *tf as f64;
            let df = *df as f64;
            // Robertson/Sparck-Jones style positive IDF used by modern BM25 variants.
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            let norm = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / average_document_len);
            idf * (tf * (BM25_K1 + 1.0) / norm)
        })
        .sum()
}

fn strongest_identifier_term_match(line: &str, terms: &[String]) -> usize {
    line.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '$'))
        .filter(|identifier| !identifier.is_empty())
        .map(|identifier| {
            terms
                .iter()
                .filter(|term| identifier.contains(term.as_str()))
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[must_use]
pub fn structural_line_bonus(line: &str, terms: &[String]) -> f64 {
    let trimmed = line.trim_start();
    let lower = crate::core::unicode_search_fold(trimmed);
    if !terms.iter().any(|term| lower.contains(term)) {
        return 0.0;
    }

    // Rust restricted visibility (`pub(super)`, `pub(crate)`, `pub(in path)`) describes the
    // ownership boundary, not a different declaration kind. Strip that prefix only for
    // declaration detection so the same scoring works for ordinary and restricted definitions.
    let definition_source = if lower.starts_with("pub(") {
        lower
            .find(") ")
            .map(|end| lower[end + 2..].trim_start())
            .unwrap_or(lower.as_str())
    } else {
        lower.as_str()
    };

    let definition_markers = [
        "pub async fn ",
        "pub fn ",
        "async fn ",
        "fn ",
        "async def ",
        "def ",
        "export async function ",
        "export function ",
        "function ",
        "func ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "export interface ",
        "interface ",
        "pub class ",
        "export class ",
        "class ",
        "pub type ",
        "type ",
        "pub const ",
        "const ",
        "pub static ",
        "static ",
        "pub mod ",
        "mod ",
    ];
    if let Some(marker) = definition_markers
        .iter()
        .find(|marker| definition_source.starts_with(**marker))
    {
        let rest = definition_source[marker.len()..].trim_start();
        let end = rest
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '$'))
            .unwrap_or(rest.len());
        let identifier = &rest[..end];
        let identifier_matches = terms
            .iter()
            .filter(|term| identifier.contains(term.as_str()))
            .count();
        if identifier_matches == 0 {
            return 6.0;
        }
        // Keep the established single-symbol ownership score, but make a compound definition
        // that owns two or more natural-language query concepts stronger than surrounding prose.
        // This is definition-only; ordinary compound references retain the weaker bonus below.
        let coverage_bonus = if identifier_matches >= 2 {
            // A definition whose identifier owns several query concepts is a strong, local
            // implementation answer even when repository-wide prose repeats those words more
            // often. Keep the bonus bounded and apply it only to declaration ownership.
            42.0 + identifier_matches as f64 * 8.0
        } else {
            10.0
        };
        // Exact symbol queries keep the established ownership floor. Natural-language queries
        // gain additional credit when several query concepts are owned by one identifier.
        return if identifier_matches == terms.len() {
            coverage_bonus.max(14.0)
        } else {
            coverage_bonus
        }
        .min(72.0);
    }

    // A compound identifier reference is weaker than its definition, but stronger than prose
    // that merely repeats the same words. This helps implementation call sites survive broad
    // repository searches without allowing repeated calls to outrank symbol ownership.
    let identifier_matches = strongest_identifier_term_match(&lower, terms);
    if identifier_matches >= 2 {
        return identifier_matches as f64 * 3.0;
    }
    if ["use ", "import ", "from ", "require(", "#include"]
        .iter()
        .any(|marker| lower.starts_with(marker))
    {
        2.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub signature: String,
}

fn identifier_after<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(marker)?.trim_start();
    let end = rest
        .find(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '$'))
        .unwrap_or(rest.len());
    let value = &rest[..end];
    (!value.is_empty()).then_some(value)
}

fn strip_rust_restricted_visibility(line: &str) -> &str {
    line.strip_prefix("pub(")
        .and_then(|rest| rest.split_once(") ").map(|(_, declaration)| declaration))
        .unwrap_or(line)
}

#[must_use]
pub fn extract_symbols(text: &str, max_symbols: usize) -> Vec<Symbol> {
    let markers: &[(&str, &str)] = &[
        ("pub async fn ", "function"),
        ("pub fn ", "function"),
        ("async fn ", "function"),
        ("fn ", "function"),
        ("def ", "function"),
        ("async def ", "function"),
        ("export async function ", "function"),
        ("export function ", "function"),
        ("function ", "function"),
        ("func ", "function"),
        ("pub struct ", "struct"),
        ("struct ", "struct"),
        ("pub enum ", "enum"),
        ("enum ", "enum"),
        ("pub trait ", "trait"),
        ("trait ", "trait"),
        ("interface ", "interface"),
        ("export interface ", "interface"),
        ("pub class ", "class"),
        ("export class ", "class"),
        ("class ", "class"),
        ("type ", "type"),
        ("pub type ", "type"),
        ("const ", "constant"),
        ("pub const ", "constant"),
        ("static ", "constant"),
        ("pub static ", "constant"),
        ("mod ", "module"),
        ("pub mod ", "module"),
    ];

    let mut symbols = Vec::new();
    let mut seen = HashSet::new();
    for (index, raw) in text.lines().enumerate() {
        let trimmed = raw.trim_start();
        let declaration = strip_rust_restricted_visibility(trimmed);
        for &(marker, kind) in markers {
            if let Some(name) = identifier_after(declaration, marker) {
                if seen.insert(name.to_string()) {
                    let signature = trimmed.chars().take(220).collect::<String>();
                    symbols.push(Symbol {
                        name: name.to_string(),
                        kind: kind.to_string(),
                        line: (index + 1) as u32,
                        signature,
                    });
                    if symbols.len() >= max_symbols {
                        return symbols;
                    }
                }
                break;
            }
        }
    }
    symbols
}

fn symbol_identifier_fragments(identifier: &str) -> Vec<String> {
    let mut fragments = Vec::new();
    for coarse in identifier.split(['_', '-']) {
        let chars = coarse.chars().collect::<Vec<_>>();
        if chars.len() < 2 {
            continue;
        }
        let mut start = 0;
        for index in 1..chars.len() {
            let previous = chars[index - 1];
            let current = chars[index];
            let next = chars.get(index + 1).copied();
            let camel_boundary = current.is_uppercase()
                && (previous.is_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_uppercase() && next.is_some_and(char::is_lowercase)));
            if camel_boundary {
                let fragment = chars[start..index].iter().collect::<String>();
                if fragment.chars().count() >= 2 {
                    fragments.push(crate::core::unicode_search_fold(&fragment));
                }
                start = index;
            }
        }
        let fragment = chars[start..].iter().collect::<String>();
        if fragment.chars().count() >= 2 {
            fragments.push(crate::core::unicode_search_fold(&fragment));
        }
    }
    fragments
}

fn symbol_fragment_matches_term(fragment: &str, term: &str) -> bool {
    fragment == term
        || fragment
            .strip_suffix('s')
            .is_some_and(|singular| singular == term)
        || term
            .strip_suffix('s')
            .is_some_and(|singular| singular == fragment)
}

pub(crate) fn symbol_term_match_score(name: &str, signature: &str, term: &str) -> usize {
    let folded_name = crate::core::unicode_search_fold(name);
    if folded_name == term {
        8
    } else if symbol_identifier_fragments(name)
        .iter()
        .any(|fragment| symbol_fragment_matches_term(fragment, term))
    {
        6
    } else if folded_name.contains(term) {
        2
    } else if crate::core::unicode_search_fold(signature).contains(term) {
        1
    } else {
        0
    }
}

/// Weighted PageRank for semantic repository edges. Lexical coincidences can remain weak while
/// AST-backed calls/types/implementations contribute more strongly to centrality. Invalid or
/// non-positive edge weights are ignored.
#[must_use]
pub fn weighted_pagerank(edges: &[Vec<(usize, f64)>], iterations: usize) -> Vec<f64> {
    let n = edges.len();
    if n == 0 {
        return Vec::new();
    }
    let damping = 0.85;
    let mut rank = vec![1.0 / n as f64; n];
    for _ in 0..iterations {
        let mut next = vec![(1.0 - damping) / n as f64; n];
        let mut undistributed = 0.0;
        for (from, targets) in edges.iter().enumerate() {
            let total = targets
                .iter()
                .filter(|(to, weight)| *to < n && weight.is_finite() && *weight > 0.0)
                .map(|(_, weight)| *weight)
                .sum::<f64>();
            if total <= f64::EPSILON {
                undistributed += rank[from];
                continue;
            }

            // Edge strengths below 1.0 deliberately leave part of the damped rank undistributed.
            // This makes a lexical coincidence (0.15) materially weaker than a call edge (0.90).
            let normalization = if total > 1.0 { 1.0 / total } else { 1.0 };
            let active_mass = total.min(1.0);
            for (to, weight) in targets {
                if *to < n && weight.is_finite() && *weight > 0.0 {
                    next[*to] += damping * rank[from] * *weight * normalization;
                }
            }
            undistributed += rank[from] * (1.0 - active_mass);
        }
        if undistributed > 0.0 {
            let share = damping * undistributed / n as f64;
            for value in &mut next {
                *value += share;
            }
        }
        rank = next;
    }
    rank
}

/// RTK-inspired conservative output compaction. It never removes non-empty source lines or
/// rewrites tokens; it only trims trailing whitespace and collapses runs of blank lines.
#[must_use]
pub fn compact_source_excerpt(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0usize;
    for line in text.lines() {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(trimmed_end);
        out.push('\n');
    }
    if !text.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_ownership_outranks_import_and_call_references() {
        let symbol = vec!["validate_session_token".to_string()];
        let natural = vec![
            "session".to_string(),
            "token".to_string(),
            "validate".to_string(),
        ];
        assert!(
            structural_line_bonus(
                "pub fn validate_session_token(token: &str) -> bool {",
                &symbol
            ) > structural_line_bonus("use crate::auth::validate_session_token;", &symbol)
        );
        assert!(
            structural_line_bonus(
                "pub fn validate_session_token(token: &str) -> bool {",
                &natural
            ) > structural_line_bonus("validate_session_token(token)", &natural)
        );
    }

    #[test]
    fn fallback_symbol_extraction_handles_restricted_rust_visibility() {
        let symbols = extract_symbols(
            "pub(super) fn pack_context() {}\npub(crate) struct RepositoryEngine {}\npub(in crate::repo) const TOKEN_LIMIT: usize = 1;\n",
            8,
        );
        assert!(symbols.iter().any(|symbol| symbol.name == "pack_context"));
        assert!(
            symbols
                .iter()
                .any(|symbol| symbol.name == "RepositoryEngine")
        );
        assert!(symbols.iter().any(|symbol| symbol.name == "TOKEN_LIMIT"));
    }

    #[test]
    fn symbol_matching_handles_plural_identifier_fragments() {
        assert_eq!(symbol_term_match_score("PROJECT_MARKERS", "", "project"), 6);
        assert_eq!(symbol_term_match_score("PROJECT_MARKERS", "", "marker"), 6);
    }

    #[test]
    fn natural_query_rewards_partial_identifier_ownership_and_references() {
        let terms = vec![
            "orchid".to_string(),
            "ledger".to_string(),
            "stale".to_string(),
            "record".to_string(),
        ];
        let definition = structural_line_bonus(
            "pub(super) fn orchid_ledger_checkpoint(value: &str) -> (u64, u64) {",
            &terms,
        );
        let reference = structural_line_bonus("orchid_ledger_checkpoint(&value)", &terms);
        let prose = structural_line_bonus("orchid ledger stale record", &terms);
        assert!(definition > reference);
        assert!(reference > prose);
    }

    #[test]
    fn restricted_rust_visibility_is_treated_as_definition_ownership() {
        let terms = vec!["quasar".to_string(), "table".to_string()];
        assert!(
            structural_line_bonus("pub(crate) const QUASAR_TABLE: &[&str] = &[", &terms)
                > structural_line_bonus("QUASAR_TABLE.iter()", &terms)
        );
    }

    #[test]
    fn bm25_prefers_repeated_rare_term() {
        let df = [1, 2];
        let a = bm25_score(&[3, 1], 20, 20.0, &df, 10);
        let b = bm25_score(&[1, 1], 20, 20.0, &df, 10);
        assert!(a > b);
    }

    #[test]
    fn term_statistics_matches_unicode_case_variants() {
        let terms = vec![crate::core::unicode_search_fold("überprüfung")];
        let (_, frequencies) = term_statistics("fn ÜBERPRÜFUNG() {}", &terms);
        assert_eq!(frequencies, vec![1]);
    }

    #[test]
    fn symbol_extraction_is_language_agnostic_for_common_forms() {
        let text = "pub fn auth_token() {}\nclass Validator:\n    pass\n";
        let symbols = extract_symbols(text, 8);
        assert_eq!(symbols[0].name, "auth_token");
        assert_eq!(symbols[1].name, "Validator");
    }

    #[test]
    fn compacting_keeps_non_empty_lines() {
        let compact = compact_source_excerpt("a  \n\n\n b\n");
        assert_eq!(compact, "a\n\n b\n");
    }

    #[test]
    fn weighted_pagerank_prefers_stronger_semantic_edges() {
        let mixed = vec![vec![(1, 0.15), (2, 0.95)], vec![], vec![]];
        let mixed_rank = weighted_pagerank(&mixed, 20);
        assert!(mixed_rank[2] > mixed_rank[1]);

        let lexical_only = vec![vec![(1, 0.15)], vec![]];
        let call_only = vec![vec![(1, 0.90)], vec![]];
        let lexical_rank = weighted_pagerank(&lexical_only, 20);
        let call_rank = weighted_pagerank(&call_only, 20);
        assert!(call_rank[1] > lexical_rank[1]);
    }

    #[test]
    fn term_statistics_treats_composed_and_decomposed_tokens_equally() {
        let term = crate::core::unicode_search_fold("CAFÉAuth");
        let terms = vec![term];
        let (_, composed) = term_statistics("fn CAFÉAuth() {}", &terms);
        let (_, decomposed) = term_statistics("fn Cafe\u{301}Auth() {}", &terms);
        assert_eq!(composed, vec![1]);
        assert_eq!(composed, decomposed);
    }
}
