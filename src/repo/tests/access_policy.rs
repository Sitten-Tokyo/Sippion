use super::*;

#[cfg(unix)]
#[test]
fn replaced_root_path_is_rejected_before_ambient_discovery() {
    let root = temp_root("root-identity");
    let moved = TestRoot::new(root.with_extension("moved"));
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("old.rs"), "fn original_root() {}\n").expect("write old");
    let repository = RepositoryAccess::open(&root).expect("open repository");

    std::fs::rename(&root, &moved).expect("move original root");
    std::fs::create_dir_all(&root).expect("replace root path");
    std::fs::write(root.join("new.rs"), "fn replacement_root() {}\n").expect("write new");

    let error = repository
        .search(&normalized_query("replacement_root"), 8, None)
        .expect_err("replacement must be rejected");
    assert_eq!(error, RepositoryAccessError::ConcurrentModification);
}

#[test]
fn parent_escape_is_rejected() {
    assert_eq!(
        normalize_relative(Path::new("../secret")),
        Err(RepositoryAccessError::InvalidRelativePath)
    );
}

#[test]
fn control_character_path_is_rejected() {
    assert_eq!(
        normalize_relative(Path::new("src/x\nFAKE.rs")),
        Err(RepositoryAccessError::InvalidRelativePath)
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_path_is_rejected_without_lossy_aliasing() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let invalid = PathBuf::from("src").join(OsString::from_vec(vec![0xff, b'.', b'r', b's']));
    assert_eq!(
        normalize_relative(&invalid),
        Err(RepositoryAccessError::NonUtf8Path)
    );
    assert_eq!(path_parts(&invalid), None);
}

#[cfg(windows)]
#[test]
fn non_unicode_windows_path_is_rejected_without_lossy_aliasing() {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    let invalid = PathBuf::from("src").join(OsString::from_wide(&[0xd800]));
    assert_eq!(
        normalize_relative(&invalid),
        Err(RepositoryAccessError::NonUtf8Path)
    );
    assert_eq!(path_parts(&invalid), None);
}

#[cfg(target_os = "linux")]
#[test]
fn discovery_marks_non_utf8_paths_incomplete_instead_of_collapsing_them() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = temp_root("non-utf8-path");
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("safe.rs"), "fn safe() {}\n").expect("write safe file");
    let invalid_name = OsString::from_vec(vec![b'b', b'a', b'd', 0xff, b'.', b'r', b's']);
    std::fs::write(root.join(invalid_name), "fn hidden() {}\n").expect("write non-utf8 file");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let started = Instant::now();
    let outcome = repository
        .discover_files(None, &started, &HashMap::new())
        .expect("discover files");

    assert!(outcome.truncated);
    assert_eq!(outcome.files.len(), 1);
    assert_eq!(outcome.files[0].path, "safe.rs");
}

#[test]
fn environment_files_are_denied_except_templates() {
    assert!(is_denied(Path::new(".env.production")));
    assert!(!is_denied(Path::new(".env.example")));
    assert!(is_denied(Path::new("terraform.tfstate")));
}

#[test]
fn ignored_subtree_prevents_repository_wide_no_match_claim() {
    let root = temp_root("gitignore-completeness");
    std::fs::create_dir_all(root.join("generated")).expect("generated dir");
    std::fs::write(root.join(".gitignore"), "generated/\n").expect("gitignore");
    std::fs::write(
        root.join("generated/ignored.rs"),
        "fn ignored_sentinel_7b19d4() {}\n",
    )
    .expect("ignored source");
    std::fs::write(root.join("visible.rs"), "fn visible() {}\n").expect("visible source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("ignored_sentinel_7b19d4"), 8, None)
        .expect("search succeeds");

    assert!(outcome.hits.is_empty(), "gitignored source must remain uninspected");
    assert!(outcome.coverage.policy_excluded_files >= 1);
    assert!(!outcome.truncated);
}

#[test]
fn spaces_and_unicode_in_directories_and_file_names_are_preserved() {
    let root = temp_root("unicode and spaces");
    let nested = root.join("project 日本語").join("src with spaces");
    let source_path = nested.join("認証 handler.rs");
    std::fs::create_dir_all(&nested).expect("nested directory");
    std::fs::write(&source_path, "fn unicode_path_marker() {}\n").expect("source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("unicode_path_marker"), 8, None)
        .expect("search succeeds");
    let expected = "project 日本語/src with spaces/認証 handler.rs";
    assert!(outcome.hits.iter().any(|hit| hit.relative_path == expected));
    let source = repository.read_source(expected).expect("read source");
    assert!(source.text.contains("unicode_path_marker"));
}

#[test]
fn lf_and_crlf_sources_are_read_and_searchable() {
    let root = temp_root("line-endings");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("lf.rs"), "fn lf_marker() {}\nsecond line\n").expect("LF source");
    std::fs::write(root.join("crlf.rs"), b"fn crlf_marker() {}\r\nsecond line\r\n").expect("CRLF source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    for marker in ["lf_marker", "crlf_marker"] {
        let outcome = repository.search(&normalized_query(marker), 8, None).expect("search succeeds");
        assert!(outcome.hits.iter().any(|hit| hit.relative_path.ends_with(".rs")));
    }
    let crlf = repository.read_source("crlf.rs").expect("read CRLF source");
    assert!(crlf.text.contains("\r\n"));
}

#[test]
#[allow(clippy::permissions_set_readonly_false)]
fn read_only_file_remains_readable_without_write_authority() {
    let root = temp_root("read-only");
    std::fs::create_dir_all(&root).expect("root");
    let path = root.join("read-only.rs");
    std::fs::write(&path, "fn read_only_marker() {}\n").expect("source");
    let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).expect("set read-only");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("read-only.rs").expect("read-only source remains readable");
    assert!(source.text.contains("read_only_marker"));

    let mut writable = std::fs::metadata(&path).expect("metadata").permissions();
    writable.set_readonly(false);
    std::fs::set_permissions(&path, writable).expect("restore permissions");
}

#[cfg(windows)]
#[test]
fn windows_relative_paths_normalize_backslashes_and_reject_absolute_paths() {
    assert_eq!(normalize_relative(Path::new(r"src\日本語\file.rs")), Ok("src/日本語/file.rs".to_string()));
    assert_eq!(normalize_relative(Path::new(r"C:\project\file.rs")), Err(RepositoryAccessError::InvalidRelativePath));
    assert_eq!(normalize_relative(Path::new(r"\\server\share\file.rs")), Err(RepositoryAccessError::InvalidRelativePath));
}
