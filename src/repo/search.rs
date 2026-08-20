use super::*;

impl RepositoryAccess {
    /// Incremental bounded hybrid retrieval. Repository contents are indexed into RAM as hashed
    /// lexical statistics, not retained source bodies. On non-Windows targets, discovery metadata
    /// invalidates changed files so only changed/unindexed files consume the broad indexing budget
    /// on later calls. Windows rebuilds the RAM index once per top-level search because its stable
    /// MSRV metadata surface is not a sufficient cross-request content identity.
    /// Adaptive bounded hybrid retrieval. Calls begin with a 32 MiB allowance (or a lower
    /// configured ceiling) and expand 32 -> 64 -> 128 -> 256 -> 512 MiB only while coverage is
    /// incomplete and confidence remains below the stop threshold. The RAM index persists across
    /// rounds, so already indexed unchanged files are not repeatedly retained as source bodies.
    #[cfg(test)]
    pub fn search(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        self.search_coordinated(query, max_results, cancellation, None)
    }

    #[cfg(test)]
    pub fn search_coordinated(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        context: Option<&CoordinationContext>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        let started = Instant::now();
        self.search_coordinated_since(query, max_results, cancellation, context, &started)
    }

    pub fn search_coordinated_since(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        context: Option<&CoordinationContext>,
        started: &Instant,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }
        // On Windows the stable std::fs metadata surface available to the MSRV does not expose a
        // stable file identity/change counter. Size + mtime can therefore be preserved across a
        // same-length replacement. Serialize top-level searches, then discard the previous RAM
        // index before discovery. Adaptive rounds within this one search still share the rebuilt
        // index, so completeness can accumulate normally without stale cross-request reuse.
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
        // Per-call cache for stable policy exclusions discovered only after reading (notably non-UTF-8).
        // This prevents adaptive rounds from repeatedly retrying the same deliberately excluded file.
        let mut policy_skips = HashMap::<String, SourceStamp>::new();
        // Exact verification is cumulative within one adaptive search. Cache only derived evidence
        // and identity, never full source bodies, so each additional round spends its byte grant on
        // candidates that have not already been verified at the same file generation.
        let mut verification_cache = HashMap::<String, VerifiedCandidate>::new();

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

            let confidence = search_confidence(query, &outcome);
            outcome.coverage.scanned_bytes = total_scanned_bytes;
            outcome.coverage.scanned_files = total_scanned_files;
            outcome.coverage.scan_budget_bytes = granted_allowance;
            outcome.coverage.scan_budget_cap_bytes = cap;
            outcome.coverage.adaptive_rounds = rounds;
            outcome.coverage.confidence_milli =
                (confidence.clamp(0.0, 1.0) * 1000.0).round() as u16;

            let complete_no_match = outcome.hits.is_empty()
                && !outcome.truncated
                && outcome.coverage.policy_excluded_files == 0;
            let should_expand = !complete_no_match
                && outcome.truncated
                && outcome.adaptive_expandable
                && confidence < ADAPTIVE_CONFIDENCE_STOP
                && target < cap
                && !search_timed_out(started);
            if !should_expand {
                self.remember_search(query, &outcome.hits, context);
                return Ok(outcome);
            }

            let next = target.saturating_mul(2).min(cap);
            if next <= target {
                self.remember_search(query, &outcome.hits, context);
                return Ok(outcome);
            }
            target = next;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn search_once(
        &self,
        query: &NormalizedQuery,
        max_results: usize,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
        round_budget_bytes: usize,
        policy_skips: &mut HashMap<String, SourceStamp>,
        verification_cache: &mut HashMap<String, VerifiedCandidate>,
        context: Option<&CoordinationContext>,
    ) -> Result<SearchOutcome, RepositoryAccessError> {
        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }
        let terms = &query.terms;
        if max_results == 0 {
            return Ok(SearchOutcome {
                hits: Vec::new(),
                truncated: false,
                coverage: SearchCoverage {
                    scan_budget_bytes: round_budget_bytes,
                    scan_budget_cap_bytes: self.max_scan_budget_bytes,
                    adaptive_rounds: 1,
                    ..SearchCoverage::default()
                },
                adaptive_expandable: false,
            });
        }

