#![no_main]

use std::path::Path;

use libfuzzer_sys::fuzz_target;

mod production {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/root.rs"));

    pub fn broad(path: &std::path::Path, home: Option<&std::path::Path>) -> bool {
        is_broad_root(path, home)
    }
}

fuzz_target!(|data: &[u8]| {
    let split = data.iter().position(|byte| *byte == 0).unwrap_or(data.len());
    let path_text = String::from_utf8_lossy(&data[..split]);
    let home_text = if split < data.len() {
        String::from_utf8_lossy(&data[split + 1..])
    } else {
        String::new().into()
    };
    let path = Path::new(path_text.as_ref());
    let home = Path::new(home_text.as_ref());

    let without_home = production::broad(path, None);
    let with_home = production::broad(path, Some(home));

    // Adding a home boundary can only make root selection more conservative.
    assert!(!without_home || with_home);
    if home.starts_with(path) {
        assert!(with_home);
    }
    if path.parent().is_none() {
        assert!(without_home);
    }
});
