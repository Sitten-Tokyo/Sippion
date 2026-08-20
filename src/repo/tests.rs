use super::*;

fn normalized_query(q: &str) -> NormalizedQuery {
    crate::core::McpToolInput {
        q: q.to_string(),
        ..Default::default()
    }
    .normalize()
    .expect("valid test query")
}

struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_root(label: &str) -> TestRoot {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    TestRoot::new(std::env::temp_dir().join(format!("sippion-{label}-{nonce}")))
}

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

    // Same-length replacement models the Windows case where size + mtime can fail to expose a
    // rewrite. Force the hit stamp to the new stamp so this test specifically exercises the
    // content fingerprint guard rather than the metadata guard.
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
            // Model the Windows same-size/same-mtime gap by making metadata appear current
            // while retaining the fingerprint from the old content generation.
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

    assert!(
        outcome.hits.is_empty(),
        "gitignored source must remain uninspected"
    );
    assert!(
        outcome.coverage.policy_excluded_files >= 1,
        "ignore rules must prevent an absolute repository-wide NO_MATCH"
    );
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
    std::fs::write(
        root.join("crlf.rs"),
        b"fn crlf_marker() {}\r\nsecond line\r\n",
    )
    .expect("CRLF source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    for marker in ["lf_marker", "crlf_marker"] {
        let outcome = repository
            .search(&normalized_query(marker), 8, None)
            .expect("search succeeds");
        assert!(
            outcome
                .hits
                .iter()
                .any(|hit| hit.relative_path.ends_with(".rs"))
        );
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
    let source = repository
        .read_source("read-only.rs")
        .expect("read-only source remains readable");
    assert!(source.text.contains("read_only_marker"));

    // Windows refuses to remove a read-only file until its attribute is restored.
    let mut writable = std::fs::metadata(&path).expect("metadata").permissions();
    writable.set_readonly(false);
    std::fs::set_permissions(&path, writable).expect("restore permissions");
}

#[cfg(windows)]
#[test]
fn windows_relative_paths_normalize_backslashes_and_reject_absolute_paths() {
    assert_eq!(
        normalize_relative(Path::new(r"src\日本語\file.rs")),
        Ok("src/日本語/file.rs".to_string())
    );
    assert_eq!(
        normalize_relative(Path::new(r"C:\project\file.rs")),
        Err(RepositoryAccessError::InvalidRelativePath)
    );
    assert_eq!(
        normalize_relative(Path::new(r"\\server\share\file.rs")),
        Err(RepositoryAccessError::InvalidRelativePath)
    );
}

#[test]
fn token_redaction_preserves_surrounding_code() {
    let input = "let token = \"sk-abcdefghijklmnopqrstuvwxyz0123456789\";";
    let redacted = redact_high_confidence_secrets(input);
    assert_eq!(redacted, "let token = \"[SIPPION_REDACTED_TOKEN]\";");
}

#[test]
fn bounded_redaction_caps_amplification_from_many_short_literals() {
    let input = "token=\"x\";\n".repeat(4_096);
    let limit = input.len();
    let outcome = redact_high_confidence_secrets_bounded(&input, limit);

    assert!(
        outcome.truncated,
        "expanded redaction must report truncation"
    );
    assert!(outcome.text.len() <= limit);
    assert!(!outcome.text.contains("token=\"x\""));
}

#[test]
fn bounded_redaction_suppresses_oversize_minified_line_before_expansion() {
    let repeats = MAX_BOUNDED_REDACTION_LINE_BYTES / "token=\"x\";".len() + 2;
    let input = "token=\"x\";".repeat(repeats);
    assert!(input.len() > MAX_BOUNDED_REDACTION_LINE_BYTES);

    let outcome = redact_high_confidence_secrets_bounded(&input, MAX_SOURCE_BYTES);

    assert!(outcome.truncated);
    assert_eq!(outcome.text, REDACTED_OVERSIZE_LINE);
    assert!(outcome.text.len() <= MAX_SOURCE_BYTES);
}

