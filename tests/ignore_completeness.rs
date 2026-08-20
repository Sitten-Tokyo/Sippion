use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-empty-ignore-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

#[test]
fn empty_gitignore_does_not_turn_complete_no_match_into_policy_excluded_status() {
    let root = temp_root();
    std::fs::write(root.join(".gitignore"), "").expect("empty ignore file");
    std::fs::write(root.join("source.rs"), "fn ordinary_marker() {}\n").expect("source");

    let mut child = Command::new(env!("CARGO_BIN_EXE_sippion"))
        .args(["mcp", "--root"])
        .arg(&root)
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
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for sippion");
    assert!(
        output.status.success(),
        "sippion failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 MCP response");
    let response: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("valid JSON-RPC response");
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("model-visible text");

    assert!(text.contains("policy_excluded=0"));
    assert!(text.contains("\n[NO_MATCH]\n"));
    assert!(!text.contains("NO_MATCH_IN_SEARCHABLE_SET"));

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}
