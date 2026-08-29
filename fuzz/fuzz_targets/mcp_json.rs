#![no_main]

use libfuzzer_sys::fuzz_target;
use serde_json::{Value, json};
use sippion::core::McpToolInput;

const MAX_MCP_REQUEST_BYTES: usize = 256 * 1024;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_MCP_REQUEST_BYTES {
        return;
    }

    if let Ok(value) = serde_json::from_slice::<Value>(data) {
        if value.get("method").and_then(Value::as_str) == Some("tools/call") {
            if let Some(arguments) = value.get("params").and_then(|params| params.get("arguments")) {
                if let Ok(input) = serde_json::from_value::<McpToolInput>(arguments.clone()) {
                    let _ = input.normalize();
                    let _ = input.coordination();
                }
            }
        }
    }

    let query = String::from_utf8_lossy(data);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "repo_context",
            "arguments": {"q": query}
        }
    });
    let encoded = serde_json::to_vec(&request).expect("JSON serialization is infallible for Value");
    let decoded: Value = serde_json::from_slice(&encoded).expect("serialized Value must parse");
    assert_eq!(decoded["method"], "tools/call");
});