#[test]
fn repository_map_reports_truncation_for_oversize_redaction_line() {
    let root = temp_root("map-redaction-bound");
    std::fs::create_dir_all(&root).expect("root");
    let repeats = MAX_BOUNDED_REDACTION_LINE_BYTES / "token=\"x\";".len() + 2;
    std::fs::write(root.join("danger.rs"), "token=\"x\";".repeat(repeats)).expect("source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![SearchHit {
        relative_path: "danger.rs".to_string(),
        start_line: 1,
        end_line: 1,
        excerpt: String::new(),
        score: 1.0,
        source_stamp: None,
        source_fingerprint: None,
    }];
    let outcome = repository
        .map_from_hits(&normalized_query("token"), &hits, 1, None)
        .expect("map succeeds");

    assert!(outcome.truncated);
}

#[test]
fn redacted_secret_match_returns_suppressed_evidence_instead_of_no_match() {
    let root = temp_root("redacted-match");
    std::fs::create_dir_all(&root).expect("root");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    std::fs::write(root.join("safe.rs"), format!("let token = \"{secret}\";\n")).expect("source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query(secret), 8, None)
        .expect("search succeeds");
    let hit = outcome
        .hits
        .iter()
        .find(|hit| hit.relative_path == "safe.rs")
        .expect("redacted match must still be represented");

    assert_eq!(hit.start_line, 0);
    assert_eq!(hit.end_line, 0);
    assert_eq!(hit.excerpt, REDACTED_MATCH_EXCERPT);
    assert!(!hit.excerpt.contains(secret));
}

#[test]
fn ordinary_auth_assignment_is_not_destroyed() {
    let input = "let password = config.password.clone();";
    assert_eq!(redact_high_confidence_secrets(input), input);
}
#[test]
fn authorization_bearer_and_basic_credentials_are_redacted() {
    let bearer = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789";
    let basic = "Proxy-Authorization: Basic dXNlcjpwYXNzd29yZA==";

    let bearer_redacted = redact_high_confidence_secrets(bearer);
    assert!(bearer_redacted.contains("Authorization: Bearer "));
    assert!(bearer_redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
    assert!(!bearer_redacted.contains("abcdefghijklmnopqrstuvwxyz0123456789"));

    let basic_redacted = redact_high_confidence_secrets(basic);
    assert!(basic_redacted.contains("Proxy-Authorization: Basic "));
    assert!(basic_redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
    assert!(!basic_redacted.contains("dXNlcjpwYXNzd29yZA=="));
}

#[test]
fn short_explicit_authorization_credentials_are_redacted() {
    for input in [
        "Authorization: Basic YTpi",
        "Authorization: Bearer x",
        "Proxy-Authorization: Basic YQ==",
        "curl -H 'Authorization: Bearer abc' https://example.test",
    ] {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
        assert!(!redacted.ends_with("Bearer x"));
        assert!(!redacted.contains("Basic YTpi"));
        assert!(!redacted.contains("Basic YQ=="));
        assert!(!redacted.contains("Bearer abc'"));
    }
}

#[test]
fn multiline_sensitive_scalars_are_redacted() {
    let yaml = "password:\n  correct-horse-battery\nnext: safe";
    let yaml_redacted = redact_high_confidence_secrets(yaml);
    assert!(yaml_redacted.contains("password:"));
    assert!(yaml_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!yaml_redacted.contains("correct-horse-battery"));
    assert_eq!(yaml.lines().count(), yaml_redacted.lines().count());

    let json = "{\n  \"password\":\n    \"abc\",\n  \"safe\": true\n}";
    let json_redacted = redact_high_confidence_secrets(json);
    assert!(json_redacted.contains("\"password\":"));
    assert!(json_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!json_redacted.contains("\"abc\""));
    assert_eq!(json.lines().count(), json_redacted.lines().count());

    let compact_json = "{\"password\":\n\"xyz\"}";
    let compact_redacted = redact_high_confidence_secrets(compact_json);
    assert!(compact_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!compact_redacted.contains("\"xyz\""));
}

#[test]
fn prefixed_multiline_sensitive_keys_are_redacted() {
    for input in [
        "OPENAI_API_KEY =\n  \"abc\"\nafter = safe",
        "DATABASE_PASSWORD =\n  \"x\"\nafter = safe",
        "AWS_SECRET_ACCESS_KEY =\n  \"short\"\nafter = safe",
        "SESSION_TOKEN =\n  \"xyz\"\nafter = safe",
    ] {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(!redacted.contains("\"abc\""));
        assert!(!redacted.contains("\"x\""));
        assert!(!redacted.contains("\"short\""));
        assert!(!redacted.contains("\"xyz\""));
        assert!(redacted.contains("after = safe"));
        assert_eq!(input.lines().count(), redacted.lines().count());
    }

    let block = "OPENAI_API_KEY: |\n  first-secret-line\n  second-secret-line\nafter: safe";
    let block_redacted = redact_high_confidence_secrets(block);
    assert!(block_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!block_redacted.contains("first-secret-line"));
    assert!(!block_redacted.contains("second-secret-line"));
    assert!(block_redacted.ends_with("after: safe"));
    assert_eq!(block.lines().count(), block_redacted.lines().count());
}

#[test]
fn multiline_sensitive_value_allows_comments_but_preserves_computed_and_nested_values() {
    let commented = "token:\n  # loaded below\n  very-secret-token\nnext: safe";
    let redacted = redact_high_confidence_secrets(commented);
    assert!(!redacted.contains("very-secret-token"));
    assert!(redacted.contains("# loaded below"));

    for input in [
        "password:\n  ${PASSWORD_FROM_ENV}\nnext: safe",
        "password:\n  type: string\nnext: safe",
    ] {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
}

#[test]
fn yaml_sensitive_block_scalars_are_suppressed_with_line_count_preserved() {
    let input = "secret: |\n  first-secret-line\n  second-secret-line\nafter: safe";
    let redacted = redact_high_confidence_secrets(input);
    assert!(redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!redacted.contains("first-secret-line"));
    assert!(!redacted.contains("second-secret-line"));
    assert!(redacted.ends_with("after: safe"));
    assert_eq!(input.lines().count(), redacted.lines().count());
}

#[test]
fn jwt_cookie_session_and_url_credentials_are_redacted() {
    let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJlMTIzNDU2";
    let jwt_redacted = redact_high_confidence_secrets(jwt);
    assert_eq!(jwt_redacted, "[SIPPION_REDACTED_JWT]");

    let cookie = "curl -H 'Cookie: session=abcdef0123456789; theme=dark' https://example.test";
    let cookie_redacted = redact_high_confidence_secrets(cookie);
    assert!(cookie_redacted.contains("Cookie: [SIPPION_REDACTED_COOKIE]"));
    assert!(cookie_redacted.ends_with("' https://example.test"));
    assert!(!cookie_redacted.contains("abcdef0123456789"));

    let session = r#"session_id = "abcdef0123456789""#;
    let session_redacted = redact_high_confidence_secrets(session);
    assert!(session_redacted.contains(r#"session_id = "[SIPPION_REDACTED_LITERAL]""#));
    assert!(!session_redacted.contains("abcdef0123456789"));

    let url = r#"let endpoint = "postgres://alice:correct-horse-battery@example.test/app";"#;
    let url_redacted = redact_high_confidence_secrets(url);
    assert!(url_redacted.contains("postgres://[SIPPION_REDACTED_URL_CREDENTIAL]@example.test/app"));
    assert!(!url_redacted.contains("correct-horse-battery"));
}

#[test]
fn auth_placeholders_and_urls_without_passwords_are_preserved() {
    for input in [
        "Authorization: Bearer {token}",
        "Authorization: Bearer ${TOKEN}",
        "Authorization: Bearer <token>",
        "use Bearer token in documentation",
        "https://example.test/path",
        "https://alice@example.test/path",
    ] {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }

    let explicit_bare_word = redact_high_confidence_secrets("Authorization: Bearer token");
    assert!(explicit_bare_word.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
    assert!(!explicit_bare_word.ends_with("Bearer token"));
}

#[test]
fn every_sensitive_literal_on_one_line_is_redacted() {
    let input = r#"{"password":"abcdefgh","token":"ijklmnop","api_key":"qrstuvwx"}"#;
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("abcdefgh"));
    assert!(!redacted.contains("ijklmnop"));
    assert!(!redacted.contains("qrstuvwx"));
    assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 3);
}

#[test]
fn computed_sensitive_value_does_not_hide_later_literal() {
    let input = r#"password=config.password.clone(), token="abcdefghijkl""#;
    let redacted = redact_high_confidence_secrets(input);
    assert!(redacted.contains("config.password.clone()"));
    assert!(!redacted.contains("abcdefghijkl"));
    assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 1);
}

#[test]
fn later_equals_cannot_steal_an_earlier_colon_literal() {
    let input = r#"password: "abcdefgh", token="ijklmnop""#;
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("abcdefgh"));
    assert!(!redacted.contains("ijklmnop"));
    assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 2);
}

#[test]
fn multiline_private_key_key_does_not_disable_whole_block_redaction() {
    let input = concat!(
        "private_key:\n",
        "  -----BEGIN PRIVATE KEY-----\n",
        "  SECRET-KEY-MATERIAL\n",
        "  -----END PRIVATE KEY-----\n",
        "after: safe",
    );
    let redacted = redact_high_confidence_secrets(input);
    assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
    assert!(!redacted.contains("SECRET-KEY-MATERIAL"));
    assert!(redacted.ends_with("after: safe"));
    assert_eq!(input.lines().count(), redacted.lines().count());
}

#[test]
fn whole_private_key_block_is_redacted() {
    let input = "-----BEGIN PRIVATE KEY-----\nABCDEF123456\n-----END PRIVATE KEY-----\ncode";
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("ABCDEF123456"));
    assert!(redacted.ends_with("code"));
}

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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return;
        }
        Err(error) => panic!("run mkfifo: {error}"),
    };
    assert!(
        status.success(),
        "mkfifo must succeed for FIFO regression test"
    );

    let (tx, rx) = mpsc::channel();
    let worker_repository = Arc::clone(&repository);
    let worker = std::thread::spawn(move || {
        let _ = tx.send(worker_repository.read_source("victim.rs"));
    });

    let result = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("FIFO open must not block waiting for a writer");
    assert!(matches!(
        result,
        Err((RepositoryAccessError::NotRegularFile, 0))
    ));
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
    let source = repository
        .read_source("safe.rs")
        .expect("read regular file");
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
    let (error, _) = repository
        .read_source("looks_safe.rs")
        .expect_err("hard-linked source must be denied");
    assert_eq!(error, RepositoryAccessError::HardLinkedFile);

    let outcome = repository
        .search(&normalized_query("definitely_missing"), 8, None)
        .expect("search succeeds");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(
        outcome.coverage.indexed_files,
        outcome.coverage.eligible_files
    );
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
    let (error, _) = repository
        .read_source("looks_safe.rs")
        .expect_err("hard-linked source must be denied on Windows");
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
    repository
        .insert_index_document(
            "cached.rs".to_string(),
            build_indexed_document("old cached term", None),
        )
        .expect("insert cached document");
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
    repository
        .insert_index_document(
            "same.rs".to_string(),
            build_indexed_document("stale_unique_term\n", Some(stamp)),
        )
        .expect("seed stale same-stamp index");

    let outcome = repository
        .search(&normalized_query("fresh_unique_term"), 8, None)
        .expect("search");
    assert!(
        outcome
            .hits
            .iter()
            .any(|hit| hit.relative_path == "same.rs")
    );
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
        analysis.entries.insert(
            "same.rs".to_string(),
            CachedAnalysis {
                stamp: stamp.clone(),
                symbols: vec![CachedRepoMapSymbol {
                    name: "stale_symbol".to_string(),
                    kind: "function".to_string(),
                    line: 1,
                }],
                semantics: SemanticFacts::default(),
                cacheable: true,
                last_used: 1,
            },
        );
    }
    let stale_graph_key = GraphCacheKey(vec![GraphCacheNode {
        path: "same.rs".to_string(),
        stamp,
    }]);
    {
        let mut graph = repository.graph_cache.lock().expect("graph cache");
        graph.entries.insert(
            stale_graph_key,
            CachedGraph {
                edge_maps: vec![HashMap::new()],
                centrality: vec![999.0],
                last_used: 1,
            },
        );
    }

    let hits = vec![SearchHit {
        relative_path: "same.rs".to_string(),
        start_line: 1,
        end_line: 1,
        excerpt: "fresh_symbol".to_string(),
        score: 1.0,
        source_stamp: None,
        source_fingerprint: None,
    }];
    let outcome = repository
        .map_from_hits(&normalized_query("fresh_symbol"), &hits, 1, None)
        .expect("map");
    let entry = outcome.entries.first().expect("map entry");
    assert!(
        entry
            .symbols
            .iter()
            .any(|symbol| symbol.name == "fresh_symbol")
    );
    assert!(
        !entry
            .symbols
            .iter()
            .any(|symbol| symbol.name == "stale_symbol")
    );
    assert!(
        entry.score < 100.0,
        "stale graph centrality must not be reused"
    );
}

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
    let (error, consumed) = repository
        .read_source("binary.bin")
        .expect_err("binary must not become model text");
    assert_eq!(error, RepositoryAccessError::NonUtf8Source);
    assert_eq!(consumed, bytes.len());
}

