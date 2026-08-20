use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::McpToolInput;
use crate::repo::RepositoryAccess;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(label: &str) -> std::path::PathBuf {
    let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("sippion-redaction-corpus-{label}-{id}"));
    std::fs::create_dir_all(&path).expect("temp directory");
    path
}

fn provider_cases() -> Vec<(&'static str, String)> {
    vec![
        ("github-classic", format!("{}{}", "ghp_", "A".repeat(36))),
        (
            "github-fine-grained",
            format!("{}{}", "github_pat_", "A".repeat(40)),
        ),
        ("gitlab", format!("{}{}", "glpat-", "A".repeat(30))),
        (
            "slack-bot",
            format!("{}{}-{}-{}", "xoxb-", "1".repeat(12), "2".repeat(12), "A".repeat(24)),
        ),
        ("google", format!("{}{}", "AIza", "A".repeat(35))),
        ("aws-access", format!("{}{}", "AKIA", "A".repeat(16))),
        ("aws-session", format!("{}{}", "ASIA", "A".repeat(16))),
        ("openai-style", format!("{}{}", "sk-", "A".repeat(32))),
    ]
}

#[test]
fn provider_shaped_credentials_never_survive_model_visible_search_output() {
    for (label, secret) in provider_cases() {
        let root = temp_dir(label);
        let source = format!("pub const credential_marker: &str = \"{secret}\";\n");
        std::fs::write(root.join("credentials.rs"), source).expect("source");
        let repository = RepositoryAccess::open(&root).expect("repository");
        let query = McpToolInput {
            q: "credential_marker".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query");
        let output = repository.search(&query, 8, None).expect("search");
        assert!(!output.hits.is_empty(), "{label} must remain discoverable");
        let rendered = output
            .hits
            .iter()
            .map(|hit| hit.excerpt.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !rendered.contains(&secret),
            "{label} secret escaped redaction: {rendered}"
        );
        assert!(
            rendered.contains("SIPPION_REDACTED"),
            "{label} should leave an explicit redaction marker: {rendered}"
        );
        drop(repository);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}

#[test]
fn sensitive_assignment_values_are_redacted_but_short_placeholders_are_not_overmatched() {
    let root = temp_dir("assignment");
    let long_password = ["correct", "horse", "battery", "staple"].join("-");
    let short_placeholder = format!("{}{}", "ghp_", "example");
    let source = format!(
        "credential_marker = True\npassword = \"{long_password}\"\nplaceholder_marker = \"{short_placeholder}\"\n"
    );
    std::fs::write(root.join("settings.py"), source).expect("source");
    let repository = RepositoryAccess::open(&root).expect("repository");

    let credential_query = McpToolInput {
        q: "credential_marker password".into(),
        ..Default::default()
    }
    .normalize()
    .expect("credential query");
    let credential_output = repository
        .search(&credential_query, 8, None)
        .expect("credential search");
    let credential_rendered = credential_output
        .hits
        .iter()
        .map(|hit| hit.excerpt.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!credential_rendered.contains(&long_password));

    let placeholder_query = McpToolInput {
        q: "placeholder_marker".into(),
        ..Default::default()
    }
    .normalize()
    .expect("placeholder query");
    let placeholder_output = repository
        .search(&placeholder_query, 8, None)
        .expect("placeholder search");
    let placeholder_rendered = placeholder_output
        .hits
        .iter()
        .map(|hit| hit.excerpt.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(placeholder_rendered.contains(&short_placeholder));

    drop(repository);
    std::fs::remove_dir_all(root).expect("cleanup");
}
