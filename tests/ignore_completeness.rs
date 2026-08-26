use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-ignore-{label}-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

fn query_missing_marker(root: &std::path::Path) -> String {
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
            "arguments": {"q": "definitely_missing_marker"}
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
        .expect("read async MCP response");
    assert!(!response_line.is_empty(), "MCP response must not be empty");

    drop(child.stdin.take());
    let status = child.wait().expect("wait for sippion");
    assert!(status.success(), "sippion process failed");

    let response: serde_json::Value =
        serde_json::from_str(response_line.trim()).expect("valid JSON-RPC response");
    response["result"]["content"][0]["text"]
        .as_str()
        .expect("model-visible text")
        .to_string()
}

fn assert_complete_no_match(text: &str) {
    assert!(text.contains("policy_excluded=0"));
    assert!(text.contains("\n[NO_MATCH]\n"));
    assert!(!text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
}

#[test]
fn empty_gitignore_does_not_turn_complete_no_match_into_policy_excluded_status() {
    let root = temp_root("empty");
    std::fs::write(root.join(".gitignore"), "").expect("empty ignore file");
    std::fs::write(root.join("source.rs"), "fn ordinary_marker() {}\n").expect("source");

    let text = query_missing_marker(&root);
    assert_complete_no_match(&text);

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}

#[test]
fn comment_only_gitignore_does_not_degrade_complete_no_match() {
    let root = temp_root("comments");
    std::fs::write(
        root.join(".gitignore"),
        "# generated comment\n\n# another comment\r\n",
    )
    .expect("comment-only ignore file");
    std::fs::write(root.join("source.rs"), "fn ordinary_marker() {}\n").expect("source");

    let text = query_missing_marker(&root);
    assert_complete_no_match(&text);

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}

#[test]
fn effective_gitignore_still_prevents_absolute_no_match() {
    let root = temp_root("effective");
    std::fs::write(root.join(".gitignore"), "hidden.rs\n").expect("effective ignore file");
    std::fs::write(root.join("source.rs"), "fn ordinary_marker() {}\n").expect("source");
    std::fs::write(
        root.join("hidden.rs"),
        "fn definitely_missing_marker() {}\n",
    )
    .expect("hidden");

    let text = query_missing_marker(&root);
    assert!(!text.contains("policy_excluded=0"));
    assert!(text.contains("NO_MATCH_IN_SEARCHABLE_SET"));
    assert!(!text.contains("\n[NO_MATCH]\n"));

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}
