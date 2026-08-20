use std::io::{BufRead, BufReader, Write};
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

fn assemble(prefix: &[u8], tail: &str) -> String {
    let mut value = String::from_utf8(prefix.to_vec()).expect("ASCII provider prefix");
    value.push_str(tail);
    value
}

fn provider_secret_corpus() -> Vec<String> {
    // Provider prefixes are numeric byte arrays so the repository never stores a
    // secret-looking token literal and GitHub push protection needs no bypass.
    [
        (&[103, 105, 116, 104, 117, 98, 95, 112, 97, 116, 95][..], "11AA22BB33CC44DD55EE66FF77GG88HH99II"),
        (&[103, 104, 112, 95][..], "abcdefghijklmnopqrstuvwxyz0123456789"),
        (&[103, 108, 112, 97, 116, 45][..], "abcdefghijklmnopqrstuvwxyz012345"),
        (&[120, 111, 120, 98, 45][..], "123456789012-123456789012-abcdefghijklmnopqrstuvwx"),
        (&[120, 111, 120, 112, 45][..], "123456789012-123456789012-abcdefghijklmnopqrstuvwx"),
        (&[65, 73, 122, 97][..], "SyA1234567890abcdefghijklmnop"),
        (&[65, 75, 73, 65][..], "1234567890ABCDEF"),
        (&[65, 83, 73, 65][..], "1234567890ABCDEF"),
        (&[115, 107, 45][..], "abcdefghijklmnopqrstuvwxyz0123456789"),
    ]
    .into_iter()
    .map(|(prefix, tail)| assemble(prefix, tail))
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
