use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use crate::core::{CoordinationContext, NormalizedQuery, RenderExcerpt};
use crate::hybrid::compact_source_excerpt;
use crate::repo::{RepositoryAccess, RepositoryAccessError};

use super::context::pack_context;

/// Retrieval engine kept separate from MCP framing and service error translation.
///
/// The engine owns the trusted repository capability and orchestrates bounded retrieval,
/// request-local source reuse, structural expansion, and final utility-per-token packing.
pub(super) struct RepositoryEngine {
    repository: RepositoryAccess,
}

impl RepositoryEngine {
    pub(super) fn open_with_scan_budget(
        root_path: impl AsRef<Path>,
        scan_budget_bytes: usize,
    ) -> Result<Self, RepositoryAccessError> {
        Ok(Self {
            repository: RepositoryAccess::open_with_scan_budget(root_path, scan_budget_bytes)?,
        })
    }

    #[cfg(test)]
    pub(super) fn from_repository(repository: RepositoryAccess) -> Self {
        Self { repository }
    }

    pub(super) fn context(
        &self,
        query: &NormalizedQuery,
        coordination: Option<&CoordinationContext>,
        cancellation: Option<&AtomicBool>,
    ) -> Result<String, RepositoryAccessError> {
        let (search_results, structural_files) = context_result_limits(query);
        let started = Instant::now();
        let optimized = self.repository.search_token_efficient_since(
            query,
            search_results,
            cancellation,
            coordination,
            &started,
        )?;
        let structure = self.repository.map_from_hits_with_snapshots_since(
            query,
            &optimized.outcome.hits,
            &optimized.snapshots,
            structural_files,
            cancellation,
            &started,
        )?;

        let search_truncated = optimized.outcome.truncated;
        let coverage = optimized.outcome.coverage.clone();
        let structural_scores = structure
            .entries
            .iter()
            .map(|entry| (entry.relative_path.as_str(), entry.score))
            .collect::<HashMap<_, _>>();
        let invalidated_evidence = structure
            .invalidated_evidence_paths
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();

        let mut seen_paths = HashSet::new();
        let excerpts = optimized
            .outcome
            .hits
            .into_iter()
            .filter(|hit| !invalidated_evidence.contains(hit.relative_path.as_str()))
            .filter(|hit| seen_paths.insert(hit.relative_path.clone()))
            .map(|hit| RenderExcerpt {
                score: structural_scores
                    .get(hit.relative_path.as_str())
                    .copied()
                    .unwrap_or(hit.score),
                path: hit.relative_path,
                start_line: hit.start_line,
                end_line: hit.end_line,
                body: compact_source_excerpt(&hit.excerpt),
            })
            .collect::<Vec<_>>();

        let status = if search_truncated || structure.truncated {
            if excerpts.is_empty() {
                "[NO_MATCH_IN_BOUNDED_SCAN: results incomplete; narrow q]"
            } else {
                "[BOUNDED_CONTEXT_INCOMPLETE]"
            }
        } else if coverage.policy_excluded_files > 0 {
            if excerpts.is_empty() {
                "[NO_MATCH_IN_SEARCHABLE_SET: policy exclusions exist]"
            } else {
                "[CONTEXT_FROM_SEARCHABLE_SET: policy exclusions exist]"
            }
        } else if excerpts.is_empty() {
            "[NO_MATCH]"
        } else {
            ""
        };

        Ok(pack_context(
            query,
            &structure.entries,
            &excerpts,
            status,
            &coverage,
        ))
    }
}

pub(super) fn context_result_limits(query: &NormalizedQuery) -> (usize, usize) {
    match query.terms.len() {
        0..=1 => (8, 6),
        2..=4 => (16, 12),
        _ => (24, 16),
    }
}
