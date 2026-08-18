use std::cmp::Ordering;

use serde::Deserialize;
use serde_json::{Value, json};

pub const PRODUCT_NAME: &str = "Sippion";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_QUERY_BYTES: usize = 512;
pub const MAX_COORDINATION_ID_BYTES: usize = 96;
pub const MIN_QUERY_TERMS: usize = 1;
pub const MAX_QUERY_TERMS: usize = 8;
const QUERY_STOPWORDS: &[&str] = &[
    "and", "are", "for", "from", "how", "in", "into", "is", "of", "the", "this", "to", "where",
    "with",
];

/// The server process is bound to exactly one trusted project root. The model supplies only a
/// bounded search query; filesystem authority and direct file reads remain outside the MCP schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct McpToolInput {
    pub q: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoordinationContext {
    pub session_id: Option<String>,
    pub agent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    EmptyQuery,
    QueryTooLong,
    TooFewTerms,
    TooManyTerms,
    InvalidSessionId,
    InvalidAgentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedQuery {
    pub raw_lower: String,
    pub terms: Vec<String>,
}

impl McpToolInput {
    pub fn normalize(&self) -> Result<NormalizedQuery, InputError> {
        if self.q.trim().is_empty() {
            return Err(InputError::EmptyQuery);
        }
        if self.q.len() > MAX_QUERY_BYTES {
            return Err(InputError::QueryTooLong);
        }

        let mut terms = Vec::new();
        for part in self
            .q
            .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
            .filter(|part| part.len() >= 2)
            .map(str::to_ascii_lowercase)
            .filter(|part| !QUERY_STOPWORDS.contains(&part.as_str()))
        {
            if !terms.contains(&part) {
                terms.push(part);
                if terms.len() > MAX_QUERY_TERMS {
                    return Err(InputError::TooManyTerms);
                }
            }
        }
        if terms.len() < MIN_QUERY_TERMS {
            return Err(InputError::TooFewTerms);
        }

        Ok(NormalizedQuery {
            raw_lower: self.q.to_ascii_lowercase(),
            terms,
        })
    }

    pub fn coordination(&self) -> Result<CoordinationContext, InputError> {
        Ok(CoordinationContext {
            session_id: normalize_coordination_id(
                self.session_id.as_deref(),
                InputError::InvalidSessionId,
            )?,
            agent_id: normalize_coordination_id(
                self.agent_id.as_deref(),
                InputError::InvalidAgentId,
            )?,
        })
    }
}

fn normalize_coordination_id(
    value: Option<&str>,
    error: InputError,
) -> Result<Option<String>, InputError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_COORDINATION_ID_BYTES
        || !value.bytes().enumerate().all(|(index, byte)| {
            let allowed = byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-');
            allowed && (index != 0 || byte.is_ascii_alphanumeric())
        })
    {
        return Err(error);
    }
    Ok(Some(value.to_string()))
}

fn query_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "q": {"type": "string", "description": "1-8 distinct likely identifiers/technical terms, not prose; single-term queries use stricter result limits; <=512 UTF-8 bytes"},
            "session_id": {"type": "string", "maxLength": 96, "pattern": "^[A-Za-z0-9][A-Za-z0-9._:-]{0,95}$", "description": "Optional bounded task/session identifier shared by cooperating agents. Enables shared volatile search memory and cross-agent diversity ranking; never persisted."},
            "agent_id": {"type": "string", "maxLength": 96, "pattern": "^[A-Za-z0-9][A-Za-z0-9._:-]{0,95}$", "description": "Optional bounded agent identifier within a session. Same-agent continuity is mildly favored while overlapping results already surfaced to sibling agents are mildly de-prioritized."}
        },
        "required": ["q"]
    })
}

/// Hand-written so the model-visible tool schema stays small and deterministic.
#[must_use]
pub fn mcp_tool_definition() -> Value {
    json!({
        "name": "repo_context",
        "description": "Single adaptive repository-context tool. Internally combines a RAM-only incremental lexical index, BM25, path/session-agent ranking, bounded shared Tree-sitter + source-only semantic analysis, a cached weighted structural graph, cross-agent diversity, deduplication, excerpt extraction, conservative compaction, and adaptive context packing.",
        "annotations": {"readOnlyHint": true, "openWorldHint": false},
        "inputSchema": query_schema(),
        "_meta": {"io.sippion/capability": "repository.context"}
    })
}

