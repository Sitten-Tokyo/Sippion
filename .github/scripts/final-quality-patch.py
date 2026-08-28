#!/usr/bin/env python3
from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    target = Path(path)
    text = target.read_text(encoding="utf-8")
    if text.count(old) != 1:
        raise SystemExit(f"{path}: expected exactly one marker, got {text.count(old)}")
    target.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_once(
    "src/service/context.rs",
    "const MAX_PACKED_ATOMS: usize = 24;",
    "// Keep broad repositories from turning semantic expansion into a large model-visible file list.\n// Twelve atoms still leave room for multiple evidence/structure pairs while making the soft token\n// budget the dominant bound rather than the number of discovered neighbors.\nconst MAX_PACKED_ATOMS: usize = 12;",
)

replace_once(
    "src/core.rs",
    "pub const MIN_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {\n    target_estimated_tokens: 1_800,\n    hard_model_text_bytes: 8 * 1024,\n};\n\npub const MAX_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {\n    target_estimated_tokens: 7_200,\n    hard_model_text_bytes: 32 * 1024,\n};",
    "pub const MIN_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {\n    target_estimated_tokens: 1_400,\n    hard_model_text_bytes: 8 * 1024,\n};\n\npub const MAX_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {\n    target_estimated_tokens: 2_600,\n    hard_model_text_bytes: 32 * 1024,\n};",
)
replace_once(
    "src/core.rs",
    "        1 => ModelVisibleBudget {\n            target_estimated_tokens: 3_600,\n            hard_model_text_bytes: 16 * 1024,\n        },\n        2 => ModelVisibleBudget {\n            target_estimated_tokens: 5_400,\n            hard_model_text_bytes: 24 * 1024,\n        },",
    "        1 => ModelVisibleBudget {\n            target_estimated_tokens: 1_800,\n            hard_model_text_bytes: 16 * 1024,\n        },\n        2 => ModelVisibleBudget {\n            target_estimated_tokens: 2_200,\n            hard_model_text_bytes: 24 * 1024,\n        },",
)
replace_once(
    "src/core.rs",
    "        assert_eq!(\n            adaptive_context_budget(0.30, 17, 20, 8).hard_model_text_bytes,\n            32 * 1024\n        );",
    "        assert_eq!(\n            adaptive_context_budget(0.30, 17, 20, 8).hard_model_text_bytes,\n            32 * 1024\n        );\n        assert_eq!(adaptive_context_budget(0.95, 2, 3, 2).target_estimated_tokens, 1_400);\n        assert_eq!(adaptive_context_budget(0.80, 6, 8, 3).target_estimated_tokens, 1_800);\n        assert_eq!(adaptive_context_budget(0.60, 12, 14, 5).target_estimated_tokens, 2_200);\n        assert_eq!(adaptive_context_budget(0.30, 17, 20, 8).target_estimated_tokens, 2_600);",
)

replace_once(
    "src/repo/ranking.rs",
    "pub(super) fn path_match_score(path: &str, terms: &[String]) -> usize {\n    let path_lower = crate::core::unicode_search_fold(path);\n    terms\n        .iter()\n        .filter(|term| path_lower.contains(term.as_str()))\n        .count()\n}\n",
    "pub(super) fn path_match_score(path: &str, terms: &[String]) -> usize {\n    let path_lower = crate::core::unicode_search_fold(path);\n    terms\n        .iter()\n        .filter(|term| path_lower.contains(term.as_str()))\n        .count()\n}\n\npub(super) fn coding_source_prior(path: &str) -> f64 {\n    // Sippion prepares context for coding tasks. Documentation remains searchable, but when\n    // lexical evidence is otherwise similar, implementation source should beat changelogs/history.\n    // Keep the prior smaller than two matched query terms so strong documentation evidence can\n    // still win when the user's query is actually documentation-oriented.\n    let extension = path.rsplit_once('.').map(|(_, extension)| extension);\n    match extension.map(str::to_ascii_lowercase).as_deref() {\n        Some(\"rs\" | \"py\" | \"js\" | \"jsx\" | \"ts\" | \"tsx\" | \"go\" | \"java\" | \"cs\" | \"c\" | \"h\" | \"cc\" | \"cpp\" | \"cxx\" | \"hpp\" | \"hxx\") => 14.0,\n        _ => 0.0,\n    }\n}\n",
)

# Candidate generation and exact-ranking must use the same prior so a source file is not pruned
# before verification and then ranked by a different formula afterward.
replace_once(
    "src/repo/search.rs",
    "                candidate.score =\n                    (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64\n                        + bm25 * 12.0\n                        + memory_bonus;",
    "                candidate.score =\n                    (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64\n                        + bm25 * 12.0\n                        + coding_source_prior(&candidate.relative_path)\n                        + memory_bonus;",
)
replace_once(
    "src/repo/search.rs",
    "                score: (CONTENT_MATCH_BASE_SCORE\n                    + matched * 10\n                    + *exact_bonus * 8\n                    + candidate.path_bonus * 3) as f64\n                    + bm25 * 12.0\n                    + *structure_bonus\n                    + memory_bonus,",
    "                score: (CONTENT_MATCH_BASE_SCORE\n                    + matched * 10\n                    + *exact_bonus * 8\n                    + candidate.path_bonus * 3) as f64\n                    + bm25 * 12.0\n                    + *structure_bonus\n                    + coding_source_prior(&candidate.relative_path)\n                    + memory_bonus,",
)
replace_once(
    "src/repo/search.rs",
    "                score: (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64\n                    + bm25 * 12.0\n                    + memory_bonus,",
    "                score: (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64\n                    + bm25 * 12.0\n                    + coding_source_prior(&candidate.relative_path)\n                    + memory_bonus,",
)

# Put the source prior under unit test without coupling the test to retrieval fixture wording.
ranking = Path("src/repo/ranking.rs")
text = ranking.read_text(encoding="utf-8")
marker = "pub(super) fn hit_is_better(candidate: &SearchHit, current: &SearchHit) -> bool {"
if text.count(marker) != 1:
    raise SystemExit("ranking.rs: hit_is_better marker mismatch")
helper_test = "#[cfg(test)]\nmod coding_source_prior_tests {\n    use super::coding_source_prior;\n\n    #[test]\n    fn implementation_sources_get_a_small_coding_prior() {\n        assert!(coding_source_prior(\"src/repo/map.rs\") > coding_source_prior(\"docs/architecture.md\"));\n        assert_eq!(coding_source_prior(\"CHANGELOG.md\"), 0.0);\n    }\n}\n\n"
ranking.write_text(text.replace(marker, helper_test + marker, 1), encoding="utf-8")
