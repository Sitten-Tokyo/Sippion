use super::*;

#[test]
fn final_ranking_verifies_candidates_beyond_provisional_top_n() {
    let root = temp_root("final-top-n");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("a.rs"), "alpha gamma beta\n").expect("write a");
    std::fs::write(root.join("z.rs"), "alpha beta gamma\n").expect("write z");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("alpha beta"), 1, None)
        .expect("search");

    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].relative_path, "z.rs");
}

#[test]
fn exact_verification_cache_advances_into_new_candidates_across_adaptive_rounds() {
    let root = temp_root("verification-cache");
    std::fs::create_dir_all(&root).expect("create root");
    let source = format!("fn alpha() {{}}\n{}", " ".repeat(100 * 1024));
    std::fs::write(root.join("a.rs"), &source).expect("write a");
    std::fs::write(root.join("b.rs"), &source).expect("write b");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let query = normalized_query("alpha");
    let started = Instant::now();
    let mut policy_skips = HashMap::new();
    let mut verification_cache = HashMap::new();

    // 512 KiB gives exact verification 128 KiB: enough for one ~100 KiB file, not both.
    let first = repository
        .search_once(
            &query,
            8,
            None,
            &started,
            512 * 1024,
            &mut policy_skips,
            &mut verification_cache,
            None,
        )
        .expect("first round");
    assert_eq!(first.hits.len(), 1);
    assert_eq!(verification_cache.len(), 1);

    let second = repository
        .search_once(
            &query,
            8,
            None,
            &started,
            512 * 1024,
            &mut policy_skips,
            &mut verification_cache,
            None,
        )
        .expect("second round");
    assert_eq!(
        second.hits.len(),
        2,
        "the second byte grant must advance past the cached leading candidate"
    );
    assert_eq!(verification_cache.len(), 2);
    assert_eq!(
        second.coverage.scanned_bytes,
        source.len(),
        "only the newly verified candidate should consume source bytes in round two"
    );
}

#[test]
fn structural_mapping_rejects_content_change_even_when_stamp_appears_current() {
    let root = temp_root("evidence-generation");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("main.rs"), "fn alpha() {}\n").expect("write alpha");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let query = normalized_query("alpha");
    let search = repository.search(&query, 8, None).expect("search");
    let mut hit = search.hits.into_iter().next().expect("alpha hit");
    let old_fingerprint = hit
        .source_fingerprint
        .expect("verified content fingerprint");

    std::fs::write(root.join("main.rs"), "fn bravo() {}\n").expect("replace source");
    let replacement = repository.read_source("main.rs").expect("read replacement");
    assert_ne!(
        source_content_fingerprint(&replacement.text),
        old_fingerprint
    );
    hit.source_stamp = Some(replacement.stamp);

    let map = repository
        .map_from_hits(&query, &[hit], 1, None)
        .expect("bounded map");
    assert!(map.truncated);
    assert!(map.entries.is_empty());
    assert_eq!(map.invalidated_evidence_paths, vec!["main.rs"]);
}

#[test]
fn structural_mapping_revalidates_hits_beyond_structural_limit() {
    let root = temp_root("evidence-beyond-map-limit");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("a.rs"), "fn alpha() {}\n").expect("write a");
    std::fs::write(root.join("z.rs"), "fn alpha() {}\n").expect("write z");
    let repository = RepositoryAccess::open(&root).expect("open repository");

    let fresh_a = repository.read_source("a.rs").expect("read a");
    let old_z = repository.read_source("z.rs").expect("read old z");
    let old_z_fingerprint = source_content_fingerprint(&old_z.text);
    std::fs::write(root.join("z.rs"), "fn bravo() {}\n").expect("replace z");
    let replacement_z = repository.read_source("z.rs").expect("read replacement z");
    assert_ne!(
        source_content_fingerprint(&replacement_z.text),
        old_z_fingerprint
    );

    let hits = vec![
        SearchHit {
            relative_path: "a.rs".to_string(),
            start_line: 1,
            end_line: 1,
            excerpt: "fn alpha() {}".to_string(),
            score: 2.0,
            source_stamp: Some(fresh_a.stamp),
            source_fingerprint: Some(source_content_fingerprint(&fresh_a.text)),
        },
        SearchHit {
            relative_path: "z.rs".to_string(),
            start_line: 1,
            end_line: 1,
            excerpt: "fn alpha() {}".to_string(),
            score: 1.0,
            source_stamp: Some(replacement_z.stamp),
            source_fingerprint: Some(old_z_fingerprint),
        },
    ];

    let map = repository
        .map_from_hits(&normalized_query("alpha"), &hits, 1, None)
        .expect("bounded map");
    assert!(map.truncated);
    assert_eq!(map.invalidated_evidence_paths, vec!["z.rs"]);
    assert!(
        map.entries
            .iter()
            .any(|entry| entry.relative_path == "a.rs")
    );
}

#[test]
fn shared_start_time_can_expire_structural_mapping_before_it_restarts_a_budget() {
    let root = temp_root("shared-deadline");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("main.rs"), "fn alpha() {}\n").expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![SearchHit {
        relative_path: "main.rs".to_string(),
        start_line: 1,
        end_line: 1,
        excerpt: "fn alpha() {}".to_string(),
        score: 1.0,
        source_stamp: None,
        source_fingerprint: None,
    }];
    let started = Instant::now()
        .checked_sub(MAX_SEARCH_WALL_TIME)
        .expect("representable deadline");

    let outcome = repository
        .map_from_hits_since(&normalized_query("alpha"), &hits, 1, None, &started)
        .expect("bounded map");
    assert!(outcome.truncated);
    assert!(outcome.entries.is_empty());
}
