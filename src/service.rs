use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::core::{CoordinationContext, NormalizedQuery};
use crate::repo::RepositoryAccessError;
pub use crate::repo::{MAX_CONFIGURED_SCAN_BYTES, MAX_SCAN_BYTES, MIN_CONFIGURED_SCAN_BYTES};

mod context;
mod engine;

use engine::RepositoryEngine;

#[derive(Debug, Clone, serde::Serialize)]
pub struct RankedFileDiagnostic {
    pub path: String,
    pub rank: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ContextDiagnostics {
    pub returned_bytes: usize,
    pub estimated_tokens: usize,
    pub hard_budget_bytes: usize,
    pub target_estimated_tokens: usize,
    pub scanned_bytes: usize,
    pub confidence_milli: u16,
    pub adaptive_rounds: usize,
    /// Retrieval ranking before model-visible packing. Evaluation must use this for Recall/MRR.
    pub ranked_files: Vec<RankedFileDiagnostic>,
    /// Unique paths that actually survived the model-visible context packer.
    pub packed_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ContextResult {
    pub model_text: String,
    pub diagnostics: ContextDiagnostics,
}

/// Internal boundary for repository-context retrieval.
///
/// MCP framing remains outside this trait. The engine beneath it owns retrieval, structural
/// expansion, request-local source reuse, and token-aware output packing.
pub trait RepositoryService: Send + Sync {
    fn context(
        &self,
        query: &NormalizedQuery,
        coordination: Option<&CoordinationContext>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<String, RepositoryServiceError>;
}

pub struct LocalRepositoryService {
    engine: RepositoryEngine,
}

impl LocalRepositoryService {
    pub fn open_with_scan_budget(
        root_path: impl AsRef<Path>,
        scan_budget_bytes: usize,
    ) -> Result<Self, RepositoryServiceError> {
        Ok(Self {
            engine: RepositoryEngine::open_with_scan_budget(root_path, scan_budget_bytes)?,
        })
    }

    pub fn context_result(
        &self,
        query: &NormalizedQuery,
        coordination: Option<&CoordinationContext>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<ContextResult, RepositoryServiceError> {
        self.engine
            .context_result(query, coordination, cancellation)
            .map_err(Into::into)
    }

    #[cfg(test)]
    fn from_repository(repository: crate::repo::RepositoryAccess) -> Self {
        Self {
            engine: RepositoryEngine::from_repository(repository),
        }
    }
}

impl RepositoryService for LocalRepositoryService {
    fn context(
        &self,
        query: &NormalizedQuery,
        coordination: Option<&CoordinationContext>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<String, RepositoryServiceError> {
        self.engine
            .context(query, coordination, cancellation)
            .map_err(Into::into)
    }
}

#[derive(Debug)]
pub struct RepositoryServiceError {
    source: RepositoryAccessError,
}

impl RepositoryServiceError {
    #[must_use]
    pub fn user_message(&self) -> &'static str {
        match &self.source {
            RepositoryAccessError::InvalidRelativePath => "invalid project-relative path",
            RepositoryAccessError::NonUtf8Path => "project-relative path is not valid UTF-8",
            RepositoryAccessError::DeniedPath => "path is denied by the local safety policy",
            RepositoryAccessError::PrunedPath => "path is pruned from retrieval",
            RepositoryAccessError::NotRegularFile => "path is not a regular file",
            RepositoryAccessError::NotFound => "file not found",
            RepositoryAccessError::TooLarge => "file exceeds the 2 MiB source limit",
            RepositoryAccessError::NonUtf8Source => "file is not UTF-8 text",
            RepositoryAccessError::HardLinkedFile => {
                "hard-linked file is denied by the local safety policy"
            }
            RepositoryAccessError::ConcurrentModification => "file changed while being read; retry",
            RepositoryAccessError::Io => "repository read failed",
            RepositoryAccessError::Cancelled => "repository search cancelled",
        }
    }
}

impl From<RepositoryAccessError> for RepositoryServiceError {
    fn from(source: RepositoryAccessError) -> Self {
        Self { source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::McpToolInput;
    use crate::repo::RepositoryAccess;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn query_width_changes_candidate_limits() {
        let single = McpToolInput {
            q: "AuthenticationMiddleware".into(),
            ..Default::default()
        }
        .normalize()
        .expect("single");
        let multi = McpToolInput {
            q: "authentication middleware".into(),
            ..Default::default()
        }
        .normalize()
        .expect("multi");
        assert_eq!(engine::context_result_limits(&single), (8, 6));
        assert_eq!(engine::context_result_limits(&multi), (16, 12));
    }

    #[test]
    fn policy_exclusions_remain_visible_in_compact_context() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("sippion-service-policy-{nonce}"));
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("normal.rs"), "fn normal() {}\n").expect("normal");
        std::fs::write(root.join("huge.rs"), vec![b'x'; 2 * 1024 * 1024 + 1]).expect("huge");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let service = LocalRepositoryService::from_repository(repository);
        let query = McpToolInput {
            q: "definitely_missing".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query");
        let output = service.context(&query, None, None).expect("context");
        assert!(output.contains("[NO_MATCH_IN_SEARCHABLE_SET:"));
        assert!(output.contains("excluded=1"));
        assert!(output.contains("CTX v=4"));

        drop(service);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn typed_diagnostics_do_not_depend_on_model_visible_format_parsing() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("sippion-service-diagnostic-{nonce}"));
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(root.join("auth.rs"), "fn validate_session_token() {}\n").expect("auth");

        let repository = RepositoryAccess::open(&root).expect("open repository");
        let service = LocalRepositoryService::from_repository(repository);
        let query = McpToolInput {
            q: "validate_session_token".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query");
        let result = service
            .context_result(&query, None, None)
            .expect("context result");
        assert_eq!(
            result
                .diagnostics
                .ranked_files
                .first()
                .map(|entry| entry.path.as_str()),
            Some("auth.rs")
        );
        assert!(
            result
                .diagnostics
                .packed_paths
                .iter()
                .any(|path| path == "auth.rs")
        );
        assert_eq!(result.diagnostics.returned_bytes, result.model_text.len());

        drop(service);
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
