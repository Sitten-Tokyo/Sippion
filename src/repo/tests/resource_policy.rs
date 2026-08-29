use super::*;

#[test]
fn oversized_source_is_policy_excluded_without_adaptive_retry() {
    let root = temp_root("oversized-policy");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
    std::fs::write(root.join("huge.rs"), vec![b'x'; MAX_SOURCE_BYTES + 1]).expect("write huge");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("definitely_missing"), 8, None).expect("search succeeds");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(outcome.coverage.adaptive_rounds, 1);
    assert_eq!(outcome.coverage.indexed_files, outcome.coverage.eligible_files);
    assert_eq!(outcome.coverage.confidence_milli, 350);
    assert!(!outcome.truncated);
}

#[test]
fn stable_non_utf8_source_is_policy_excluded_without_adaptive_retry() {
    let root = temp_root("nonutf8-policy");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
    std::fs::write(root.join("bad.rs"), [0xff, 0xfe, 0xfd, 0x00]).expect("write non-utf8");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("definitely_missing"), 8, None).expect("search succeeds");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(outcome.coverage.adaptive_rounds, 1);
    assert_eq!(outcome.coverage.indexed_files, outcome.coverage.eligible_files);
    assert_eq!(outcome.coverage.confidence_milli, 350);
    assert!(!outcome.truncated);
}

#[test]
fn shared_analysis_cache_does_not_retain_source_line_signatures() {
    let root = temp_root("cache-structural-only");
    std::fs::create_dir_all(&root).expect("root");
    let sentinel = "CACHE_SOURCE_SENTINEL_9f4b2b";
    std::fs::write(root.join("safe.rs"), format!("fn visible() {{}} // {sentinel}\n")).expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("safe.rs").expect("read source");
    let analysis = repository
        .analyze_source_cached("safe.rs", &source.text, &source.stamp, None, Instant::now() + Duration::from_secs(1))
        .expect("analysis succeeds")
        .expect("analysis result");
    assert!(analysis.symbols.iter().any(|symbol| symbol.name == "visible"));
    let cache = repository.analysis_cache.lock().expect("analysis cache");
    let cached_debug = format!("{:?}", cache.entries.get("safe.rs"));
    assert!(!cached_debug.contains(sentinel));
}

#[test]
fn candidate_generation_pruning_can_never_be_complete_no_match() {
    let root = temp_root("candidate-pruning-completeness");
    std::fs::create_dir_all(&root).expect("root");
    for index in 0..129 {
        std::fs::write(root.join(format!("candidate-{index:03}.rs")), "abc___bcd\n").expect("write n-gram false positive");
    }
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("abcd"), 8, None).expect("search succeeds");
    assert!(outcome.hits.is_empty());
    assert!(outcome.truncated, "candidate pruning must prevent complete NO_MATCH");
    assert_eq!(outcome.coverage.adaptive_rounds, 1);
}

#[test]
fn path_match_is_returned_when_body_has_no_query_term() {
    let root = temp_root("path-match");
    std::fs::create_dir_all(root.join("src/auth")).expect("temp root");
    std::fs::write(root.join("src/auth/middleware.rs"), "pub fn verify_request() -> bool { true }\n").expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("middleware gateway"), 8, None).expect("search succeeds");
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].relative_path, "src/auth/middleware.rs");
    assert!(outcome.hits[0].excerpt.is_empty());
    assert_eq!((outcome.hits[0].start_line, outcome.hits[0].end_line), (0, 0));
    assert_eq!(outcome.hits[0].score, 3.0);
}

#[test]
fn search_redacts_model_visible_excerpt_without_redacting_every_source_read() {
    let root = temp_root("excerpt-redaction");
    std::fs::create_dir_all(&root).expect("temp root");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    std::fs::write(root.join("auth.rs"), format!("const AUTH_TOKEN: &str = \"{secret}\";\n")).expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("auth.rs").expect("read source");
    assert!(source.text.contains(secret));
    let outcome = repository.search(&normalized_query("AUTH_TOKEN credential"), 8, None).expect("search succeeds");
    assert_eq!(outcome.hits.len(), 1);
    assert!(!outcome.hits[0].excerpt.contains(secret));
    assert!(outcome.hits[0].excerpt.contains("SIPPION_REDACTED_TOKEN"));
}

#[test]
fn private_key_redaction_preserves_line_count_without_marker_amplification() {
    let input = concat!("before\n", "-----BEGIN PRIVATE KEY-----\n", "a\n", "b\n", "-----END PRIVATE KEY-----\n", "after\n");
    let redacted = redact_high_confidence_secrets(input);
    assert_eq!(redacted.lines().count(), input.lines().count());
    assert_eq!(redacted.matches("SIPPION_REDACTED_PRIVATE_KEY").count(), 1);
    assert!(!redacted.contains("\na\n"));
    assert!(!redacted.contains("\nb\n"));
    assert!(redacted.len() <= input.len() + 16);
}

#[test]
fn content_matches_always_rank_above_path_only_matches() {
    let content = SearchHit { relative_path: "src/implementation.rs".into(), start_line: 1, end_line: 1, excerpt: "token".into(), score: (CONTENT_MATCH_BASE_SCORE + 10) as f64, source_stamp: None, source_fingerprint: None };
    let path_only = SearchHit { relative_path: "authentication/token/validation/middleware.rs".into(), start_line: 0, end_line: 0, excerpt: String::new(), score: (MAX_QUERY_TERMS * 3) as f64, source_stamp: None, source_fingerprint: None };
    assert!(content.score > path_only.score);
}

#[test]
fn prefiltered_policy_paths_are_counted_for_completeness() {
    let root = temp_root("prefilter-policy-count");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("normal");
    std::fs::write(root.join("Cargo.lock"), "pruned_only_marker = true\n").expect("lockfile");
    std::fs::write(root.join("image.png"), b"not-really-an-image").expect("binary extension");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("pruned_only_marker"), 8, None).expect("search succeeds");
    assert!(outcome.hits.is_empty());
    assert!(outcome.coverage.policy_excluded_files >= 2);
    assert_eq!(outcome.coverage.confidence_milli, 350);
    assert!(!outcome.truncated);
}

#[test]
fn dependency_lockfiles_and_generated_dirs_are_pruned() {
    assert!(is_pruned(Path::new("package-lock.json")));
    assert!(is_pruned(Path::new("ios/Pods/Library.swift")));
    assert!(is_pruned(Path::new("app/.gradle/cache.bin")));
    assert!(is_pruned(Path::new("cmake-build-debug/CMakeCache.txt")));
    assert!(!is_pruned(Path::new("src/package.rs")));
    assert!(!is_pruned(Path::new("Cargo.toml")));
}
