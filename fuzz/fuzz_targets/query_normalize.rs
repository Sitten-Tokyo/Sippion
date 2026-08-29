#![no_main]

use libfuzzer_sys::fuzz_target;
use sippion::core::{MAX_QUERY_TERMS, McpToolInput};

fuzz_target!(|data: &[u8]| {
    let first_cut = data.len() / 3;
    let second_cut = first_cut.saturating_mul(2).min(data.len());
    let query = String::from_utf8_lossy(&data[..first_cut]).into_owned();
    let session_id = String::from_utf8_lossy(&data[first_cut..second_cut]).into_owned();
    let agent_id = String::from_utf8_lossy(&data[second_cut..]).into_owned();

    let input = McpToolInput {
        q: query,
        session_id: Some(session_id),
        agent_id: Some(agent_id),
    };

    if let Ok(normalized) = input.normalize() {
        assert!(!normalized.terms.is_empty());
        assert!(normalized.terms.len() <= MAX_QUERY_TERMS);
        assert!(normalized.terms.iter().all(|term| !term.is_empty()));
    }

    if let Ok(coordination) = input.coordination() {
        for id in [coordination.session_id, coordination.agent_id]
            .into_iter()
            .flatten()
        {
            assert!(id.len() <= 96);
            assert!(id.as_bytes()[0].is_ascii_alphanumeric());
            assert!(id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            }));
        }
    }
});