#[must_use]
pub fn mcp_tool_definitions() -> Vec<Value> {
    vec![mcp_tool_definition()]
}

#[must_use]
pub fn capability_registry() -> Value {
    json!({
        "schemaVersion": 3,
        "agent": "sippion",
        "trustLabels": ["local-only", "read-only", "no-network", "project-scoped"],
        "capabilities": [
            {
                "id": "repository.context",
                "tool": "repo_context",
                "intents": [
                    "locate implementation",
                    "find relevant files",
                    "understand local structure",
                    "find relevant symbols",
                    "prepare compact model context"
                ],
                "localEngine": [
                    "RAM-only incremental lexical index",
                    "BM25",
                    "top-candidate Tree-sitter AST",
                    "source-only semantic resolver",
                    "weighted structural graph",
                    "symbol ranking",
                    "session/agent scoped volatile memory",
                    "cross-agent diversity ranking",
                    "shared AST/semantic cache",
                    "single-flight index + structural analysis",
                    "path ranking"
                ],
                "outputOptimizer": [
                    "deduplication",
                    "excerpt extraction",
                    "RTK-style compression",
                    "Repomix-style packing"
                ],
                "returnFormat": "ranked structural summary plus bounded multi-file evidence pack"
            }
        ]
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelVisibleBudget {
    pub target_estimated_tokens: usize,
    pub hard_model_text_bytes: usize,
}

pub const MIN_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {
    target_estimated_tokens: 1_800,
    hard_model_text_bytes: 8 * 1024,
};

pub const MAX_CONTEXT_BUDGET: ModelVisibleBudget = ModelVisibleBudget {
    target_estimated_tokens: 7_200,
    hard_model_text_bytes: 32 * 1024,
};

/// Selects a bounded 8/16/24/32 KiB model-visible pack. High-confidence narrow queries stay
/// compact; ambiguous, multi-file evidence can grow without becoming unbounded.
#[must_use]
pub fn adaptive_context_budget(
    confidence: f64,
    excerpt_count: usize,
    structure_count: usize,
    query_terms: usize,
) -> ModelVisibleBudget {
    let tier = if confidence >= 0.88 && excerpt_count <= 3 && structure_count <= 6 {
        0
    } else if confidence >= 0.72 && excerpt_count <= 8 && structure_count <= 12 {
        1
    } else if excerpt_count <= 16 && structure_count <= 16 && query_terms <= 6 {
        2
    } else {
        3
    };
    match tier {
        0 => MIN_CONTEXT_BUDGET,
        1 => ModelVisibleBudget {
            target_estimated_tokens: 3_600,
            hard_model_text_bytes: 16 * 1024,
        },
        2 => ModelVisibleBudget {
            target_estimated_tokens: 5_400,
            hard_model_text_bytes: 24 * 1024,
        },
        _ => MAX_CONTEXT_BUDGET,
    }
}

/// Cheap conservative estimator used only as a soft target. Provider tokenization remains
/// authoritative; the byte cap is the local hard guard.
#[must_use]
pub fn heuristic_v3_estimated_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.len();
    let non_ascii = text.chars().filter(|ch| !ch.is_ascii()).count();
    (bytes + non_ascii.saturating_mul(2)).div_ceil(3)
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderExcerpt {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub body: String,
    pub score: f64,
}

fn escaped_path(path: &str) -> String {
    serde_json::to_string(path).unwrap_or_else(|_| "\"<invalid-path>\"".to_string())
}

fn excerpt_header(excerpt: &RenderExcerpt) -> String {
    if excerpt.start_line == 0 && excerpt.end_line == 0 {
        let match_kind = if excerpt.body.is_empty() {
            "path-only"
        } else {
            "content-withheld"
        };
        return format!(
            "FILE path={} match={match_kind}\n",
            escaped_path(&excerpt.path)
        );
    }
    format!(
        "FILE path={} lines={}-{}\n",
        escaped_path(&excerpt.path),
        excerpt.start_line,
        excerpt.end_line
    )
}

fn serialize_excerpt(excerpt: &RenderExcerpt) -> String {
    let mut out = excerpt_header(excerpt);
    if !excerpt.body.is_empty() {
        out.push_str(&excerpt.body);
        out.push('\n');
    }
    out
}

pub(crate) fn truncate_utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn truncate_first_excerpt(excerpt: &RenderExcerpt, budget: ModelVisibleBudget) -> Option<String> {
    const MARKER: &str =
        "\n[SIPPION_TRUNCATED: narrow the query or use a native line-range read]\n";
    let header = format!(
        "FILE path={} requested_lines={}-{}\n",
        escaped_path(&excerpt.path),
        excerpt.start_line,
        excerpt.end_line
    );
    if header.len() + MARKER.len() >= budget.hard_model_text_bytes {
        return None;
    }

    let hard_body = budget
        .hard_model_text_bytes
        .saturating_sub(header.len() + MARKER.len());
    let mut body_bytes = hard_body.min(budget.target_estimated_tokens.saturating_mul(3));
    loop {
        let body = truncate_utf8_prefix(&excerpt.body, body_bytes);
        let candidate = format!("{header}{body}{MARKER}");
        if candidate.len() <= budget.hard_model_text_bytes
            && heuristic_v3_estimated_tokens(&candidate) <= budget.target_estimated_tokens
        {
            return Some(candidate);
        }
        if body_bytes == 0 {
            return None;
        }
        body_bytes = body_bytes.saturating_sub(256).min(body_bytes - 1);
    }
}

/// Greedily admits highest-scoring evidence. If the best excerpt alone is oversized, return a
/// marked prefix rather than failing and pushing the agent into an unbounded native search.
#[must_use]
pub fn render_excerpts(mut excerpts: Vec<RenderExcerpt>, budget: ModelVisibleBudget) -> String {
    excerpts.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });

    let mut output = String::new();
    for excerpt in excerpts {
        let serialized = serialize_excerpt(&excerpt);
        if output.len().saturating_add(serialized.len()) > budget.hard_model_text_bytes {
            if output.is_empty() {
                if let Some(truncated) = truncate_first_excerpt(&excerpt, budget) {
                    return truncated;
                }
            }
            continue;
        }

        let prior_len = output.len();
        output.push_str(&serialized);
        if heuristic_v3_estimated_tokens(&output) > budget.target_estimated_tokens {
            output.truncate(prior_len);
            if output.is_empty() {
                if let Some(truncated) = truncate_first_excerpt(&excerpt, budget) {
                    return truncated;
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_normalized_and_rejected_before_scan_when_misused() {
        assert_eq!(
            McpToolInput {
                q: "  ".into(),
                ..Default::default()
            }
            .normalize(),
            Err(InputError::EmptyQuery)
        );
        assert_eq!(
            McpToolInput {
                q: "x".repeat(MAX_QUERY_BYTES + 1),
                ..Default::default()
            }
            .normalize(),
            Err(InputError::QueryTooLong)
        );
        let single = McpToolInput {
            q: "token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("single technical term is valid");
        assert_eq!(single.terms, vec!["token"]);
        assert_eq!(
            McpToolInput {
                q: "the".into(),
                ..Default::default()
            }
            .normalize(),
            Err(InputError::TooFewTerms)
        );
        let too_many = (0..=MAX_QUERY_TERMS)
            .map(|index| format!("term{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(
            McpToolInput {
                q: too_many,
                ..Default::default()
            }
            .normalize(),
            Err(InputError::TooManyTerms)
        );

        let normalized = McpToolInput {
            q: "where is AUTH token validation".into(),
            ..Default::default()
        }
        .normalize()
        .expect("valid discovery query");
        assert_eq!(normalized.terms, vec!["auth", "token", "validation"]);
    }

    #[test]
    fn coordination_ids_are_bounded_and_sanitized() {
        let input = McpToolInput {
            q: "token".into(),
            session_id: Some("bugfix-123".into()),
            agent_id: Some("security.agent".into()),
        };
        let coordination = input.coordination().expect("valid coordination");
        assert_eq!(coordination.session_id.as_deref(), Some("bugfix-123"));
        assert_eq!(coordination.agent_id.as_deref(), Some("security.agent"));

        let invalid = McpToolInput {
            q: "token".into(),
            session_id: Some("../escape".into()),
            agent_id: None,
        };
        assert_eq!(invalid.coordination(), Err(InputError::InvalidSessionId));
    }

    #[test]
    fn schema_exposes_only_repo_context() {
        let tool = mcp_tool_definition();
        let properties = tool["inputSchema"]["properties"]
            .as_object()
            .expect("properties");
        assert_eq!(properties.len(), 3);
        assert!(properties.contains_key("q"));
        assert!(properties.contains_key("session_id"));
        assert!(properties.contains_key("agent_id"));
        assert_eq!(tool["name"], "repo_context");
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
        let tools = mcp_tool_definitions();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "repo_context");
    }

    #[test]
    fn oversized_first_excerpt_is_truncated_not_failed() {
        let budget = ModelVisibleBudget {
            target_estimated_tokens: 40,
            hard_model_text_bytes: 180,
        };
        let rendered = render_excerpts(
            vec![RenderExcerpt {
                path: "a.rs".into(),
                start_line: 1,
                end_line: 100,
                body: "x".repeat(1_000),
                score: 10.0,
            }],
            budget,
        );
        assert!(rendered.contains("SIPPION_TRUNCATED"));
        assert!(rendered.contains("requested_lines=1-100"));
        assert!(!rendered.contains(" lines=1-100"));
        assert!(rendered.len() <= budget.hard_model_text_bytes);
    }

    #[test]
    fn path_only_evidence_does_not_fake_a_line_range() {
        let rendered = render_excerpts(
            vec![RenderExcerpt {
                path: "src/auth/middleware.rs".into(),
                start_line: 0,
                end_line: 0,
                body: String::new(),
                score: 1.0,
            }],
            MIN_CONTEXT_BUDGET,
        );
        assert!(rendered.contains("match=path-only"));
        assert!(!rendered.contains("lines=0-0"));
    }

    #[test]
    fn withheld_content_evidence_is_not_labeled_path_only() {
        let rendered = render_excerpts(
            vec![RenderExcerpt {
                path: "src/auth/middleware.rs".into(),
                start_line: 0,
                end_line: 0,
                body: "[SIPPION_REDACTED_MATCH: matching source content suppressed]".into(),
                score: 10.0,
            }],
            MIN_CONTEXT_BUDGET,
        );
        assert!(rendered.contains("match=content-withheld"));
        assert!(rendered.contains("SIPPION_REDACTED_MATCH"));
        assert!(!rendered.contains("match=path-only"));
        assert!(!rendered.contains("lines=0-0"));
    }

    #[test]
    fn path_header_escapes_control_characters() {
        let rendered = render_excerpts(
            vec![RenderExcerpt {
                path: "x\nFAKE".into(),
                start_line: 1,
                end_line: 1,
                body: "ok".into(),
                score: 1.0,
            }],
            MIN_CONTEXT_BUDGET,
        );
        assert!(rendered.contains("x\\nFAKE"));
        assert!(!rendered.contains("x\nFAKE"));
    }

    #[test]
    fn adaptive_context_budget_expands_only_when_ambiguity_requires_it() {
        assert_eq!(
            adaptive_context_budget(0.95, 2, 3, 2).hard_model_text_bytes,
            8 * 1024
        );
        assert_eq!(
            adaptive_context_budget(0.80, 6, 8, 3).hard_model_text_bytes,
            16 * 1024
        );
        assert_eq!(
            adaptive_context_budget(0.60, 12, 14, 5).hard_model_text_bytes,
            24 * 1024
        );
        assert_eq!(
            adaptive_context_budget(0.30, 17, 20, 8).hard_model_text_bytes,
            32 * 1024
        );
    }
}