#[test]
fn file_local_best_hit_prefers_higher_score_then_earlier_line() {
    let earlier = SearchHit {
        relative_path: "a.rs".into(),
        start_line: 2,
        end_line: 2,
        excerpt: "earlier".into(),
        score: 10.0,
        source_stamp: None,
        source_fingerprint: None,
    };
    let later = SearchHit {
        start_line: 20,
        ..earlier.clone()
    };
    let higher = SearchHit {
        score: 11.0,
        ..later.clone()
    };
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
    std::fs::write(
        root.join("a_relevant.rs"),
        "fn check() {\n    // authentication\n    let token = load();\n    // validation\n}\n",
    )
    .expect("write relevant source");
    std::fs::write(root.join("z_noise.rs"), "let token = load();\n").expect("write noise source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(
            &normalized_query("authentication token validation"),
            8,
            None,
        )
        .expect("search succeeds");
    assert_eq!(
        outcome.hits.first().map(|hit| hit.relative_path.as_str()),
        Some("a_relevant.rs")
    );
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
    assert!(!read_failure_makes_scan_incomplete(
        &RepositoryAccessError::NonUtf8Source
    ));
    assert!(read_failure_makes_scan_incomplete(
        &RepositoryAccessError::TooLarge
    ));
    assert!(!read_failure_makes_scan_incomplete(
        &RepositoryAccessError::DeniedPath
    ));
}

