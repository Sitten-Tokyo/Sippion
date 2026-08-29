use super::*;

#[cfg(unix)]
#[test]
fn fifo_replacement_is_rejected_without_blocking() {
    use std::process::Command;
    use std::sync::mpsc;
    let root = temp_root("fifo-replacement");
    std::fs::create_dir_all(&root).expect("temp root");
    let path = root.join("victim.rs");
    std::fs::write(&path, "fn victim() {}\n").expect("write initial regular file");
    let repository = Arc::new(RepositoryAccess::open(&root).expect("open repository"));
    std::fs::remove_file(&path).expect("remove regular file");
    let status = match Command::new("mkfifo").arg(&path).status() {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("run mkfifo: {error}"),
    };
    assert!(status.success(), "mkfifo must succeed for FIFO regression test");
    let (tx, rx) = mpsc::channel();
    let worker_repository = Arc::clone(&repository);
    let worker = std::thread::spawn(move || { let _ = tx.send(worker_repository.read_source("victim.rs")); });
    let result = rx.recv_timeout(Duration::from_secs(1)).expect("FIFO open must not block");
    assert!(matches!(result, Err((RepositoryAccessError::NotRegularFile, 0))));
    worker.join().expect("FIFO read worker");
}

#[cfg(unix)]
#[test]
fn symlink_alias_cannot_bypass_denied_path() {
    use std::os::unix::fs::symlink;
    let root = temp_root("symlink-alias");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join(".env"), "not-a-real-secret").expect("write denied file");
    symlink(".env", root.join("safe.txt")).expect("create symlink");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    assert!(repository.read_source("safe.txt").is_err());
}

#[cfg(unix)]
#[test]
fn final_symlink_to_allowed_file_is_still_refused() {
    use std::os::unix::fs::symlink;
    let root = temp_root("final-link");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("real.rs"), "fn real() {}").expect("write real file");
    symlink("real.rs", root.join("alias.rs")).expect("create symlink");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    assert!(repository.read_source("alias.rs").is_err());
}

#[cfg(unix)]
#[test]
fn parent_directory_symlink_is_refused() {
    use std::os::unix::fs::symlink;
    let root = temp_root("parent-link");
    std::fs::create_dir_all(root.join("real")).expect("temp root");
    std::fs::write(root.join("real/file.rs"), "fn real() {}").expect("write real file");
    symlink("real", root.join("alias")).expect("create directory symlink");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    assert!(repository.read_source("alias/file.rs").is_err());
}

#[test]
fn regular_file_still_reads_after_nofollow_hardening() {
    let root = temp_root("regular");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("safe.rs"), "fn safe() {}").expect("write regular file");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("safe.rs").expect("read regular file");
    assert_eq!(source.text, "fn safe() {}");
}

