use super::*;

#[test]
fn top_k_candidate_pruning_is_not_an_incomplete_scan() {
    let mut hits = (0..130)
        .map(|index| SearchHit {
            relative_path: format!("src/path-{index}.rs"),
            start_line: 1,
            end_line: 1,
            excerpt: "path only".into(),
            score: 3.0,
            source_stamp: None,
            source_fingerprint: None,
        })
        .collect::<Vec<_>>();
    prune_candidates_if_needed(&mut hits, 64);
    assert!(hits.len() <= 64);
}

#[test]
fn ram_index_reuses_unchanged_files_without_broad_reread() {
    let root = temp_root("ram-index-reuse");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("a.rs"), "fn alpha() {}\n").expect("write a");
    std::fs::write(root.join("b.rs"), "fn beta() {}\n").expect("write b");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let first = repository
        .search(&normalized_query("zzzzmissing"), 8, None)
        .expect("first search");
    assert_eq!(first.coverage.indexed_files, first.coverage.eligible_files);
    assert!(first.coverage.scanned_files >= 2);

    let second = repository
        .search(&normalized_query("zzzzmissing"), 8, None)
        .expect("second search");
    assert_eq!(second.coverage.indexed_files, second.coverage.eligible_files);
    #[cfg(windows)]
    assert!(
        second.coverage.scanned_files >= 2,
        "Windows rebuilds the RAM index per top-level search to preserve fail-closed freshness"
    );
    #[cfg(not(windows))]
    {
        assert_eq!(second.coverage.scanned_files, 0);
        assert_eq!(second.coverage.scanned_bytes, 0);
    }
}

#[test]
fn changed_file_is_invalidated_and_reindexed() {
    let root = temp_root("ram-index-change");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("service.rs"), "fn old_value() {}\n").expect("write source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    repository
        .search(&normalized_query("needle"), 8, None)
        .expect("prime index");
    std::fs::write(
        root.join("service.rs"),
        "fn newly_changed_needle_handler() { println!(\"needle\"); }\n",
    )
    .expect("change source");

    let second = repository
        .search(&normalized_query("needle"), 8, None)
        .expect("changed search");
    assert!(second.coverage.scanned_files > 0);
    assert_eq!(
        second.hits.first().map(|hit| hit.relative_path.as_str()),
        Some("service.rs")
    );
}

#[test]
fn ram_index_adds_identifier_subterms_without_storing_source_body() {
    let document = build_indexed_document("struct AuthTokenValidator;", None);
    let frequencies = indexed_query_frequencies(
        &document,
        &[(stable_term_hash("token"), query_substring_grams("token"))],
    );
    assert!(frequencies[0] > 0);
    assert!(document.terms.len() < 16);
}

#[test]
fn ram_index_preserves_ascii_substring_candidate_recall() {
    let document = build_indexed_document("fn authenticate_request() {}", None);
    let query = [(stable_term_hash("auth"), query_substring_grams("auth"))];
    let frequencies = indexed_query_frequencies(&document, &query);
    assert_eq!(frequencies, vec![1]);
}

#[test]
fn ram_index_preserves_unicode_substring_candidate_recall() {
    let document = build_indexed_document("fn ユーザー認証処理() {}", None);
    let query = [(stable_term_hash("認証"), query_substring_grams("認証"))];
    let frequencies = indexed_query_frequencies(&document, &query);
    assert_eq!(frequencies, vec![1]);
}

#[test]
fn unicode_substring_search_reaches_exact_verification() {
    let root = temp_root("unicode-substring-recall");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(
        root.join("auth.rs"),
        "fn ユーザー認証処理() { println!(\"認証済み\"); }\n",
    )
    .expect("write source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("認証"), 8, None)
        .expect("unicode search");
    assert!(
        outcome.hits.iter().any(|hit| hit.relative_path == "auth.rs"),
        "Unicode substring must not be filtered out by the RAM candidate index"
    );
}

#[test]
fn ascii_substring_recall_survives_mixed_unicode_identifier() {
    let document = build_indexed_document("fn auth認証_handler() {}", None);
    let query = [(stable_term_hash("auth"), query_substring_grams("auth"))];
    let frequencies = indexed_query_frequencies(&document, &query);
    assert_eq!(frequencies, vec![1]);
}

#[test]
fn index_flight_guard_releases_registration_during_unwind() {
    let root = temp_root("index-flight-unwind");
    std::fs::create_dir_all(&root).expect("temp root");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    repository
        .index_inflight
        .lock()
        .expect("index inflight")
        .insert("src/a.rs".to_string());

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = IndexFlightGuard::new(&repository, "src/a.rs".to_string());
        panic!("simulated index worker panic");
    }));

    assert!(unwind.is_err());
    assert!(
        !repository
            .index_inflight
            .lock()
            .expect("index inflight")
            .contains("src/a.rs")
    );
}

#[test]
fn analysis_flight_guard_releases_registration_during_unwind() {
    let root = temp_root("analysis-flight-unwind");
    std::fs::create_dir_all(&root).expect("temp root");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    repository
        .analysis_cache
        .lock()
        .expect("analysis cache")
        .inflight
        .insert("src/a.rs".to_string());

    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = AnalysisFlightGuard::new(&repository, "src/a.rs".to_string());
        panic!("simulated analysis worker panic");
    }));

    assert!(unwind.is_err());
    assert!(
        !repository
            .analysis_cache
            .lock()
            .expect("analysis cache")
            .inflight
            .contains("src/a.rs")
    );
}

#[test]
fn broad_lane_round_robins_top_level_directories() {
    let pending = [
        "frontend/a.rs",
        "frontend/b.rs",
        "backend/a.rs",
        "backend/b.rs",
        "mobile/a.rs",
    ]
    .into_iter()
    .map(|path| PendingFile {
        file: DiscoveredFile {
            path: path.to_string(),
            stamp: None,
        },
        path_bonus: 0,
        changed: false,
    })
    .collect::<Vec<_>>();
    let (_, _, broad) = stratified_pending_lanes(pending);
    let first_three = broad
        .iter()
        .take(3)
        .map(|file| file.file.path.split('/').next().unwrap_or(""))
        .collect::<HashSet<_>>();
    assert_eq!(first_three.len(), 3);
}

#[test]
fn unicode_substring_grams_preserve_sequence_order() {
    let forward = query_substring_grams(&crate::core::unicode_search_fold("認証処理"));
    let reordered = query_substring_grams(&crate::core::unicode_search_fold("証認処理"));
    assert_ne!(forward, reordered);
    assert!(!forward.is_empty());
}

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
