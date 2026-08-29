#![no_main]

use libfuzzer_sys::fuzz_target;
use sippion::fuzz_support::redact_bounded;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let limit = data
        .first()
        .map_or(4096usize, |byte| 256usize.saturating_add(usize::from(*byte) * 64));
    let (output, _) = redact_bounded(&text, limit);
    assert!(output.len() <= limit);

    // Independently synthesize a high-confidence credential around arbitrary input so the fuzzer
    // continuously checks the non-disclosure property, not only crash resistance.
    let secret = data
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if secret.len() >= 32 {
        let source = format!("api_key = \"{secret}\"\n");
        let (redacted, _) = redact_bounded(&source, 4096);
        assert!(!redacted.contains(&secret));
    }
});
