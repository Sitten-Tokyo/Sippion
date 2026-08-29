use super::*;

#[test]
fn sibling_agent_memory_prefers_complementary_paths() {
    let root = temp_root("agent-diversity");
    std::fs::create_dir_all(&root).expect("temp root");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let query = normalized_query("authentication token");
    let hits = vec![SearchHit { relative_path: "src/auth.rs".into(), start_line: 1, end_line: 1, excerpt: "authentication token".into(), score: 10.0, source_stamp: None, source_fingerprint: None }];
    let first = CoordinationContext { session_id: Some("bugfix-1".into()), agent_id: Some("agent-a".into()) };
    let sibling = CoordinationContext { session_id: Some("bugfix-1".into()), agent_id: Some("agent-b".into()) };
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
        (r#"password = "correct-horse-battery""#, "password", "correct-horse-battery"),
        (r#""client_secret": "abcdefghijklmnop""#, "client_secret", "abcdefghijklmnop"),
        ("api_token: abcdefghijklmnop", "api_token", "abcdefghijklmnop"),
        (r#"AWS_SECRET_ACCESS_KEY = "abcdefghijklmnopqrstuvwx""#, "AWS_SECRET_ACCESS_KEY", "abcdefghijklmnopqrstuvwx"),
        (r#"clientSecret = "abcdefghijklmnop""#, "clientSecret", "abcdefghijklmnop"),
        (r#"DATABASE_URL = "postgres://user:password@example.test/db""#, "DATABASE_URL", "postgres://user:password@example.test/db"),
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
    for (input, secret) in [(r#"password = "x""#, "x"), ("token=abc", "abc"), (r#"client_secret='12'"#, "12"), ("api_key=q", "q")] {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_LITERAL"));
        assert!(!redacted.contains(secret));
    }
}

#[test]
fn empty_and_structural_sensitive_values_are_preserved() {
    for input in [r#"password = """#, "token=''", "api_key=false", "client_secret=null"] {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
}

#[test]
fn unquoted_secret_literals_with_url_and_password_punctuation_are_redacted() {
    let cases = [
        ("password: p@ssw0rd!", "password: [SIPPION_REDACTED_LITERAL]"),
        ("DATABASE_URL=postgres://user:password@example.test/db", "DATABASE_URL=[SIPPION_REDACTED_LITERAL]"),
        ("token=abc#def!ghi", "token=[SIPPION_REDACTED_LITERAL]"),
    ];
    for (input, expected) in cases { assert_eq!(redact_high_confidence_secrets(input), expected); }
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
    for input in ["let password = config.password.clone();", "let token = load_token_from_keychain();", "TOKEN=${TOKEN_FROM_KEYCHAIN}", "PASSWORD=$PASSWORD_FROM_ENV"] {
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
    let input = concat!("-----BEGIN PGP PRIVATE KEY BLOCK-----\n", "super-secret-material\n", "-----END PGP PRIVATE KEY BLOCK-----\n", "after");
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("super-secret-material"));
    assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
    assert!(redacted.ends_with("after"));
}
