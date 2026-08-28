use super::*;

const MIN_CONFIDENCE_GAIN_FOR_EXPANSION: f64 = 0.025;
const MIN_NEW_HITS_PER_MIB: f64 = 0.02;

fn normalized_idf_rarity(document_count: usize, document_frequencies: &[usize]) -> f64 {
    if document_count == 0 || document_frequencies.is_empty() {
        return 0.0;
    }
    let n = document_count as f64;
    let max_observed_idf = (1.0 + (n - 1.0 + 0.5) / (1.0 + 0.5)).ln().max(f64::EPSILON);
    document_frequencies
        .iter()
        .map(|frequency| {
            if *frequency == 0 {
                return 0.0;
            }
            let df = (*frequency).min(document_count) as f64;
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
            (idf / max_observed_idf).clamp(0.0, 1.0)
        })
        .sum::<f64>()
        / document_frequencies.len() as f64
}

fn exact_query_term_coverage(query: &NormalizedQuery, outcome: &SearchOutcome) -> f64 {
    if query.terms.is_empty() || outcome.hits.is_empty() {
        return 0.0;
    }
    let mut exact = HashSet::<String>::new();
    for hit in outcome.hits.iter().take(3) {
        let folded = crate::core::unicode_search_fold(&format!(
            "{} {}",
            hit.relative_path, hit.excerpt
        ));
        let tokens = crate::core::split_search_tokens(&folded).collect::<HashSet<_>>();
        for term in &query.terms {
            if tokens.contains(term.as_str()) {
                exact.insert(term.clone());
            }
        }
    }
    exact.len() as f64 / query.terms.len() as f64
}

fn efficiency_confidence(outcome: &SearchOutcome, specificity: f64) -> f64 {
    if outcome.hits.is_empty() {
        return if !outcome.truncated && outcome.coverage.policy_excluded_files == 0 {
            0.98
        } else if outcome.coverage.policy_excluded_files > 0 {
            0.35
        } else {
            0.05
        };
    }
    let coverage = if outcome.coverage.eligible_files == 0 {
        1.0
    } else {
        outcome.coverage.indexed_files as f64 / outcome.coverage.eligible_files as f64
    };
    let top = outcome.hits.first().map_or(0.0, |hit| hit.score.max(0.0));
    let second = outcome.hits.get(1).map_or(0.0, |hit| hit.score.max(0.0));
    let score_confidence = 1.0 - (-top / 90.0).exp();
    let gap_confidence = if top <= f64::EPSILON {
        0.0
    } else {
        ((top - second).max(0.0) / top).clamp(0.0, 1.0)
    };
    (score_confidence * 0.38
        + gap_confidence * 0.20
        + coverage * 0.22
        + specificity.clamp(0.0, 1.0) * 0.20)
        .clamp(0.0, 1.0)
}

impl RepositoryAccess {
    fn query_specificity(
        &self,
        query: &NormalizedQuery,
        outcome: &SearchOutcome,
    ) -> Result<f64, RepositoryAccessError> {
        let hashes = query
            .terms
            .iter()
            .map(|term| stable_term_hash(term))
            .collect::<Vec<_>>();
        let (document_count, document_frequencies) = {
            let index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            let mut frequencies = vec![0usize; hashes.len()];
            for document in index.files.values() {
                for (position, hash) in hashes.iter().enumerate() {
                    if document
                        .terms
                        .binary_search_by_key(hash, |(term_hash, _)| *term_hash)
                        .is_ok()
                    {
                        frequencies[position] = frequencies[position].saturating_add(1);
                    }
                }
            }
            (index.files.len(), frequencies)
        };
        let rarity = normalized_idf_rarity(document_count, &document_frequencies);
        let exact = exact_query_term_coverage(query, outcome);
        let corroboration = if query.terms.len() <= 1 {
            0.0
        } else {
            ((query.terms.len() - 1).min(3) as f64 / 3.0) * exact
        };
        Ok((rarity * 0.55 + exact * 0.35 + corroboration * 0.10).clamp(0.0, 1.0))
    }