#[test]
fn structural_map_links_symbol_references_with_multi_pattern_matcher() {
    let root = temp_root("structural-aho");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n")
        .expect("write caller");
    std::fs::write(
        root.join("auth.rs"),
        "pub fn authenticate() -> bool { true }\n",
    )
    .expect("write auth");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![
        SearchHit {
            relative_path: "caller.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "authenticate".into(),
            score: 10.0,
            source_stamp: None,
            source_fingerprint: None,
        },
        SearchHit {
            relative_path: "auth.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "authenticate".into(),
            score: 9.0,
            source_stamp: None,
            source_fingerprint: None,
        },
    ];
    let map = repository
        .map_from_hits(&normalized_query("authenticate"), &hits, 2, None)
        .expect("map succeeds");
    let caller = map
        .entries
        .iter()
        .find(|entry| entry.relative_path == "caller.rs")
        .expect("caller entry");
    assert!(caller.links_to.iter().any(|path| path == "auth.rs"));
    assert!(
        caller
            .semantic_links
            .iter()
            .any(|link| { link.relative_path == "auth.rs" && link.weight >= 0.80 })
    );
}

#[test]
fn structural_analysis_and_graph_are_shared_across_repeated_calls() {
    let root = temp_root("shared-analysis-cache");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("caller.rs"), "fn handle() { authenticate(); }\n")
        .expect("write caller");
    std::fs::write(
        root.join("auth.rs"),
        "pub fn authenticate() -> bool { true }\n",
    )
    .expect("write auth");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let hits = vec![
        SearchHit {
            relative_path: "caller.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "authenticate".into(),
            score: 10.0,
            source_stamp: None,
            source_fingerprint: None,
        },
        SearchHit {
            relative_path: "auth.rs".into(),
            start_line: 1,
            end_line: 1,
            excerpt: "authenticate".into(),
            score: 9.0,
            source_stamp: None,
            source_fingerprint: None,
        },
    ];
    let query = normalized_query("authenticate");
    repository
        .map_from_hits(&query, &hits, 2, None)
        .expect("first map");
    let analysis_entries = repository
        .analysis_cache
        .lock()
        .expect("analysis cache")
        .entries
        .len();
    let graph_entries = repository
        .graph_cache
        .lock()
        .expect("graph cache")
        .entries
        .len();
    assert_eq!(analysis_entries, 2);
    assert_eq!(graph_entries, 1);

    repository
        .map_from_hits(&query, &hits, 2, None)
        .expect("second map");
    assert_eq!(
        repository
            .analysis_cache
            .lock()
            .expect("analysis cache")
            .entries
            .len(),
        analysis_entries
    );
    assert_eq!(
        repository
            .graph_cache
            .lock()
            .expect("graph cache")
            .entries
            .len(),
        graph_entries
    );
}

