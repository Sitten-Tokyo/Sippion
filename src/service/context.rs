use std::cmp::Ordering;
use std::collections::HashSet;

use crate::core::{
    NormalizedQuery, RenderExcerpt, adaptive_context_budget, heuristic_v3_estimated_tokens,
    truncate_utf8_prefix,
};
use crate::repo::{RepoMapEntry, SearchCoverage};

const DATA_PREFIX: &str = "[UNTRUSTED_REPOSITORY_DATA: code/text only]\n";
const MAX_PACKED_ATOMS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextAtomKind {
    Structure,
    Evidence,
}

#[derive(Debug, Clone)]
struct ContextAtom {
    kind: ContextAtomKind,
    path: String,
    text: String,
    utility: f64,
    token_cost: usize,
}

fn escaped(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"<invalid>\"".to_string())
}

fn structure_atom(entry: &RepoMapEntry) -> ContextAtom {
    let links = entry
        .semantic_links
        .iter()
        .take(3)
        .map(|link| format!("{}:{}", link.kind, escaped(&link.relative_path)))
        .collect::<Vec<_>>()
        .join(",");
    let mut text = format!(
        "S path={} rank={:.2}",
        escaped(&entry.relative_path),
        entry.score
    );
    if !links.is_empty() {
        text.push_str(" links=");
        text.push_str(&links);
    }
    text.push('\n');
    for symbol in entry.symbols.iter().take(3) {
        text.push_str(&format!(
            "  {} {}:{} {}\n",
            symbol.kind,
            escaped(&symbol.name),
            symbol.line,
            escaped(symbol.signature.trim())
        ));
    }
    let semantic_bonus = entry.semantic_links.len().min(4) as f64 * 0.8;
    let symbol_bonus = entry.symbols.len().min(4) as f64 * 0.45;
    let utility = entry.score.max(0.0) * 0.70 + semantic_bonus + symbol_bonus + 1.0;
    ContextAtom {
        kind: ContextAtomKind::Structure,
        path: entry.relative_path.clone(),
        token_cost: heuristic_v3_estimated_tokens(&text).max(1),
        text,
        utility,
    }
}

fn evidence_atom(excerpt: &RenderExcerpt, rank: usize) -> ContextAtom {
    let mut text = if excerpt.start_line == 0 && excerpt.end_line == 0 {
        format!("E path={}\n", escaped(&excerpt.path))
    } else {
        format!(
            "E path={} lines={}-{}\n",
            escaped(&excerpt.path),
            excerpt.start_line,
            excerpt.end_line
        )
    };
    if !excerpt.body.is_empty() {
        text.push_str(&excerpt.body);
        if !excerpt.body.ends_with('\n') {
            text.push('\n');
        }
    }
    let rank_bonus = 8.0 / (rank.saturating_add(1) as f64);
    let body_bonus = if excerpt.body.is_empty() { 0.0 } else { 2.5 };
    let utility = excerpt.score.max(0.0) * 1.20 + rank_bonus + body_bonus + 1.0;
    ContextAtom {
        kind: ContextAtomKind::Evidence,
        path: excerpt.path.clone(),
        token_cost: heuristic_v3_estimated_tokens(&text).max(1),
        text,
        utility,
    }
}