        let requested_results = max_results.min(MAX_SEARCH_RESULTS);
        let candidate_limit = requested_results
            .saturating_mul(16)
            .clamp(64, MAX_SEARCH_CANDIDATES);
        let discovery = self.discover_files(cancellation, started, policy_skips)?;
        let eligible_paths = discovery
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();

        // Reconcile the volatile index with current discovery metadata. A truncated discovery is
        // only a lower bound on the live repository, so previously-indexed paths not observed in
        // that partial walk are retained rather than incorrectly treated as deleted.
        let mut pending = Vec::new();
        {
            let mut index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            if !discovery.truncated {
                index.files.retain(|path, _| eligible_paths.contains(path));
            }
            index.total_entries = index
                .files
                .values()
                .map(|doc| doc.terms.len().saturating_add(doc.substring_grams.len()))
                .sum();
            index.saturated = false;

            for file in &discovery.files {
                let path_bonus = path_match_score(&file.path, terms);
                let changed = index
                    .files
                    .get(&file.path)
                    .is_some_and(|doc| doc.stamp != file.stamp);
                let missing = !index.files.contains_key(&file.path);
                if changed {
                    if let Some(old) = index.files.remove(&file.path) {
                        index.total_entries = index.total_entries.saturating_sub(
                            old.terms.len().saturating_add(old.substring_grams.len()),
                        );
                    }
                }
                if missing || changed {
                    pending.push(PendingFile {
                        file: file.clone(),
                        path_bonus,
                        changed,
                    });
                }
            }
        }

        let (priority_lane, sample_lane, broad_lane) = stratified_pending_lanes(pending);
        // Reserve one quarter of the configured budget for verifying model-visible evidence from
        // the index. The remaining three quarters grow/refresh index coverage.
        let index_budget = round_budget_bytes.saturating_mul(3) / 4;
        let verify_budget = round_budget_bytes.saturating_sub(index_budget);
        let priority_cap = index_budget / 2;
        let sample_cap = index_budget / 8;
        let mut indexed_read_bytes = 0usize;
        let mut scanned_files = 0usize;
        let mut scan_incomplete = false;
        let mut scanned_paths = HashSet::new();