#[test]
fn sibling_agent_memory_prefers_complementary_paths() {
    let root = temp_root("agent-diversity");
    std::fs::create_dir_all(&root).expect("temp root");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let query = normalized_query("authentication token");
    let hits = vec![SearchHit {
        relative_path: "src/auth.rs".into(),
        start_line: 1,
        end_line: 1,
        excerpt: "authentication token".into(),
        score: 10.0,
        source_stamp: None,
        source_fingerprint: None,
    }];
    let first = CoordinationContext {
        session_id: Some("bugfix-1".into()),
        agent_id: Some("agent-a".into()),
    };
    let sibling = CoordinationContext {
        session_id: Some("bugfix-1".into()),
        agent_id: Some("agent-b".into()),
    };
    repository.remember_search(&query, &hits, Some(&first));
    assert!(repository.memory_adjustment(&query.terms, "src/auth.rs", Some(&first)) > 0.0);
    assert!(repository.memory_adjustment(&query.terms, "src/auth.rs", Some(&sibling)) < 0.0);
}

#[test]
fn pre_cancelled_search_stops_before_discovery() {
    let root = temp_root("cancelled");
    std::fs::create_dir_all(&root).expect("temp root");
    std::fs::write(root.join("source.rs"), "fn target() {}\n").expect("write source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let cancelled = AtomicBool::new(true);

    let result = repository.search(&normalized_query("target helper"), 8, Some(&cancelled));
    assert_eq!(result, Err(RepositoryAccessError::Cancelled));
}

#[test]
fn high_confidence_aws_access_key_is_redacted_without_erasing_line() {
    let text = "access_key = AKIAABCDEFGHIJKLMNOP # fixture";
    let redacted = redact_high_confidence_secrets(text);
    assert_eq!(redacted, "access_key = [SIPPION_REDACTED_TOKEN] # fixture");
}

#[test]
fn sensitive_literal_assignments_are_redacted_without_erasing_the_key() {
    let cases = [
        (
            r#"password = "correct-horse-battery""#,
            "password",
            "correct-horse-battery",
        ),
        (
            r#""client_secret": "abcdefghijklmnop""#,
            "client_secret",
            "abcdefghijklmnop",
        ),
        (
            "api_token: abcdefghijklmnop",
            "api_token",
            "abcdefghijklmnop",
        ),
        (
            r#"AWS_SECRET_ACCESS_KEY = "abcdefghijklmnopqrstuvwx""#,
            "AWS_SECRET_ACCESS_KEY",
            "abcdefghijklmnopqrstuvwx",
        ),
        (
            r#"clientSecret = "abcdefghijklmnop""#,
            "clientSecret",
            "abcdefghijklmnop",
        ),
        (
            r#"DATABASE_URL = "postgres://user:password@example.test/db""#,
            "DATABASE_URL",
            "postgres://user:password@example.test/db",
        ),
    ];
    for (input, key, secret) in cases {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains(key));
        assert!(redacted.contains("SIPPION_REDACTED_LITERAL"));
        assert!(!redacted.contains(secret));
    }
}

