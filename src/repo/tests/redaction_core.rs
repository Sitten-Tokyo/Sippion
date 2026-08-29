use super::*;

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
    assert!(outcome.truncated, "expanded redaction must report truncation");
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
        relative_path: "danger.rs".to_string(), start_line: 1, end_line: 1,
        excerpt: String::new(), score: 1.0, source_stamp: None, source_fingerprint: None,
    }];
    let outcome = repository.map_from_hits(&normalized_query("token"), &hits, 1, None).expect("map succeeds");
    assert!(outcome.truncated);
}

#[test]
fn redacted_secret_match_returns_suppressed_evidence_instead_of_no_match() {
    let root = temp_root("redacted-match");
    std::fs::create_dir_all(&root).expect("root");
    let secret = "sk-abcdefghijklmnopqrstuvwxyz0123456789";
    std::fs::write(root.join("safe.rs"), format!("let token = \"{secret}\";\n")).expect("source");
    let repository = RepositoryAccess::open(&root).expect("open repository");
    let outcome = repository.search(&normalized_query(secret), 8, None).expect("search succeeds");
    let hit = outcome.hits.iter().find(|hit| hit.relative_path == "safe.rs").expect("redacted match");
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
        "Authorization: Basic YTpi", "Authorization: Bearer x",
        "Proxy-Authorization: Basic YQ==", "curl -H 'Authorization: Bearer abc' https://example.test",
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
        "OPENAI_API_KEY =\n  \"abc\"\nafter = safe", "DATABASE_PASSWORD =\n  \"x\"\nafter = safe",
        "AWS_SECRET_ACCESS_KEY =\n  \"short\"\nafter = safe", "SESSION_TOKEN =\n  \"xyz\"\nafter = safe",
    ] {
        let redacted = redact_high_confidence_secrets(input);
        assert!(redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
        assert!(redacted.contains("after = safe"));
        assert_eq!(input.lines().count(), redacted.lines().count());
    }
    let block = "OPENAI_API_KEY: |\n  first-secret-line\n  second-secret-line\nafter: safe";
    let block_redacted = redact_high_confidence_secrets(block);
    assert!(block_redacted.contains("SIPPION_REDACTED_MULTILINE_LITERAL"));
    assert!(!block_redacted.contains("first-secret-line"));
    assert!(!block_redacted.contains("second-secret-line"));
    assert!(block_redacted.ends_with("after: safe"));
}

#[test]
fn multiline_sensitive_value_allows_comments_but_preserves_computed_and_nested_values() {
    let commented = "token:\n  # loaded below\n  very-secret-token\nnext: safe";
    let redacted = redact_high_confidence_secrets(commented);
    assert!(!redacted.contains("very-secret-token"));
    assert!(redacted.contains("# loaded below"));
    for input in ["password:\n  ${PASSWORD_FROM_ENV}\nnext: safe", "password:\n  type: string\nnext: safe"] {
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
    assert_eq!(redact_high_confidence_secrets(jwt), "[SIPPION_REDACTED_JWT]");
    let cookie = "curl -H 'Cookie: session=abcdef0123456789; theme=dark' https://example.test";
    let cookie_redacted = redact_high_confidence_secrets(cookie);
    assert!(cookie_redacted.contains("Cookie: [SIPPION_REDACTED_COOKIE]"));
    assert!(!cookie_redacted.contains("abcdef0123456789"));
    let session = r#"session_id = "abcdef0123456789""#;
    let session_redacted = redact_high_confidence_secrets(session);
    assert!(session_redacted.contains(r#"session_id = "[SIPPION_REDACTED_LITERAL]""#));
    let url = r#"let endpoint = "postgres://alice:correct-horse-battery@example.test/app";"#;
    let url_redacted = redact_high_confidence_secrets(url);
    assert!(url_redacted.contains("postgres://[SIPPION_REDACTED_URL_CREDENTIAL]@example.test/app"));
}

#[test]
fn auth_placeholders_and_urls_without_passwords_are_preserved() {
    for input in [
        "Authorization: Bearer {token}", "Authorization: Bearer ${TOKEN}", "Authorization: Bearer <token>",
        "use Bearer token in documentation", "https://example.test/path", "https://alice@example.test/path",
    ] {
        assert_eq!(redact_high_confidence_secrets(input), input);
    }
    let explicit_bare_word = redact_high_confidence_secrets("Authorization: Bearer token");
    assert!(explicit_bare_word.contains("SIPPION_REDACTED_AUTH_CREDENTIAL"));
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
}

#[test]
fn later_equals_cannot_steal_an_earlier_colon_literal() {
    let input = r#"password: "abcdefgh", token="ijklmnop""#;
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("abcdefgh"));
    assert!(!redacted.contains("ijklmnop"));
}

#[test]
fn multiline_private_key_key_does_not_disable_whole_block_redaction() {
    let input = concat!("private_key:\n", "  -----BEGIN PRIVATE KEY-----\n", "  SECRET-KEY-MATERIAL\n", "  -----END PRIVATE KEY-----\n", "after: safe");
    let redacted = redact_high_confidence_secrets(input);
    assert!(redacted.contains("SIPPION_REDACTED_PRIVATE_KEY"));
    assert!(!redacted.contains("SECRET-KEY-MATERIAL"));
    assert_eq!(input.lines().count(), redacted.lines().count());
}

#[test]
fn whole_private_key_block_is_redacted() {
    let input = "-----BEGIN PRIVATE KEY-----\nABCDEF123456\n-----END PRIVATE KEY-----\ncode";
    let redacted = redact_high_confidence_secrets(input);
    assert!(!redacted.contains("ABCDEF123456"));
    assert!(redacted.ends_with("code"));
}