        let priority = self.scan_index_lane(
            &priority_lane,
            priority_cap,
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(priority.bytes);
        scanned_files = scanned_files.saturating_add(priority.files);
        scan_incomplete |= priority.incomplete;

        let remaining_after_priority = index_budget.saturating_sub(indexed_read_bytes);
        let sample = self.scan_index_lane(
            &sample_lane,
            sample_cap.min(remaining_after_priority),
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(sample.bytes);
        scanned_files = scanned_files.saturating_add(sample.files);
        scan_incomplete |= sample.incomplete;

        let remaining_for_broad = index_budget.saturating_sub(indexed_read_bytes);
        let broad = self.scan_index_lane(
            &broad_lane,
            remaining_for_broad,
            started,
            cancellation,
            &mut scanned_paths,
            policy_skips,
        )?;
        indexed_read_bytes = indexed_read_bytes.saturating_add(broad.bytes);
        scanned_files = scanned_files.saturating_add(broad.files);
        scan_incomplete |= broad.incomplete;

        if is_cancelled(cancellation) {
            return Err(RepositoryAccessError::Cancelled);
        }

        // Corpus statistics come from the current RAM index. Query matching uses stable hashes of
        // full identifiers plus common identifier subterms; final evidence is always re-read and
        // checked with the original text matcher before becoming model-visible.
        let indexed_query = terms
            .iter()
            .map(|term| (stable_term_hash(term), query_substring_grams(term)))
            .collect::<Vec<_>>();
        let mut document_frequencies = vec![0usize; terms.len()];
        let mut document_count = 0usize;
        let mut total_document_len = 0usize;
        let mut ranked = Vec::new();
        let mut indexed_paths = HashSet::new();
        {
            let index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            for (path, document) in &index.files {
                if !eligible_paths.contains(path) {
                    continue;
                }
                document_count = document_count.saturating_add(1);
                total_document_len = total_document_len.saturating_add(document.document_len);
                let frequencies = indexed_query_frequencies(document, &indexed_query);
                for (position, frequency) in frequencies.iter().enumerate() {
                    if *frequency > 0 {
                        document_frequencies[position] =
                            document_frequencies[position].saturating_add(1);
                    }
                }
                let path_bonus = path_match_score(path, terms);
                let has_content = frequencies.iter().any(|frequency| *frequency > 0);
                indexed_paths.insert(path.clone());
                if has_content || path_bonus > 0 {
                    ranked.push(RankedCandidate {
                        relative_path: path.clone(),
                        term_frequencies: frequencies,
                        document_len: document.document_len,
                        path_bonus,
                        has_content,
                        score: 0.0,
                    });
                }
            }
        }

        // A path match is still useful even when its body has not yet entered the incremental index.
        for file in &discovery.files {
            if indexed_paths.contains(&file.path) {
                continue;
            }
            if file
                .stamp
                .as_ref()
                .is_some_and(|stamp| policy_skips.get(&file.path) == Some(stamp))
            {
                continue;
            }
            let path_bonus = path_match_score(&file.path, terms);
            if path_bonus > 0 {
                ranked.push(RankedCandidate {
                    relative_path: file.path.clone(),
                    term_frequencies: vec![0; terms.len()],
                    document_len: 1,
                    path_bonus,
                    has_content: false,
                    score: (path_bonus * 3) as f64,
                });
            }
        }

        let average_document_len = if document_count == 0 {
            1.0
        } else {
            total_document_len as f64 / document_count as f64
        };
        for candidate in &mut ranked {
            let memory_bonus = self.memory_adjustment(terms, &candidate.relative_path, context);
            if candidate.has_content {
                let matched = candidate
                    .term_frequencies
                    .iter()
                    .filter(|frequency| **frequency > 0)
                    .count();
                let bm25 = bm25_score(
                    &candidate.term_frequencies,
                    candidate.document_len,
                    average_document_len,
                    &document_frequencies,
                    document_count,
                );
                candidate.score =
                    (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64
                        + bm25 * 12.0
                        + memory_bonus;
            } else {
                candidate.score = (candidate.path_bonus * 3) as f64 + memory_bonus.min(4.0);
            }
        }
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        // Candidate generation may contain false positives (for example n-gram collisions).
        // If we discard any candidates before exact verification, the search space is incomplete and
        // must never be upgraded to a complete NO_MATCH.
        let candidate_generation_truncated = ranked.len() > candidate_limit;
        ranked.truncate(candidate_limit);

        let stamps = discovery
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.stamp.clone()))
            .collect::<HashMap<_, _>>();
        let query_lower = &query.raw_lower;
        let mut verified_bytes = 0usize;
        let mut verification_incomplete = false;
        let mut hits = Vec::new();

        // Verify the whole bounded candidate set (subject to time/byte guards), then rank and cut
        // Top-N. Exact results are cached across adaptive rounds at the same SourceStamp so later
        // byte grants advance into new candidates instead of rereading the same leading files.
        for candidate in ranked {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                verification_incomplete = true;
                break;
            }

            if !candidate.has_content {
                let Some(Some(discovered_stamp)) = stamps.get(candidate.relative_path.as_str())
                else {
                    verification_incomplete = true;
                    continue;
                };
                match self.verified_metadata_stamp(&candidate.relative_path) {
                    Ok(verified_stamp) if &verified_stamp == discovered_stamp => {
                        hits.push(SearchHit {
                            relative_path: candidate.relative_path,
                            start_line: 0,
                            end_line: 0,
                            excerpt: String::new(),
                            score: candidate.score,
                            source_stamp: Some(verified_stamp),
                            source_fingerprint: None,
                        });
                    }
                    Ok(_) => {
                        verification_incomplete = true;
                    }
                    Err(error) => {
                        if matches!(error, RepositoryAccessError::HardLinkedFile) {
                            policy_skips
                                .insert(candidate.relative_path.clone(), discovered_stamp.clone());
                        }
                        if read_failure_makes_scan_incomplete(&error) {
                            verification_incomplete = true;
                        }
                    }
                }
                continue;
            }

            let Some(Some(discovered_stamp)) = stamps.get(candidate.relative_path.as_str()) else {
                verification_incomplete = true;
                continue;
            };

            // A cached exact result costs no bytes in this round. If discovery observed a different
            // generation, drop the stale derived evidence and verify the new generation normally.
            let cache_is_current = verification_cache
                .get(candidate.relative_path.as_str())
                .is_some_and(|verified| &verified.stamp == discovered_stamp);
            if cache_is_current {
                if let Some(verified) = verification_cache.get(candidate.relative_path.as_str()) {
                    if let Some(hit) = self.hit_from_verified_candidate(
                        &candidate,
                        verified,
                        average_document_len,
                        &document_frequencies,
                        document_count,
                        terms,
                        context,
                    ) {
                        hits.push(hit);
                    }
                }
                continue;
            }
            verification_cache.remove(candidate.relative_path.as_str());

            if verified_bytes >= verify_budget {
                verification_incomplete = true;
                break;
            }
            let remaining = verify_budget.saturating_sub(verified_bytes);
            if discovered_stamp.len > remaining as u64 {
                // This candidate cannot fit in the remaining verification budget, but a later
                // smaller candidate may still fit. Skip only this candidate instead of
                // truncating the rest of the ranked candidate set.
                verification_incomplete = true;
                continue;
            }

            let source = match self.read_source(&candidate.relative_path) {
                Ok(source) => source,
                Err((error, consumed_bytes)) => {
                    verified_bytes = verified_bytes.saturating_add(consumed_bytes);
                    if matches!(
                        error,
                        RepositoryAccessError::NonUtf8Source
                            | RepositoryAccessError::HardLinkedFile
                    ) {
                        policy_skips
                            .insert(candidate.relative_path.clone(), discovered_stamp.clone());
                    }
                    if read_failure_makes_scan_incomplete(&error) {
                        verification_incomplete = true;
                    }
                    continue;
                }
            };
            verified_bytes = verified_bytes.saturating_add(source.source_bytes);
            scanned_files = scanned_files.saturating_add(1);
            if verified_bytes > verify_budget {
                verification_incomplete = true;
            }
            if &source.stamp != discovered_stamp {
                // Discovery and capability-scoped verification did not observe the same file object.
                verification_incomplete = true;
                continue;
            }

            let indexed = build_indexed_document(&source.text, Some(source.stamp.clone()));
            self.insert_index_document(candidate.relative_path.clone(), indexed)?;

            let (document_len, term_frequencies) = term_statistics(&source.text, terms);
            let has_content_match = term_frequencies.iter().any(|frequency| *frequency > 0);
            let fingerprint = source_content_fingerprint(&source.text);
            let mut evidence_scan_complete = true;
            let evidence = if !has_content_match {
                VerifiedEvidence::None
            } else {
                let safe_text = redact_high_confidence_secrets(&source.text);
                let safe_lower = safe_text.to_ascii_lowercase();
                let redaction_suppressed_match = safe_text != source.text
                    && !terms.iter().any(|term| safe_lower.contains(term.as_str()));
                let lines = safe_text.lines().collect::<Vec<_>>();
                let mut best: Option<(SearchHit, usize, f64)> = None;
                for (index, lower) in safe_lower.lines().enumerate() {
                    if index % 256 == 0 {
                        if is_cancelled(cancellation) {
                            return Err(RepositoryAccessError::Cancelled);
                        }
                        if search_timed_out(started) {
                            verification_incomplete = true;
                            evidence_scan_complete = false;
                            break;
                        }
                    }
                    let Some(match_byte) = first_term_match_byte(lower, terms) else {
                        continue;
                    };
                    let (excerpt, start, end) = bounded_search_excerpt(&lines, index, match_byte);
                    let evidence_lower = excerpt.to_ascii_lowercase();
                    let matched = terms
                        .iter()
                        .filter(|term| evidence_lower.contains(term.as_str()))
                        .count();
                    let exact_bonus = usize::from(evidence_lower.contains(query_lower));
                    let structure_bonus = structural_line_bonus(lines[index], terms);
                    let preliminary = SearchHit {
                        relative_path: candidate.relative_path.clone(),
                        start_line: (start + 1) as u32,
                        end_line: end as u32,
                        excerpt,
                        score: (CONTENT_MATCH_BASE_SCORE
                            + matched * 10
                            + exact_bonus * 8
                            + candidate.path_bonus * 3) as f64
                            + structure_bonus,
                        source_stamp: Some(source.stamp.clone()),
                        source_fingerprint: Some(fingerprint),
                    };
                    if best
                        .as_ref()
                        .is_none_or(|(current, _, _)| hit_is_better(&preliminary, current))
                    {
                        best = Some((preliminary, exact_bonus, structure_bonus));
                    }
                }

                if let Some((hit, exact_bonus, structure_bonus)) = best {
                    VerifiedEvidence::Visible {
                        start_line: hit.start_line,
                        end_line: hit.end_line,
                        excerpt: hit.excerpt,
                        exact_bonus,
                        structure_bonus,
                    }
                } else if redaction_suppressed_match {
                    // Exact verification found the query in the original source, but the model-visible
                    // redacted form no longer contains any query term. Preserve the existence signal
                    // without revealing the matching secret value or its source line.
                    VerifiedEvidence::Redacted
                } else {
                    VerifiedEvidence::None
                }
            };

            let verified = VerifiedCandidate {
                stamp: source.stamp,
                fingerprint,
                document_len,
                term_frequencies,
                evidence,
            };
            if let Some(hit) = self.hit_from_verified_candidate(
                &candidate,
                &verified,
                average_document_len,
                &document_frequencies,
                document_count,
                terms,
                context,
            ) {
                hits.push(hit);
            }
            // Do not memoize a partially scanned evidence result at the shared deadline. A future
            // implementation with a refreshed deadline must be able to rescan it completely.
            if evidence_scan_complete {
                verification_cache.insert(candidate.relative_path, verified);
            }
        }