#[test]
fn short_sensitive_literal_assignments_are_redacted() {
    let cases = [
        (r#"password = "x""#, "x"),
        ("token=abc", "abc"),
        (r#"client_secret='12'"#, "12"),
        ("api_key=q", "q"),
    ];

    for (input, secret) in cases {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_LITERAL"));
        assert!(!redacted.contains(secret));
    }
}

#[test]
fn empty_and_structural_sensitive_values_are_preserved() {
    for input in [
        r#"password = """#,
        "token=''",
        "api_key=false",
        "client_secret=null",
    ] {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
}

#[test]
fn unquoted_secret_literals_with_url_and_password_punctuation_are_redacted() {
    let cases = [
        (
            "password: p@ssw0rd!",
            "password: [SIPPION_REDACTED_LITERAL]",
        ),
        (
            "DATABASE_URL=postgres://user:password@example.test/db",
            "DATABASE_URL=[SIPPION_REDACTED_LITERAL]",
        ),
        ("token=abc#def!ghi", "token=[SIPPION_REDACTED_LITERAL]"),
    ];
    for (input, expected) in cases {
        assert_eq!(redact_high_confidence_secrets(input), expected);
    }
}

#[test]
fn type_annotation_does_not_hide_the_actual_secret_literal() {
    let input = r#"let password: SecretString = "correct-horse-battery";"#;
    let redacted = redact_high_confidence_secrets(input);
    assert!(redacted.contains("password: SecretString = "));
    assert!(!redacted.contains("correct-horse-battery"));
    assert_eq!(redacted.matches("SIPPION_REDACTED_LITERAL").count(), 1);
}

#[test]
fn computed_secret_references_with_calls_or_shell_expansion_are_not_destroyed() {
    let cases = [
        "let password = config.password.clone();",
        "let token = load_token_from_keychain();",
        "TOKEN=${TOKEN_FROM_KEYCHAIN}",
        "PASSWORD=$PASSWORD_FROM_ENV",
    ];
    for input in cases {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
}

#[test]
fn computed_secret_values_are_not_destroyed() {
    let input = "let token = load_token_from_keychain();";
    assert_eq!(redact_high_confidence_secrets(input), input);
}

#[test]
fn pgp_private_key_blocks_are_redacted() {
    let input = concat!(
        "-----BEGIN PGP PRIVATE KEY BLOCK-----\n",
        "super-secret-material\n",
        "-----END PGP PRIVATE KEY BLOCK-----\n",
        "after"
    );
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("super-secret-material"));
    assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
    assert!(redacted.ends_with("after"));
}

#[test]
fn oversized_source_is_policy_excluded_without_adaptive_retry() {
    let root = temp_root("oversized-policy");
    std::fs::create_dir_all(&root).expect("root");
    std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("write normal");
    std::fs::write(root.join("huge.rs"), vec![b'x'; MAX_SOURCE_BYTES + 1]).expect("write huge");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("definitely_missing"), 8, None)
        .expect("search succeeds");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(outcome.coverage.adaptive_rounds, 1);
    assert_eq!(
        outcome.coverage.indexed_files,
        outcome.coverage.eligible_files
    );
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
    let outcome = repository
        .search(&normalized_query("definitely_missing"), 8, None)
        .expect("search succeeds");
    assert_eq!(outcome.coverage.policy_excluded_files, 1);
    assert_eq!(outcome.coverage.adaptive_rounds, 1);
    assert_eq!(
        outcome.coverage.indexed_files,
        outcome.coverage.eligible_files
    );
    assert_eq!(outcome.coverage.confidence_milli, 350);
    assert!(!outcome.truncated);
}

#[test]
fn shared_analysis_cache_does_not_retain_source_line_signatures() {
    let root = temp_root("cache-structural-only");
    std::fs::create_dir_all(&root).expect("root");
    let sentinel = "CACHE_SOURCE_SENTINEL_9f4b2b";
    std::fs::write(
        root.join("safe.rs"),
        format!("fn visible() {{}} // {sentinel}\n"),
    )
    .expect("write source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("safe.rs").expect("read source");
    let analysis = repository
        .analyze_source_cached(
            "safe.rs",
            &source.text,
            &source.stamp,
            None,
            Instant::now() + Duration::from_secs(1),
        )
        .expect("analysis succeeds")
        .expect("analysis result");
    assert!(
        analysis
            .symbols
            .iter()
            .any(|symbol| symbol.name == "visible")
    );

    let cache = repository.analysis_cache.lock().expect("analysis cache");
    let cached_debug = format!("{:?}", cache.entries.get("safe.rs"));
    assert!(!cached_debug.contains(sentinel));
}

#[test]
fn candidate_generation_pruning_can_never_be_complete_no_match() {
    let root = temp_root("candidate-pruning-completeness");
    std::fs::create_dir_all(&root).expect("root");
    for index in 0..129 {
        std::fs::write(root.join(format!("candidate-{index:03}.rs")), "abc___bcd\n")
            .expect("write n-gram false positive");
    }

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("abcd"), 8, None)
        .expect("search succeeds");
    assert!(outcome.hits.is_empty());
    assert!(
        outcome.truncated,
        "candidate pruning must prevent complete NO_MATCH"
    );
    assert_eq!(
        outcome.coverage.adaptive_rounds, 1,
        "candidate-cap truncation alone must not waste scan-budget expansion rounds",
    );
}

#[test]
fn path_match_is_returned_when_body_has_no_query_term() {
    let root = temp_root("path-match");
    std::fs::create_dir_all(root.join("src/auth")).expect("temp root");
    std::fs::write(
        root.join("src/auth/middleware.rs"),
        "pub fn verify_request() -> bool { true }\n",
    )
    .expect("write source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository
        .search(&normalized_query("middleware gateway"), 8, None)
        .expect("search succeeds");
    assert_eq!(outcome.hits.len(), 1);
    assert_eq!(outcome.hits[0].relative_path, "src/auth/middleware.rs");
    assert!(outcome.hits[0].excerpt.is_empty());
    assert_eq!(
        (outcome.hits[0].start_line, outcome.hits[0].end_line),
        (0, 0)
    );
    assert_eq!(outcome.hits[0].score, 3.0);
}

#[test]
fn search_redacts_model_visible_excerpt_without_redacting_every_source_read() {
    let root = temp_root("excerpt-redaction");
    std::fs::create_dir_all(&root).expect("temp root");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    std::fs::write(
        root.join("auth.rs"),
        format!("const AUTH_TOKEN: &str = \"{secret}\";\n"),
    )
    .expect("write source");

    let repository = RepositoryAccess::open(&root).expect("open repository");
    let source = repository.read_source("auth.rs").expect("read source");
    assert!(source.text.contains(secret));

    let outcome = repository
        .search(&normalized_query("AUTH_TOKEN credential"), 8, None)
        .expect("search succeeds");
    assert_eq!(outcome.hits.len(), 1);
    assert!(!outcome.hits[0].excerpt.contains(secret));
    assert!(outcome.hits[0].excerpt.contains("SIPPION_REDACTED_TOKEN"));
}

#[test]
fn private_key_redaction_preserves_line_count_without_marker_amplification() {
    let input = concat!(
        "before\n",
        "-----BEGIN PRIVATE KEY-----\n",
        "a\n",
        "b\n",
        "-----END PRIVATE KEY-----\n",
        "after\n"
    );
    let redacted = redact_high_confidence_secrets(input);
    assert_eq!(redacted.lines().count(), input.lines().count());
    assert_eq!(redacted.matches("SIPPION_REDACTED_PRIVATE_KEY").count(), 1);
    assert!(!redacted.contains("\na\n"));
    assert!(!redacted.contains("\nb\n"));
    assert!(redacted.len() <= input.len() + 16);
}

#[test]
fn content_matches_always_rank_above_path_only_matches() {
    let content = SearchHit {
        relative_path: "src/implementation.rs".into(),
        start_line: 1,
        end_line: 1,
        excerpt: "token".into(),
        score: (CONTENT_MATCH_BASE_SCORE + 10) as f64,
        source_stamp: None,
        source_fingerprint: None,
    };
    let path_only = SearchHit {
        relative_path: "authentication/token/validation/middleware.rs".into(),
        start_line: 0,
        end_line: 0,
        excerpt: String::new(),
        score: (MAX_QUERY_TERMS * 3) as f64,
        source_stamp: None,
        source_fingerprint: None,
    };
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
    let outcome = repository
        .search(&normalized_query("pruned_only_marker"), 8, None)
        .expect("search succeeds");
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
    assert_eq!(
        second.coverage.indexed_files,
        second.coverage.eligible_files
    );
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
        outcome
            .hits
            .iter()
            .any(|hit| hit.relative_path == "auth.rs"),
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
