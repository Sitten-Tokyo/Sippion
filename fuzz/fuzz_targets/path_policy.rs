#![no_main]

use libfuzzer_sys::fuzz_target;
use sippion::fuzz_support::{PathDisposition, path_disposition};

fuzz_target!(|data: &[u8]| {
    let path = String::from_utf8_lossy(data);
    let _ = path_disposition(&path);

    let traversal = format!("../{path}");
    assert_ne!(path_disposition(&traversal), PathDisposition::Allowed);

    let absolute = format!("/{path}");
    assert_ne!(path_disposition(&absolute), PathDisposition::Allowed);

    assert_ne!(path_disposition(".git/config"), PathDisposition::Allowed);
    assert_ne!(path_disposition("target/debug/sippion"), PathDisposition::Allowed);
});