    /// Adaptive retrieval optimized for useful evidence per scanned byte rather than coverage alone.
    ///
    /// The legacy adaptive search remains available for regression tests and compatibility. The
    /// service path uses this controller so a low-confidence query does not automatically expand to
    /// the next 2x scan tier when the previous tier produced negligible new evidence.
    pub(crate) fn search_token_efficient_since(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        context: Option<&CoordinationContext>,
        started: &Instant,
    ) -> Result<OptimizedSearchOutcome, RepositoryAccessError> {
        #[cfg(windows)]
        let _windows_search_guard = self
            .windows_search_serial
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        #[cfg(windows)]
        self.reset_ram_index()?;

        let cap = self.max_scan_budget_bytes;
        let mut target = ADAPTIVE_INITIAL_SCAN_BYTES.min(cap);
        let mut granted_allowance = 0usize;
        let mut total_scanned_bytes = 0usize;
        let mut total_scanned_files = 0usize;
        let mut rounds = 0usize;
        let mut policy_skips = HashMap::<String, SourceStamp>::new();
        let mut verification_cache = HashMap::<String, VerifiedCandidate>::new();
        let mut previous_paths = HashSet::<String>::new();
        let mut previous_confidence = 0.0f64;

        loop {
            let round_allowance = target.saturating_sub(granted_allowance);
            rounds = rounds.saturating_add(1);
            let mut outcome = self.search_once(
                query,
                max_results,
                cancellation,
                started,
                round_allowance.max(1),
                &mut policy_skips,
                &mut verification_cache,
                context,
            )?;
            granted_allowance = target;
            total_scanned_bytes =
                total_scanned_bytes.saturating_add(outcome.coverage.scanned_bytes);
            total_scanned_files =
                total_scanned_files.saturating_add(outcome.coverage.scanned_files);

            let specificity = self.query_specificity(query, &outcome)?;
            let confidence = efficiency_confidence(&outcome, specificity);
            outcome.coverage.scanned_bytes = total_scanned_bytes;
            outcome.coverage.scanned_files = total_scanned_files;
            outcome.coverage.scan_budget_bytes = granted_allowance;
            outcome.coverage.scan_budget_cap_bytes = cap;
            outcome.coverage.adaptive_rounds = rounds;
            outcome.coverage.confidence_milli =
                (confidence.clamp(0.0, 1.0) * 1000.0).round() as u16;

            let current_paths = outcome
                .hits
                .iter()
                .map(|hit| hit.relative_path.clone())
                .collect::<HashSet<_>>();
            let new_hits = current_paths.difference(&previous_paths).count();
            let round_mib = (round_allowance.max(1) as f64) / (1024.0 * 1024.0);
            let new_hits_per_mib = new_hits as f64 / round_mib.max(1.0);
            let confidence_gain = (confidence - previous_confidence).max(0.0);
            let marginal_stalled =
                rounds >= 2 && new_hits == 0 && confidence_gain < MIN_CONFIDENCE_GAIN_FOR_EXPANSION;
            let marginal_too_small = rounds >= 3
                && new_hits_per_mib < MIN_NEW_HITS_PER_MIB
                && confidence_gain < MIN_CONFIDENCE_GAIN_FOR_EXPANSION * 1.5;

            let complete_no_match = outcome.hits.is_empty()
                && !outcome.truncated
                && outcome.coverage.policy_excluded_files == 0;
            let timed_out = started.elapsed() >= MAX_SEARCH_WALL_TIME;
            let should_expand = !complete_no_match
                && outcome.truncated
                && outcome.adaptive_expandable
                && confidence < ADAPTIVE_CONFIDENCE_STOP
                && target < cap
                && !timed_out
                && !marginal_stalled
                && !marginal_too_small;

            if !should_expand {
                self.remember_search(query, &outcome.hits, context);
                let snapshots = outcome
                    .hits
                    .iter()
                    .filter_map(|hit| {
                        verification_cache
                            .get(hit.relative_path.as_str())
                            .and_then(|verified| verified.snapshot_source.clone())
                            .map(|source| (hit.relative_path.clone(), source))
                    })
                    .collect::<HashMap<_, _>>();
                return Ok(OptimizedSearchOutcome { outcome, snapshots });
            }

            previous_paths = current_paths;
            previous_confidence = confidence;
            let next = target.saturating_mul(2).min(cap);
            if next <= target {
                self.remember_search(query, &outcome.hits, context);
                let snapshots = outcome
                    .hits
                    .iter()
                    .filter_map(|hit| {
                        verification_cache
                            .get(hit.relative_path.as_str())
                            .and_then(|verified| verified.snapshot_source.clone())
                            .map(|source| (hit.relative_path.clone(), source))
                    })
                    .collect::<HashMap<_, _>>();
                return Ok(OptimizedSearchOutcome { outcome, snapshots });
            }
            target = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(scores: &[f64], indexed: usize) -> SearchOutcome {
        SearchOutcome {
            hits: scores
                .iter()
                .enumerate()
                .map(|(index, score)| SearchHit {
                    relative_path: format!("{index}.rs"),
                    start_line: 1,
                    end_line: 1,
                    excerpt: String::new(),
                    score: *score,
                    source_stamp: None,
                    source_fingerprint: None,
                })
                .collect(),
            truncated: true,
            coverage: SearchCoverage {
                eligible_files: 10,
                indexed_files: indexed,
                ..SearchCoverage::default()
            },
            adaptive_expandable: true,
        }
    }

    #[test]
    fn confidence_rewards_rank_gap_coverage_and_specificity() {
        assert!(efficiency_confidence(&make(&[100.0, 10.0], 10), 0.9) > efficiency_confidence(&make(&[40.0, 39.0], 3), 0.2));
        assert!(efficiency_confidence(&make(&[60.0, 40.0], 8), 0.9) > efficiency_confidence(&make(&[60.0, 40.0], 8), 0.1));
    }

    #[test]
    fn idf_rarity_rewards_terms_seen_in_fewer_documents() {
        let rare = normalized_idf_rarity(100, &[1, 2]);
        let common = normalized_idf_rarity(100, &[70, 80]);
        assert!(rare > common);
    }

    #[test]
    fn complete_no_match_is_high_confidence_without_specificity_hack() {
        let outcome = SearchOutcome {
            hits: Vec::new(),
            truncated: false,
            coverage: SearchCoverage::default(),
            adaptive_expandable: false,
        };
        assert!(efficiency_confidence(&outcome, 0.0) > 0.9);
    }
}
