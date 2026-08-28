use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-unicode-search-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

#[test]
fn unicode_case_variants_match_and_preserve_original_excerpt() {
    let root = temp_root();
    std::fs::write(
        root.join("unicode.rs"),
        "pub fn ÜBERPRÜFUNG() -> bool { true }\n",
    )
    .expect("source");

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
            "arguments": {"q": "überprüfung"}
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
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("model-visible text");
    assert!(text.contains("unicode.rs"));
    assert!(text.contains("ÜBERPRÜFUNG"));
    assert!(!text.contains("\n[NO_MATCH]\n"));

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}
