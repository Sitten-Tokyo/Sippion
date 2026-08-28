#!/usr/bin/env python3
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    (ROOT / path).write_text(text, encoding="utf-8")


def replace_once(path: str, old: str, new: str) -> None:
    text = read(path)
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one replacement target, found {count}")
    write(path, text.replace(old, new, 1))


def append_before_last_brace(path: str, marker: str, addition: str) -> None:
    text = read(path)
    if marker in text:
        return
    index = text.rfind("\n}")
    if index < 0:
        raise SystemExit(f"{path}: final module brace not found")
    write(path, text[:index] + "\n" + addition.rstrip() + "\n" + text[index:])


# Release candidate version.
replace_once("Cargo.toml", 'version = "0.1.0-rc.34"', 'version = "0.1.0-rc.35"')
replace_once(
    "Cargo.lock",
    'name = "sippion"\nversion = "0.1.0-rc.34"',
    'name = "sippion"\nversion = "0.1.0-rc.35"',
)

# Make duplicate dependency versions fail by default. Current unavoidable duplicates will be
# inventoried explicitly if cargo-deny reports any during this validation run.
replace_once(
    "deny.toml",
    'multiple-versions = "warn"',
    'multiple-versions = "deny"',
)

# Normalize the complete query/source before token boundary detection, and keep Unicode combining
# marks attached to their base token. This makes composed and decomposed spellings tokenize alike.
replace_once(
    "src/core.rs",
    "use unicode_normalization::char::decompose_compatible;",
    "use unicode_normalization::char::{decompose_compatible, is_combining_mark};",
)

core_anchor = '''#[must_use]\npub(crate) fn unicode_search_fold(text: &str) -> String {\n    if text.is_ascii() {\n        return text.to_ascii_lowercase();\n    }\n    let mut folded = String::with_capacity(text.len());\n    for ch in text.chars() {\n        fold_search_scalar(ch, |folded_ch| folded.push(folded_ch));\n    }\n    folded\n}\n'''
core_replacement = core_anchor + '''\nfn is_search_token_char(ch: char) -> bool {\n    ch.is_alphanumeric() || ch == '_' || ch == '-' || is_combining_mark(ch)\n}\n\n/// Splits text that has already gone through `unicode_search_fold`. Combining marks remain part of\n/// the preceding identifier token, and the minimum length is expressed in alphanumeric Unicode\n/// scalars rather than UTF-8 bytes.\npub(crate) fn split_search_tokens(\n    folded_text: &str,\n) -> impl Iterator<Item = &str> {\n    folded_text\n        .split(|ch: char| !is_search_token_char(ch))\n        .filter(|part| {\n            part.chars()\n                .filter(|ch| ch.is_alphanumeric())\n                .take(2)\n                .count()\n                >= 2\n        })\n}\n'''
replace_once("src/core.rs", core_anchor, core_replacement)

old_normalize = '''        let mut terms = Vec::new();\n        for part in self\n            .q\n            .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))\n            .filter(|part| part.len() >= 2)\n            .map(unicode_search_fold)\n            .filter(|part| !QUERY_STOPWORDS.contains(&part.as_str()))\n        {\n            if !terms.contains(&part) {\n                terms.push(part);\n                if terms.len() > MAX_QUERY_TERMS {\n                    return Err(InputError::TooManyTerms);\n                }\n            }\n        }\n        if terms.len() < MIN_QUERY_TERMS {\n            return Err(InputError::TooFewTerms);\n        }\n\n        Ok(NormalizedQuery {\n            raw_lower: unicode_search_fold(&self.q),\n            terms,\n        })\n'''
new_normalize = '''        let raw_lower = unicode_search_fold(&self.q);\n        let mut terms = Vec::new();\n        for part in split_search_tokens(&raw_lower)\n            .filter(|part| !QUERY_STOPWORDS.contains(part))\n        {\n            let part = part.to_string();\n            if !terms.contains(&part) {\n                terms.push(part);\n                if terms.len() > MAX_QUERY_TERMS {\n                    return Err(InputError::TooManyTerms);\n                }\n            }\n        }\n        if terms.len() < MIN_QUERY_TERMS {\n            return Err(InputError::TooFewTerms);\n        }\n\n        Ok(NormalizedQuery { raw_lower, terms })\n'''
replace_once("src/core.rs", old_normalize, new_normalize)

