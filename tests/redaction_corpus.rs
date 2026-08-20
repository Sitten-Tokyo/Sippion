use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_root() -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("sippion-redaction-corpus-{nonce}"));
    std::fs::create_dir_all(&root).expect("create test repository");
    root
}

fn provider_secret_corpus() -> Vec<String> {
    // Assemble synthetic, non-functional provider shapes at runtime so repository
    // push-protection does not need a bypass for intentionally fake credentials.
    [
        ("github_pat_", "11AA22BB33CC44DD55EE66FF77GG88HH99II"),
        ("ghp_", "abcdefghijklmnopqrstuvwxyz0123456789"),
        ("glpat-", "abcdefghijklmnopqrstuvwxyz012345"),
        (
            "xoxb-",
            "123456789012-123456789012-abcdefghijklmnopqrstuvwx",
        ),
        (
            "xoxp-",
            "123456789012-123456789012-abcdefghijklmnopqrstuvwx",
        ),
        ("AIza", "SyA1234567890abcdefghijklmnop"),
        ("AKIA", "1234567890ABCDEF"),
        ("ASIA", "1234567890ABCDEF"),
        ("sk-", "abcdefghijklmnopqrstuvwxyz0123456789"),
    ]
    .into_iter()
    .map(|(prefix, tail)| format!("{prefix}{tail}"))
    .collect()
}

#[test]
fn provider_secret_formats_never_reach_model_visible_mcp_output() {
    let secrets = provider_secret_corpus();
    let root = temp_root();
    let source = format!(
        "fn provider_secret_marker() {{\n    let credentials = {:?};\n}}\n",
        secrets
    );
    std::fs::write(root.join("provider_secrets.rs"), source).expect("write corpus source");

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
            "arguments": {"q": "provider_secret_marker"}
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

    assert!(text.contains("provider_secret_marker"));
    assert!(text.contains("SIPPION_REDACTED"));
    for secret in secrets {
        assert!(
            !text.contains(&secret),
            "provider-shaped secret escaped redaction: {secret}"
        );
    }

    std::fs::remove_dir_all(root).expect("cleanup test repository");
}
