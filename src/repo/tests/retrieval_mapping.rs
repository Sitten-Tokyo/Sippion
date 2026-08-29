use super::*;

#[test]
fn search_candidate_excerpt_is_bounded_before_retention() {
    let long = "x".repeat(MAX_SEARCH_EXCERPT_BYTES * 2);
    let lines = [long.as_str()];
    let (bounded, start, end) = bounded_search_excerpt(&lines, 0, MAX_SEARCH_EXCERPT_BYTES);
    assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
    assert!(bounded.contains("SIPPION_EXCERPT_TRUNCATED"));
    assert_eq!((start, end), (0, 1));
}

#[test]
fn non_utf8_read_reports_consumed_bytes_for_scan_budget() {
    let root = temp_root("binary");
    std::fs::create_dir_all(&root).expect("temp root");
    let bytes = [0xff, 0xfe, 0xfd, 0x00];
    std::fs::write(root.join("binary.bin"), bytes).expect("write binary file");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let (error, consumed) = repository.read_source("binary.bin").expect_err("binary must not become model text");
    assert_eq!(error, RepositoryAccessError::NonUtf8Source);
    assert_eq!(consumed, bytes.len());
}

#[test]
fn file_local_best_hit_prefers_higher_score_then_earlier_line() {
    let earlier = SearchHit { relative_path: "a.rs".into(), start_line: 2, end_line: 2, excerpt: "earlier".into(), score: 10.0, source_stamp: None, source_fingerprint: None };
    let later = SearchHit { start_line: 20, ..earlier.clone() };
    let higher = SearchHit { score: 11.0, ..later.clone() };
    assert!(hit_is_better(&earlier, &later));
    assert!(hit_is_better(&higher, &earlier));
}

#[test]
fn bounded_focus_line_is_utf8_safe_and_byte_bounded() {
    let line = format!("{}MATCH{}", "界".repeat(900), "界".repeat(900));
    let match_byte = line.find("MATCH").expect("match");
    let bounded = bounded_focus_line(&line, match_byte);
    assert!(bounded.contains("MATCH"));
    assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
}

#[test]
fn bounded_excerpt_keeps_match_when_an_adjacent_line_is_huge() {
    let huge = "x".repeat(MAX_SEARCH_EXCERPT_BYTES * 2);
    let lines = [huge.as_str(), "authentication_token_validation"];
    let (excerpt, start, end) = bounded_search_excerpt(&lines, 1, 0);
    assert!(excerpt.contains("authentication_token_validation"));
    assert!(excerpt.len() <= MAX_SEARCH_EXCERPT_BYTES);
    assert!(start <= 1 && end >= 2);
}

#[test]
fn multi_line_query_terms_score_as_one_evidence_window() {
    let root = temp_root("window-score");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("a_relevant.rs"), "fn check() {\n    // authentication\n    let token = load();\n    // validation\n}\n").expect("write relevant source");
    std::fs::write(root.join("z_noise.rs"), "let token = load();\n").expect("write noise source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query("authentication token validation"), 8, None).expect("search succeeds");
    assert_eq!(outcome.hits.first().map(|hit| hit.relative_path.as_str()), Some("a_relevant.rs"));
    assert!(outcome.hits[0].score > outcome.hits[1].score);
}

#[test]
fn obvious_binary_formats_are_pruned_before_source_scan() {
    assert!(is_obvious_binary(Path::new("assets/logo.png")));
    assert!(is_obvious_binary(Path::new("lib/archive.JAR")));
    assert!(!is_obvious_binary(Path::new("src/app.rs")));
    assert!(!is_obvious_binary(Path::new("assets/icon.svg")));
}

#[test]
fn non_utf8_skip_is_not_reported_as_bounded_scan_failure() {
    assert!(!read_failure_makes_scan_incomplete(&RepositoryAccessError::NonUtf8Source));
    assert!(read_failure_makes_scan_incomplete(&RepositoryAccessError::TooLarge));
    assert!(!read_failure_makes_scan_incomplete(&RepositoryAccessError::DeniedPath));
}

#[test]
fn structural_map_links_symbol_references_with_multi_pattern_matcher() {
    let root = temp_root("structural-aho");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n").expect("write caller");
    std::fs::write(root.join("auth.rs"), "pub fn authenticate() -> bool { true }\n").expect("write auth");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![
        SearchHit { relative_path: "caller.rs".into(), start_line: 1, end_line: 1, excerpt: "authenticate".into(), score: 10.0, source_stamp: None, source_fingerprint: None },
        SearchHit { relative_path: "auth.rs".into(), start_line: 1, end_line: 1, excerpt: "authenticate".into(), score: 9.0, source_stamp: None, source_fingerprint: None },
    ];
    let map = repository.map_from_hits(&normalized_query("authenticate"), &hits, 2, None).expect("map succeeds");
    let caller = map.entries.iter().find(|entry| entry.relative_path == "caller.rs").expect("caller entry");
    assert!(caller.links_to.iter().any(|path| path == "auth.rs"));
    assert!(caller.semantic_links.iter().any(|link| link.relative_path == "auth.rs" && link.weight >= 0.80));
}

#[test]
fn structural_analysis_and_graph_are_shared_across_repeated_calls() {
    let root = temp_root("shared-analysis-cache");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n").expect("write caller");
    std::fs::write(root.join("auth.rs"), "pub fn authenticate() -> bool { true }\n").expect("write auth");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![
        SearchHit { relative_path: "caller.rs".into(), start_line: 1, end_line: 1, excerpt: "authenticate".into(), score: 10.0, source_stamp: None, source_fingerprint: None },
        SearchHit { relative_path: "auth.rs".into(), start_line: 1, end_line: 1, excerpt: "authenticate".into(), score: 9.0, source_stamp: None, source_fingerprint: None },
    ];
    let query = normalized_query("authenticate");
    repository.map_from_hits(&query, &hits, 2, None).expect("first map");
    let analysis_entries = repository.analysis_cache.lock().expect("analysis cache").entries.len();
    let graph_entries = repository.graph_cache.lock().expect("graph cache").entries.len();
    assert_eq!(analysis_entries, 2);
    assert_eq!(graph_entries, 1);
    repository.map_from_hits(&query, &hits, 2, None).expect("second map");
    assert_eq!(repository.analysis_cache.lock().expect("analysis cache").entries.len(), analysis_entries);
    assert_eq!(repository.graph_cache.lock().expect("graph cache").entries.len(), graph_entries);
}