        sort_hits(&mut hits);
        hits.truncate(requested_results);

        // Files learned to be stable non-UTF-8 during this round are deliberate policy exclusions,
        // not unfinished retrieval. Remove them from the effective searchable set immediately.
        let newly_policy_excluded = eligible_paths
            .iter()
            .filter(|path| {
                let Some(stamp) = stamps.get(path.as_str()).cloned().flatten() else {
                    return false;
                };
                policy_skips.get(path.as_str()) == Some(&stamp)
            })
            .cloned()
            .collect::<HashSet<_>>();
        let effective_eligible_paths = eligible_paths
            .iter()
            .filter(|path| !newly_policy_excluded.contains(*path))
            .cloned()
            .collect::<HashSet<_>>();
        let eligible_files = effective_eligible_paths.len();
        let policy_excluded_files = discovery
            .policy_excluded_files
            .saturating_add(newly_policy_excluded.len());

        let (indexed_files, partial_index_files, saturated) = {
            let index = self
                .ram_index
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            let indexed_files = effective_eligible_paths
                .iter()
                .filter(|path| index.files.contains_key(path.as_str()))
                .count();
            let partial_index_files = effective_eligible_paths
                .iter()
                .filter_map(|path| index.files.get(path.as_str()))
                .filter(|doc| doc.term_truncated)
                .count();
            (indexed_files, partial_index_files, index.saturated)
        };
        let coverage = SearchCoverage {
            discovery_complete: !discovery.truncated,
            eligible_files,
            indexed_files,
            partial_index_files,
            policy_excluded_files,
            scanned_files,
            scanned_bytes: indexed_read_bytes.saturating_add(verified_bytes),
            scan_budget_bytes: round_budget_bytes,
            scan_budget_cap_bytes: self.max_scan_budget_bytes,
            adaptive_rounds: 1,
            confidence_milli: 0,
        };
        let adaptive_expandable = discovery.truncated
            || scan_incomplete
            || verification_incomplete
            || saturated
            || partial_index_files > 0
            || indexed_files < eligible_files;
        let truncated = candidate_generation_truncated || adaptive_expandable;
        Ok(SearchOutcome {
            hits,
            truncated,
            coverage,
            adaptive_expandable,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn hit_from_verified_candidate(
        &self,
        candidate: &RankedCandidate,
        verified: &VerifiedCandidate,
        average_document_len: f64,
        document_frequencies: &[usize],
        document_count: usize,
        terms: &[String],
        context: Option<&CoordinationContext>,
    ) -> Option<SearchHit> {
        let has_content_match = verified
            .term_frequencies
            .iter()
            .any(|frequency| *frequency > 0);
        if !has_content_match {
            return (candidate.path_bonus > 0).then(|| SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: 0,
                end_line: 0,
                excerpt: String::new(),
                score: (candidate.path_bonus * 3) as f64,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            });
        }

        let matched = verified
            .term_frequencies
            .iter()
            .filter(|frequency| **frequency > 0)
            .count();
        let bm25 = bm25_score(
            &verified.term_frequencies,
            verified.document_len,
            average_document_len,
            document_frequencies,
            document_count.max(1),
        );
        let memory_bonus = self.memory_adjustment(terms, &candidate.relative_path, context);
        match &verified.evidence {
            VerifiedEvidence::Visible {
                start_line,
                end_line,
                excerpt,
                exact_bonus,
                structure_bonus,
            } => Some(SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: *start_line,
                end_line: *end_line,
                excerpt: excerpt.clone(),
                score: (CONTENT_MATCH_BASE_SCORE
                    + matched * 10
                    + *exact_bonus * 8
                    + candidate.path_bonus * 3) as f64
                    + bm25 * 12.0
                    + *structure_bonus
                    + memory_bonus,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            }),
            VerifiedEvidence::Redacted => Some(SearchHit {
                relative_path: candidate.relative_path.clone(),
                start_line: 0,
                end_line: 0,
                excerpt: REDACTED_MATCH_EXCERPT.to_string(),
                score: (CONTENT_MATCH_BASE_SCORE + matched * 10 + candidate.path_bonus * 3) as f64
                    + bm25 * 12.0
                    + memory_bonus,
                source_stamp: Some(verified.stamp.clone()),
                source_fingerprint: Some(verified.fingerprint),
            }),
            VerifiedEvidence::None => None,
        }
    }

