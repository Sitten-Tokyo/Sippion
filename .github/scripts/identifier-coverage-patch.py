#!/usr/bin/env python3
from pathlib import Path

path = Path('src/hybrid.rs')
text = path.read_text(encoding='utf-8')
start = text.index('#[must_use]\npub fn structural_line_bonus')
end = text.index('#[derive(Debug, Clone, PartialEq, Eq)]\npub struct Symbol', start)
replacement = r'''fn strongest_identifier_term_match(line: &str, terms: &[String]) -> usize {
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
        .find(|marker| lower.starts_with(**marker))
    {
        let rest = lower[marker.len()..].trim_start();
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
        let coverage_bonus = 6.0 + identifier_matches as f64 * 4.0;
        // Exact symbol queries keep the established ownership floor. Natural-language queries
        // gain additional credit when several query concepts are owned by one identifier.
        return if identifier_matches == terms.len() {
            coverage_bonus.max(14.0)
        } else {
            coverage_bonus
        }
        .min(22.0);
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

'''
path.write_text(text[:start] + replacement + text[end:], encoding='utf-8')

text = path.read_text(encoding='utf-8')
needle = '''    #[test]\n    fn bm25_prefers_repeated_rare_term() {'''
insert = '''    #[test]\n    fn natural_query_rewards_partial_identifier_ownership_and_references() {\n        let terms = vec![\n            "source".to_string(),\n            "fingerprint".to_string(),\n            "stale".to_string(),\n            "evidence".to_string(),\n        ];\n        let definition = structural_line_bonus(\n            "pub(super) fn source_content_fingerprint(text: &str) -> (u64, u64) {",\n            &terms,\n        );\n        let reference = structural_line_bonus("source_content_fingerprint(&text)", &terms);\n        let prose = structural_line_bonus("source fingerprint stale evidence", &terms);\n        assert!(definition > reference);\n        assert!(reference > prose);\n    }\n\n    #[test]\n    fn bm25_prefers_repeated_rare_term() {'''
if text.count(needle) != 1:
    raise SystemExit('test insertion marker mismatch')
path.write_text(text.replace(needle, insert, 1), encoding='utf-8')
