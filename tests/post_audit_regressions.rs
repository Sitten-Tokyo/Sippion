use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-post-audit-{label}-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

fn query(root: &std::path::Path, q: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sippion"))
        .args(["mcp", "--root"])
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start sippion");

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            },
            "name": "repo_context",
            "arguments": {"q": q}
        }
    });
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        serde_json::to_writer(&mut *stdin, &request).expect("serialize request");
        stdin.write_all(b"\n").expect("write newline");
        stdin.flush().expect("flush request");
    }

    let stdout = child.stdout.take().expect("child stdout");
    let mut reader = BufReader::new(stdout);
    let mut response_line = String::new();
    reader
        .read_line(&mut response_line)
        .expect("read MCP response");
    assert!(!response_line.is_empty());
    drop(child.stdin.take());
    let status = child.wait().expect("wait for sippion");
    assert!(status.success());

    let response: serde_json::Value = serde_json::from_str(response_line.trim()).expect("JSON-RPC");
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("model-visible text")
        .to_string()
}

#[test]
fn git_info_exclude_prevents_absolute_no_match() {
    let root = temp_root("git-info-exclude");
    std::fs::create_dir_all(root.join(".git/info")).expect("git info");
    std::fs::write(root.join(".git/info/exclude"), "hidden.rs\n").expect("exclude");
    std::fs::write(root.join("visible.rs"), "fn ordinary_marker() {}\n").expect("visible");
    std::fs::write(root.join("hidden.rs"), "fn git_exclude_marker() {}\n").expect("hidden");

    let text = query(&root, "git_exclude_marker");
    assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
    assert!(!text.contains("\n[NO_MATCH]\n"));
    assert!(!text.contains("policy_excluded=0"));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn framework_secret_keys_are_redacted() {
    let root = temp_root("secret-key");
    let secret = "django-production-secret-value-123456";
    let secret_base = "rails-secret-key-base-value-654321";
    std::fs::write(
        root.join("settings.rs"),
        format!(
            "const SECRET_KEY: &str = \"{secret}\"; const SECRET_KEY_BASE: &str = \"{secret_base}\"; fn django_secret_marker() {{}}\n"
        ),
    )
    .expect("source");

    let text = query(&root, "django_secret_marker");
    assert!(text.contains("django_secret_marker"));
    assert!(text.contains("SIPPION_REDACTED"));
    assert!(!text.contains(secret));
    assert!(!text.contains(secret_base));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn full_unicode_casefold_retrieves_sharp_s_identifier() {
    let root = temp_root("unicode-casefold");
    std::fs::write(
        root.join("unicode.rs"),
        "pub fn StraßeMarker() -> bool { true }\n",
    )
    .expect("source");

    let text = query(&root, "STRASSEMARKER");
    assert!(text.contains("unicode.rs"));
    assert!(text.contains("StraßeMarker"));
    assert!(!text.contains("\n[NO_MATCH]\n"));
    std::fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn credential_control_files_are_denied_before_read() {
    let root = temp_root("credential-paths");
    std::fs::create_dir_all(root.join(".cargo")).expect("cargo dir");
    std::fs::write(root.join("visible.rs"), "fn ordinary_marker() {}\n").expect("visible");
    std::fs::write(
        root.join(".terraformrc"),
        "terraform_credential_marker = true\n",
    )
    .expect("terraform");
    std::fs::write(
        root.join(".cargo/credentials.toml"),
        "cargo_credential_marker = true\n",
    )
    .expect("cargo credentials");

    for marker in ["terraform_credential_marker", "cargo_credential_marker"] {
        let text = query(&root, marker);
        assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
        assert!(!text.contains("FILE path=\".terraformrc\""));
        assert!(!text.contains("FILE path=\".cargo/credentials.toml\""));
    }
    std::fs::remove_dir_all(root).expect("cleanup");
}
