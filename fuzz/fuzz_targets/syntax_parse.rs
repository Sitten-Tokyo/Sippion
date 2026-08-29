#![no_main]

use libfuzzer_sys::fuzz_target;
use sippion::fuzz_support::parse_supported_source;

const PATHS: &[&str] = &[
    "input.rs",
    "input.py",
    "input.js",
    "input.ts",
    "input.go",
    "input.java",
    "input.cs",
    "input.c",
    "input.cpp",
];

fuzz_target!(|data: &[u8]| {
    let Some((&selector, source)) = data.split_first() else {
        return;
    };
    let path = PATHS[usize::from(selector) % PATHS.len()];
    let text = String::from_utf8_lossy(source);
    let count = parse_supported_source(path, &text, 64);
    assert!(count <= 64);
});
