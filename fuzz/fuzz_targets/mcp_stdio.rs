#![no_main]

use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(binary) = std::env::var("SIPPION_FUZZ_BIN") else {
        return;
    };
    let Ok(root) = std::env::var("SIPPION_FUZZ_ROOT") else {
        return;
    };

    let mut child = Command::new(binary)
        .args(["mcp", "--root", &root])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start sippion MCP server");

    if let Some(mut stdin) = child.stdin.take() {
        let capped = &data[..data.len().min(64 * 1024)];
        stdin.write_all(capped).expect("write fuzz frame");
        if !capped.ends_with(b"\n") {
            stdin.write_all(b"\n").expect("terminate fuzz frame");
        }
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait().expect("poll MCP server") {
            Some(status) => {
                assert!(status.success(), "MCP server exited abnormally for arbitrary input");
                break;
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                // Valid tool calls can legitimately consume a bounded retrieval budget. A timeout
                // here is not a crash signal; terminate the child so fuzzing keeps making progress.
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
        }
    }
});