old_term_stats = '''pub fn term_statistics(text: &str, terms: &[String]) -> (usize, Vec<usize>) {\n    let mut frequencies = vec![0usize; terms.len()];\n    let mut document_len = 0usize;\n    for part in text\n        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))\n        .filter(|part| part.len() >= 2)\n    {\n        document_len = document_len.saturating_add(1);\n        let folded = crate::core::unicode_search_fold(part);\n        for (index, term) in terms.iter().enumerate() {\n            if folded == *term {\n                frequencies[index] = frequencies[index].saturating_add(1);\n            }\n        }\n    }\n\n    // Code identifiers are often compounds (validate_token, AuthTokenValidator). Preserve the\n    // old substring-recall behaviour as a one-hit fallback while BM25 rewards exact token repeats.\n    let lower = crate::core::unicode_search_fold(text);\n    for (index, term) in terms.iter().enumerate() {\n        if frequencies[index] == 0 && lower.contains(term.as_str()) {\n            frequencies[index] = 1;\n        }\n    }\n    (document_len.max(1), frequencies)\n}\n'''
new_term_stats = '''pub fn term_statistics(text: &str, terms: &[String]) -> (usize, Vec<usize>) {\n    let mut frequencies = vec![0usize; terms.len()];\n    let mut document_len = 0usize;\n    let folded_text = crate::core::unicode_search_fold(text);\n    for part in crate::core::split_search_tokens(&folded_text) {\n        document_len = document_len.saturating_add(1);\n        for (index, term) in terms.iter().enumerate() {\n            if part == term {\n                frequencies[index] = frequencies[index].saturating_add(1);\n            }\n        }\n    }\n\n    // Code identifiers are often compounds (validate_token, AuthTokenValidator). Preserve the\n    // old substring-recall behaviour as a one-hit fallback while BM25 rewards exact token repeats.\n    for (index, term) in terms.iter().enumerate() {\n        if frequencies[index] == 0 && folded_text.contains(term.as_str()) {\n            frequencies[index] = 1;\n        }\n    }\n    (document_len.max(1), frequencies)\n}\n'''
replace_once("src/hybrid.rs", old_term_stats, new_term_stats)

append_before_last_brace(
    "src/core.rs",
    "query_tokenization_treats_composed_and_decomposed_forms_equally",
    r'''    #[test]
    fn query_tokenization_treats_composed_and_decomposed_forms_equally() {
        let composed = McpToolInput {
            q: "CAFÉAuth token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("composed query");
        let decomposed = McpToolInput {
            q: "Cafe\u{301}Auth token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("decomposed query");
        assert_eq!(composed.raw_lower, decomposed.raw_lower);
        assert_eq!(composed.terms, decomposed.terms);
    }

    #[test]
    fn token_minimum_length_counts_unicode_letters_not_utf8_bytes() {
        assert_eq!(
            McpToolInput {
                q: "é".into(),
                ..Default::default()
            }
            .normalize(),
            Err(InputError::TooFewTerms)
        );
        assert!(
            McpToolInput {
                q: "認証".into(),
                ..Default::default()
            }
            .normalize()
            .is_ok()
        );
    }

    #[test]
    fn deterministic_tokenization_property_corpus_keeps_canonical_equivalence() {
        for index in 0..256u16 {
            let composed = format!("prefix{index}-CAFÉAuth suffix{index}");
            let decomposed = format!("prefix{index}-Cafe\u{301}Auth suffix{index}");
            let a = McpToolInput {
                q: composed,
                ..Default::default()
            }
            .normalize()
            .expect("composed corpus query");
            let b = McpToolInput {
                q: decomposed,
                ..Default::default()
            }
            .normalize()
            .expect("decomposed corpus query");
            assert_eq!(a.terms, b.terms, "corpus case {index}");
        }
    }
''',
)

append_before_last_brace(
    "src/hybrid.rs",
    "term_statistics_treats_composed_and_decomposed_tokens_equally",
    r'''    #[test]
    fn term_statistics_treats_composed_and_decomposed_tokens_equally() {
        let term = crate::core::unicode_search_fold("CAFÉAuth");
        let terms = vec![term];
        let (_, composed) = term_statistics("fn CAFÉAuth() {}", &terms);
        let (_, decomposed) = term_statistics("fn Cafe\u{301}Auth() {}", &terms);
        assert_eq!(composed, vec![1]);
        assert_eq!(composed, decomposed);
    }
''',
)

repo_tests = read("src/repo/tests.rs")
if "generated_sensitive_literal_corpus_never_leaks" not in repo_tests:
    repo_tests += r'''

#[test]
fn generated_sensitive_literal_corpus_never_leaks() {
    let keys = [
        "password",
        "passwd",
        "secret_key",
        "secret_key_base",
        "signing_key",
        "encryption_key",
    ];
    for index in 0..512usize {
        let key = keys[index % keys.len()];
        let secret = format!("generated-secret-{index:04}-Zx9Q");
        let line = match index % 3 {
            0 => format!("{key} = \"{secret}\""),
            1 => format!("{key}: '{secret}'"),
            _ => format!("\"{key}\": \"{secret}\","),
        };
        let redacted = redact_high_confidence_secrets(&line);
        assert!(
            !redacted.contains(&secret),
            "secret leaked for generated case {index}: {redacted}"
        );
        assert!(redacted.contains("SIPPION_REDACTED"));
    }
}

#[test]
fn generated_ascii_case_variants_of_sensitive_paths_stay_denied() {
    let sensitive = [
        ".ssh/id_rsa",
        ".env.production",
        ".cargo/credentials.toml",
        ".terraformrc",
        ".vault-token",
        "auth.json",
    ];
    let mut state = 0x5eed_u64;
    for round in 0..128usize {
        for path in sensitive {
            let variant = path
                .bytes()
                .map(|byte| {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    if byte.is_ascii_alphabetic() && (state >> 63) != 0 {
                        byte.to_ascii_uppercase()
                    } else {
                        byte
                    }
                })
                .map(char::from)
                .collect::<String>();
            assert!(
                is_denied(Path::new(&variant)),
                "generated path variant escaped policy in round {round}: {variant}"
            );
        }
    }
}
'''
    write("src/repo/tests.rs", repo_tests)

print("post-rc34 hardening patch applied")
