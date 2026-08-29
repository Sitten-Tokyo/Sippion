use sippion::core::{MAX_QUERY_TERMS, McpToolInput};

#[test]
fn successful_normalization_is_idempotent_and_bounded() {
    let atoms = [
        "AuthToken",
        "Straße",
        "Ｃａｃｈｅ",
        "décomposé",
        "snake_case",
        "kebab-case",
        "HTTP2",
        "東京API",
        "and",
        "the",
    ];

    for seed in 0usize..512 {
        let mut parts = Vec::new();
        for offset in 0usize..10 {
            if (seed >> (offset % 9)) & 1 == 1 {
                parts.push(atoms[(seed + offset * 3) % atoms.len()]);
            }
        }
        if parts.is_empty() {
            parts.push(atoms[seed % atoms.len()]);
        }

        let input = McpToolInput {
            q: parts.join(" / "),
            session_id: None,
            agent_id: None,
        };
        let Ok(first) = input.normalize() else {
            continue;
        };
        assert!(!first.terms.is_empty());
        assert!(first.terms.len() <= MAX_QUERY_TERMS);

        let second = McpToolInput {
            q: first.raw_lower.clone(),
            session_id: None,
            agent_id: None,
        }
        .normalize()
        .expect("normalized query should remain valid");

        assert_eq!(first.terms, second.terms);
    }
}

#[test]
fn accepted_coordination_ids_round_trip_after_trimming() {
    let ids = [
        "agent-1",
        "session.alpha",
        "A_B:C-9",
        "  worker.007  ",
        "z",
    ];

    for id in ids {
        let coordination = McpToolInput {
            q: "token".to_string(),
            session_id: Some(id.to_string()),
            agent_id: Some(id.to_string()),
        }
        .coordination()
        .expect("sample coordination id should be accepted");

        assert_eq!(coordination.session_id.as_deref(), Some(id.trim()));
        assert_eq!(coordination.agent_id.as_deref(), Some(id.trim()));
    }
}