#[cfg(unix)]
#[test]
fn hard_linked_source_is_denied_and_policy_excluded() {
    let root = temp_root("hardlink-root");
    let outside = temp_root("hardlink-outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    let outside_file = outside.join("secret.rs");
    std::fs::write(&outside_file, "fn outside_secret() {}\n").expect("write outside");
    std::fs::hard_link(&outside_file, root.join("looks_safe.rs")).expect("create hard link");
    std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let (error, _) = repository.read_source("looks_safe.rs").expect_err("hard-linked source denied");
    assert_eq!(error, RepositoryAccessError::HardLinkedFile);
    let outcome = repository.search(&normalized_query("definitely_missing"), 8, None).expect("search");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(outcome.coverage.indexed_files, outcome.coverage.eligible_files);
    assert_eq!(outcome.coverage.confidence_milli, 350);
    assert!(!outcome.truncated);
}

#[cfg(windows)]
#[test]
fn windows_hard_linked_source_is_denied_by_open_handle_information() {
    let root = temp_root("windows-hardlink-root");
    let outside = temp_root("windows-hardlink-outside");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::create_dir_all(&outside).expect("outside");
    let outside_file = outside.join("secret.rs");
    std::fs::write(&outside_file, "fn outside_secret() {}\n").expect("write outside");
    std::fs::hard_link(&outside_file, root.join("looks_safe.rs")).expect("create hard link");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let (error, _) = repository.read_source("looks_safe.rs").expect_err("hard-linked source denied");
    assert_eq!(error, RepositoryAccessError::HardLinkedFile);
}

#[cfg(unix)]
#[test]
fn source_stamp_detects_same_length_file_replacement() {
    let root = temp_root("stamp-replacement");
    std::fs::create_dir_all(&root).expect("root");
    let path = root.join("same.rs");
    let replacement = root.join("replacement.rs");
    std::fs::write(&path, "AAAA\n").expect("write original");
    let before = source_stamp(&std::fs::metadata(&path).expect("metadata before"));
    std::fs::write(&replacement, "BBBB\n").expect("write replacement");
    std::fs::rename(&replacement, &path).expect("replace same-length file");
    let after = source_stamp(&std::fs::metadata(&path).expect("metadata after"));
    assert_ne!(before, after);
    assert_eq!(before.len, after.len);
}

#[test]
fn reset_ram_index_discards_cached_documents_and_saturation() {
    let root = temp_root("reset-index");
    std::fs::create_dir_all(&root).expect("root");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    repository.insert_index_document("cached.rs".to_string(), build_indexed_document("old cached term", None)).expect("insert");
    {
        let mut index = repository.ram_index.lock().expect("index lock");
        index.saturated = true;
        assert!(!index.files.is_empty());
    }
    repository.reset_ram_index().expect("reset index");
    let index = repository.ram_index.lock().expect("index lock");
    assert!(index.files.is_empty());
    assert_eq!(index.total_entries, 0);
    assert!(!index.saturated);
}

#[cfg(windows)]
#[test]
fn windows_search_rebuilds_a_stale_same_stamp_ram_index() {
    let root = temp_root("windows-stale-index");
    std::fs::create_dir_all(&root).expect("root");
    let path = root.join("same.rs");
    std::fs::write(&path, "fresh_unique_term\n").expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let stamp = source_stamp(&std::fs::metadata(&path).expect("metadata"));
    repository.insert_index_document("same.rs".to_string(), build_indexed_document("stale_unique_term\n", Some(stamp))).expect("seed stale");
    let outcome = repository.search(&normalized_query("fresh_unique_term"), 8, None).expect("search");
    assert!(outcome.hits.iter().any(|hit| hit.relative_path == "same.rs"));
}

#[cfg(windows)]
#[test]
fn windows_map_discards_stale_same_stamp_structural_caches() {
    let root = temp_root("windows-stale-structural-cache");
    std::fs::create_dir_all(&root).expect("root");
    let path = root.join("same.rs");
    std::fs::write(&path, "pub fn fresh_symbol() -> bool { true }\n").expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let stamp = source_stamp(&std::fs::metadata(&path).expect("metadata"));
    {
        let mut analysis = repository.analysis_cache.lock().expect("analysis cache");
        analysis.entries.insert("same.rs".to_string(), CachedAnalysis {
            stamp: stamp.clone(), symbols: vec![CachedRepoMapSymbol { name: "stale_symbol".to_string(), kind: "function".to_string(), line: 1 }],
            semantics: SemanticFacts::default(), cacheable: true, last_used: 1,
        });
    }
    let stale_graph_key = GraphCacheKey(vec![GraphCacheNode { path: "same.rs".to_string(), stamp }]);
    {
        let mut graph = repository.graph_cache.lock().expect("graph cache");
        graph.entries.insert(stale_graph_key, CachedGraph { edge_maps: vec![HashMap::new()], centrality: vec![999.0], last_used: 1 });
    }
    let hits = vec![SearchHit { relative_path: "same.rs".to_string(), start_line: 1, end_line: 1, excerpt: "fresh_symbol".to_string(), score: 1.0, source_stamp: None, source_fingerprint: None }];
    let outcome = repository.map_from_hits(&normalized_query("fresh_symbol"), &hits, 1, None).expect("map");
    let entry = outcome.entries.first().expect("map entry");
    assert!(entry.symbols.iter().any(|symbol| symbol.name == "fresh_symbol"));
    assert!(!entry.symbols.iter().any(|symbol| symbol.name == "stale_symbol"));
    assert!(entry.score < 100.0, "stale graph centrality must not be reused");
}
