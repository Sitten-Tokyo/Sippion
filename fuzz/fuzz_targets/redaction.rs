#![no_main]

use libfuzzer_sys::fuzz_target;

const MAX_BOUNDED_REDACTION_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct RedactionOutcome {
    text: String,
    truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSensitiveValue {
    indent: usize,
    allow_same_indent: bool,
}

mod production {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/repo/redaction.rs"));

    pub fn bounded(text: &str, limit: usize) -> (String, bool) {
        let result = redact_high_confidence_secrets_bounded(text, limit);
        (result.text, result.truncated)
    }
}

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let limit = data
        .get(..2)
        .map(|bytes| usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
        .unwrap_or(4096)
        .clamp(1, 64 * 1024);

    let (redacted, _truncated) = production::bounded(&input, limit);
    assert!(redacted.len() <= limit);

    // Redaction markers must be stable under a second pass; otherwise repeated repository packing
    // could amplify or mutate already-sanitized model-visible context.
    let (second, _) = production::bounded(&redacted, limit);
    assert_eq!(redacted, second);
});