    pub(super) fn index_document_is_current(
        &self,
        path: &str,
        expected: Option<&SourceStamp>,
    ) -> Result<bool, RepositoryAccessError> {
        let index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        Ok(index.files.get(path).is_some_and(|document| {
            match (document.stamp.as_ref(), expected) {
                (_, None) => true,
                (Some(actual), Some(expected)) => actual == expected,
                (None, Some(_)) => false,
            }
        }))
    }

    pub(super) fn claim_index_flight(
        &self,
        path: &str,
        expected: Option<&SourceStamp>,
        started: &Instant,
        cancellation: Option<&AtomicBool>,
    ) -> Result<IndexFlightClaim, RepositoryAccessError> {
        loop {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                return Ok(IndexFlightClaim::TimedOut);
            }
            let mut inflight = self
                .index_inflight
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            if !inflight.contains(path) {
                // Recheck index state while owning the flight registry. This closes the race where
                // another worker finishes indexing between an earlier index check and flight claim.
                if self.index_document_is_current(path, expected)? {
                    return Ok(IndexFlightClaim::AlreadyIndexed);
                }
                inflight.insert(path.to_string());
                return Ok(IndexFlightClaim::Leader);
            }
            let (guard, _) = self
                .index_ready
                .wait_timeout(inflight, Duration::from_millis(20))
                .map_err(|_| RepositoryAccessError::Io)?;
            drop(guard);
        }
    }

    pub(super) fn release_index_flight(&self, path: &str) {
        let mut inflight = match self.index_inflight.lock() {
            Ok(inflight) => inflight,
            Err(poisoned) => poisoned.into_inner(),
        };
        inflight.remove(path);
        drop(inflight);
        self.index_ready.notify_all();
    }

    pub(super) fn scan_index_lane(
        &self,
        lane: &[PendingFile],
        byte_budget: usize,
        started: &Instant,
        cancellation: Option<&AtomicBool>,
        scanned_paths: &mut HashSet<String>,
        policy_skips: &mut HashMap<String, SourceStamp>,
    ) -> Result<ScanLaneOutcome, RepositoryAccessError> {
        let mut outcome = ScanLaneOutcome::default();
        if byte_budget == 0 {
            return Ok(outcome);
        }
        for pending in lane {
            if scanned_paths.contains(&pending.file.path) {
                continue;
            }
            if outcome.bytes >= byte_budget {
                break;
            }
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                outcome.incomplete = true;
                break;
            }
            // Keep the configured source-read budget strict when discovery metadata is available.
            // Unknown metadata can still overshoot by at most one bounded source read (2 MiB).
            if let Some(stamp) = &pending.file.stamp {
                let remaining = byte_budget.saturating_sub(outcome.bytes);
                if stamp.len > remaining as u64 {
                    continue;
                }
            }
            match self.claim_index_flight(
                &pending.file.path,
                pending.file.stamp.as_ref(),
                started,
                cancellation,
            )? {
                IndexFlightClaim::AlreadyIndexed => {
                    scanned_paths.insert(pending.file.path.clone());
                    continue;
                }
                IndexFlightClaim::TimedOut => {
                    outcome.incomplete = true;
                    break;
                }
                IndexFlightClaim::Leader => {}
            }
            let _index_flight = IndexFlightGuard::new(self, pending.file.path.clone());

            scanned_paths.insert(pending.file.path.clone());
            let source = match self.read_source(&pending.file.path) {
                Ok(source) => source,
                Err((error, consumed_bytes)) => {
                    outcome.bytes = outcome.bytes.saturating_add(consumed_bytes);
                    if matches!(
                        error,
                        RepositoryAccessError::NonUtf8Source
                            | RepositoryAccessError::HardLinkedFile
                    ) {
                        if let Some(stamp) = pending.file.stamp.clone() {
                            policy_skips.insert(pending.file.path.clone(), stamp);
                        }
                    }
                    if read_failure_makes_scan_incomplete(&error) {
                        outcome.incomplete = true;
                    }
                    continue;
                }
            };
            outcome.bytes = outcome.bytes.saturating_add(source.source_bytes);
            outcome.files = outcome.files.saturating_add(1);
            let document = build_indexed_document(&source.text, Some(source.stamp.clone()));
            self.insert_index_document(pending.file.path.clone(), document)?;
        }
        Ok(outcome)
    }

    pub(super) fn insert_index_document(
        &self,
        path: String,
        document: IndexedDocument,
    ) -> Result<(), RepositoryAccessError> {
        let mut index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        if let Some(old) = index.files.remove(&path) {
            index.total_entries = index
                .total_entries
                .saturating_sub(old.terms.len().saturating_add(old.substring_grams.len()));
        }
        let document_entries = document
            .terms
            .len()
            .saturating_add(document.substring_grams.len());
        if index.total_entries.saturating_add(document_entries) > MAX_INDEX_TOTAL_ENTRIES {
            index.saturated = true;
            return Ok(());
        }
        index.total_entries = index.total_entries.saturating_add(document_entries);
        index.files.insert(path, document);
        Ok(())
    }

    pub(super) fn analyze_source_cached(
        &self,
        path: &str,
        safe_source: &str,
        stamp: &SourceStamp,
        cancellation: Option<&AtomicBool>,
        deadline: Instant,
    ) -> Result<Option<CachedAnalysis>, RepositoryAccessError> {
        loop {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut state = self
                .analysis_cache
                .lock()
                .map_err(|_| RepositoryAccessError::Io)?;
            state.tick = state.tick.saturating_add(1);
            let tick = state.tick;
            if state
                .entries
                .get(path)
                .is_some_and(|entry| &entry.stamp == stamp)
            {
                if let Some(entry) = state.entries.get_mut(path) {
                    entry.last_used = tick;
                    return Ok(Some(entry.clone()));
                }
            }
            if state.entries.contains_key(path) {
                state.entries.remove(path);
            }
            if state.inflight.contains(path) {
                let (guard, _) = self
                    .analysis_ready
                    .wait_timeout(state, Duration::from_millis(20))
                    .map_err(|_| RepositoryAccessError::Io)?;
                drop(guard);
                continue;
            }
            state.inflight.insert(path.to_string());
            break;
        }
        let _analysis_flight = AnalysisFlightGuard::new(self, path.to_string());

        let computed = (|| {
            let syntax_supported = supports_tree_sitter_path(path);
            let ast_symbols =
                extract_ast_symbols_bounded(path, safe_source, 64, cancellation, Some(deadline));
            let ast_cacheable = ast_symbols.is_some() || !syntax_supported;
            let mut symbols = ast_symbols
                .unwrap_or_default()
                .into_iter()
                .map(|symbol| CachedRepoMapSymbol {
                    name: symbol.name,
                    kind: symbol.kind,
                    line: symbol.line,
                })
                .collect::<Vec<_>>();
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            let mut seen = symbols
                .iter()
                .map(|symbol| symbol.name.clone())
                .collect::<HashSet<_>>();
            for symbol in extract_symbols(safe_source, 64) {
                if symbols.len() >= 64 {
                    break;
                }
                if seen.insert(symbol.name.clone()) {
                    symbols.push(CachedRepoMapSymbol {
                        name: symbol.name,
                        kind: symbol.kind,
                        line: symbol.line,
                    });
                }
            }
            let semantic_facts = extract_semantic_facts_bounded(
                path,
                safe_source,
                512,
                64,
                cancellation,
                Some(deadline),
            );
            let semantic_cacheable = semantic_facts.is_some() || !syntax_supported;
            let semantics = semantic_facts.unwrap_or_default();
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            Ok(Some(CachedAnalysis {
                stamp: stamp.clone(),
                symbols,
                semantics,
                cacheable: ast_cacheable && semantic_cacheable,
                last_used: 0,
            }))
        })();

        let mut state = self
            .analysis_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        if let Ok(Some(mut entry)) = computed.clone() {
            if entry.cacheable {
                state.tick = state.tick.saturating_add(1);
                entry.last_used = state.tick;
                if state.entries.len() >= MAX_ANALYSIS_CACHE_FILES
                    && !state.entries.contains_key(path)
                {
                    if let Some(evict) = state
                        .entries
                        .iter()
                        .min_by_key(|(_, cached)| cached.last_used)
                        .map(|(path, _)| path.clone())
                    {
                        state.entries.remove(&evict);
                    }
                }
                state.entries.insert(path.to_string(), entry);
            }
        }
        drop(state);
        computed
    }
}