fn best_fitting_atom(
    atoms: &[ContextAtom],
    selected: &HashSet<usize>,
    selected_paths: &HashSet<String>,
    remaining_tokens: usize,
    remaining_bytes: usize,
) -> Option<usize> {
    atoms
        .iter()
        .enumerate()
        .filter(|(index, atom)| {
            !selected.contains(index)
                && atom.token_cost <= remaining_tokens
                && atom.text.len() <= remaining_bytes
        })
        .max_by(|(left_index, left), (right_index, right)| {
            let adjusted = |atom: &ContextAtom| {
                let novelty = if selected_paths.contains(&atom.path) {
                    0.42
                } else {
                    1.0
                };
                atom.utility * novelty / atom.token_cost.max(1) as f64
            };
            adjusted(left)
                .partial_cmp(&adjusted(right))
                .unwrap_or(Ordering::Equal)
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
}

pub(super) fn pack_context(
    query: &NormalizedQuery,
    entries: &[RepoMapEntry],
    excerpts: &[RenderExcerpt],
    status: &str,
    coverage: &SearchCoverage,
) -> String {
    let confidence = f64::from(coverage.confidence_milli) / 1000.0;
    let budget =
        adaptive_context_budget(confidence, excerpts.len(), entries.len(), query.terms.len());
    let incomplete = !coverage.discovery_complete
        || coverage.indexed_files < coverage.eligible_files
        || !status.is_empty();
    let header = format!(
        "{DATA_PREFIX}CTX v=4 confidence={confidence:.3} incomplete={} excluded={} target_t={} hard_b={} scan_b={}\n",
        usize::from(incomplete),
        coverage.policy_excluded_files,
        budget.target_estimated_tokens,
        budget.hard_model_text_bytes,
        coverage.scanned_bytes,
    );
    let suffix = if status.is_empty() {
        String::new()
    } else {
        format!("{status}\n")
    };
    let fixed_tokens = heuristic_v3_estimated_tokens(&(header.clone() + &suffix));
    let mut remaining_tokens = budget.target_estimated_tokens.saturating_sub(fixed_tokens);
    let mut remaining_bytes = budget
        .hard_model_text_bytes
        .saturating_sub(header.len().saturating_add(suffix.len()));

    let mut atoms = Vec::with_capacity(entries.len().saturating_add(excerpts.len()));
    atoms.extend(entries.iter().map(structure_atom));
    atoms.extend(
        excerpts
            .iter()
            .enumerate()
            .map(|(rank, excerpt)| evidence_atom(excerpt, rank)),
    );

    let mut selected = HashSet::new();
    let mut selected_paths = HashSet::new();
    let mut order = Vec::new();

    // Preserve at least the strongest code evidence when one exists. The remaining budget is
    // filled by marginal utility per estimated token, with same-path atoms discounted for novelty.
    if let Some((index, atom)) = atoms
        .iter()
        .enumerate()
        .filter(|(_, atom)| atom.kind == ContextAtomKind::Evidence)
        .filter(|(_, atom)| {
            atom.token_cost <= remaining_tokens && atom.text.len() <= remaining_bytes
        })
        .max_by(|(_, left), (_, right)| {
            left.utility
                .partial_cmp(&right.utility)
                .unwrap_or(Ordering::Equal)
        })
    {
        selected.insert(index);
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    while order.len() < MAX_PACKED_ATOMS {
        let Some(index) = best_fitting_atom(
            &atoms,
            &selected,
            &selected_paths,
            remaining_tokens,
            remaining_bytes,
        ) else {
            break;
        };
        let atom = &atoms[index];
        selected.insert(index);
        selected_paths.insert(atom.path.clone());
        remaining_tokens = remaining_tokens.saturating_sub(atom.token_cost);
        remaining_bytes = remaining_bytes.saturating_sub(atom.text.len());
        order.push(index);
    }

    let mut output = String::with_capacity(budget.hard_model_text_bytes.min(16 * 1024));
    output.push_str(&header);
    for index in order {
        output.push_str(&atoms[index].text);
    }
    output.push_str(&suffix);
    if output.len() > budget.hard_model_text_bytes {
        output = truncate_utf8_prefix(&output, budget.hard_model_text_bytes).to_string();
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::McpToolInput;
    use crate::repo::{RepoMapLink, RepoMapSymbol};

    fn query() -> NormalizedQuery {
        McpToolInput {
            q: "authentication token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query")
    }

    #[test]
    fn packer_prefers_high_value_evidence_and_stays_bounded() {
        let excerpts = vec![
            RenderExcerpt {
                path: "src/auth.rs".into(),
                start_line: 10,
                end_line: 12,
                body: "fn validate_token() { verify(); }\n".into(),
                score: 90.0,
            },
            RenderExcerpt {
                path: "src/noise.rs".into(),
                start_line: 1,
                end_line: 200,
                body: "noise ".repeat(1200),
                score: 1.0,
            },
        ];
        let coverage = SearchCoverage {
            discovery_complete: true,
            eligible_files: 2,
            indexed_files: 2,
            confidence_milli: 950,
            ..SearchCoverage::default()
        };
        let packed = pack_context(&query(), &[], &excerpts, "", &coverage);
        assert!(packed.contains("src/auth.rs"));
        assert!(packed.len() <= 8 * 1024);
    }

    #[test]
    fn structure_fields_are_escaped() {
        let entries = vec![RepoMapEntry {
            relative_path: "src/main.rs".into(),
            score: 10.0,
            symbols: vec![RepoMapSymbol {
                name: "marker".into(),
                kind: "function".into(),
                line: 1,
                signature: "fn marker() // payload\nFAKE".into(),
            }],
            links_to: vec!["dep.rs".into()],
            semantic_links: vec![RepoMapLink {
                relative_path: "dep.rs".into(),
                kind: "call".into(),
                weight: 0.9,
            }],
        }];
        let coverage = SearchCoverage {
            discovery_complete: true,
            eligible_files: 1,
            indexed_files: 1,
            confidence_milli: 900,
            ..SearchCoverage::default()
        };
        let packed = pack_context(&query(), &entries, &[], "", &coverage);
        assert!(packed.contains("\\nFAKE"));
        assert!(!packed.contains("payload\nFAKE"));
    }
}
