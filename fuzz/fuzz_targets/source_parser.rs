#![no_main]

use std::fs;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;

const EXTENSIONS: &[&str] = &["rs", "py", "js", "ts", "go", "java", "cs", "c", "cpp"];

fuzz_target!(|data: &[u8]| {
    let Ok(binary) = std::env::var("SIPPION_FUZZ_BIN") else {
        return;
    };
    let extension = EXTENSIONS[data.first().copied().unwrap_or(0) as usize % EXTENSIONS.len()];
    let root = std::env::temp_dir().join(format!("sippion-source-fuzz-{}", std::process::id()));
    fs::create_dir_all(&root).expect("create source fuzz root");
    let source = String::from_utf8_lossy(data);
    fs::write(root.join(format!("input.{extension}")), source.as_bytes()).expect("write fuzz source");

    let mut child = Command::new(&binary)
        .arg("query")
        .arg("--root")
        .arg(&root)
        .arg("--json")
        .arg("--")
        .args(["fuzz", "target", "symbol"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start retrieval query");

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        match child.try_wait().expect("poll retrieval query") {
            Some(status) => {
                assert!(status.success(), "retrieval failed on arbitrary UTF-8 source");
                break;
            }
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("retrieval exceeded the source-parser fuzz deadline");
            }
        }
    }
});
