use super::*;

impl RepositoryAccess {
    pub(super) fn graph_cache_get(
        &self,
        key: &GraphCacheKey,
    ) -> Result<Option<CachedGraph>, RepositoryAccessError> {
        let mut cache = self
            .graph_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        cache.tick = cache.tick.saturating_add(1);
        let tick = cache.tick;
        if let Some(entry) = cache.entries.get_mut(key) {
            entry.last_used = tick;
            return Ok(Some(entry.clone()));
        }
        Ok(None)
    }

    pub(super) fn graph_cache_put(
        &self,
        key: GraphCacheKey,
        mut graph: CachedGraph,
    ) -> Result<(), RepositoryAccessError> {
        let mut cache = self
            .graph_cache
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        cache.tick = cache.tick.saturating_add(1);
        graph.last_used = cache.tick;
        if cache.entries.len() >= MAX_GRAPH_CACHE_ENTRIES && !cache.entries.contains_key(&key) {
            if let Some(evict) = cache
                .entries
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(key, _)| key.clone())
            {
                cache.entries.remove(&evict);
            }
        }
        cache.entries.insert(key, graph);
        Ok(())
    }

    /// Builds a query-focused structural graph from an already-ranked bounded candidate set.
    /// This avoids a second repository-wide search when `repo_context` needs both retrieval and
    /// structural evidence in one MCP call.
    #[cfg(test)]
    pub fn map_from_hits(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        max_files: usize,
        cancellation: Option<&AtomicBool>,
    ) -> Result<RepositoryMapOutcome, RepositoryAccessError> {
        let started = Instant::now();
        self.map_from_hits_since(query, hits, max_files, cancellation, &started)
    }

    pub fn map_from_hits_since(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        max_files: usize,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
    ) -> Result<RepositoryMapOutcome, RepositoryAccessError> {
        // Windows' stable metadata surface at this MSRV cannot distinguish every same-size,
        // same-mtime replacement. The verified open-handle stamp still protects each individual
        // read from concurrent mutation, but it is not a safe cross-request content identity.
        // Serialize top-level map construction and discard prior structural caches before reading
        // candidates so stale symbols, semantic facts, or graph edges cannot cross requests.
        #[cfg(windows)]
        let _windows_map_guard = self
            .windows_map_serial
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        #[cfg(windows)]
        self.reset_structural_caches()?;

        let mut truncated = false;
        let mut candidates = Vec::<MapCandidate>::new();
        let mut map_source_bytes = 0usize;
        let mut map_redacted_bytes = 0usize;
        let mut map_folded_bytes = 0usize;
        let mut invalidated_evidence_paths = Vec::<String>::new();
        let structural_limit = max_files.min(16);
        let mut structural_collection_enabled = true;

        // Revalidate every returned search hit before any excerpt is rendered. This is especially
        // important on Windows: size + mtime can be preserved across a same-length rewrite, so an
        // adaptive-round verification cache can otherwise carry a stale excerpt into the final
        // context. Structural analysis remains limited to `structural_limit`; lower-ranked hits are
        // read only for generation/fingerprint validation and are not retained as source bodies.
        for (hit_index, hit) in hits.iter().enumerate() {
            if is_cancelled(cancellation) {
                return Err(RepositoryAccessError::Cancelled);
            }
            if search_timed_out(started) {
                truncated = true;
                invalidated_evidence_paths.extend(
                    hits[hit_index..]
                        .iter()
                        .map(|remaining| remaining.relative_path.clone()),
                );
                break;
            }
            let source = match self.read_source(&hit.relative_path) {
                Ok(source) => source,
                Err((_error, _)) => {
                    // If the current file cannot be re-opened and re-verified, the previously
                    // collected excerpt is no longer safe to present as current evidence.
                    invalidated_evidence_paths.push(hit.relative_path.clone());
                    truncated = true;
                    continue;
                }
            };
            if hit
                .source_stamp
                .as_ref()
                .is_some_and(|expected| expected != &source.stamp)
            {
                // Evidence and structure must describe one file generation. A changed candidate is
                // omitted rather than mixing stale evidence with fresh structural analysis.
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }
            if hit
                .source_fingerprint
                .is_some_and(|expected| expected != source_content_fingerprint(&source.text))
            {
                // SourceStamp is intentionally conservative but Windows can preserve size + mtime
                // across a rewrite. The content fingerprint closes that within-call consistency gap.
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }

            // Evidence for this hit is current. Lower-ranked hits need no structural work, and once
            // the structural budget/deadline is exhausted we keep validating evidence without
            // retaining or analyzing additional source bodies.
            if hit_index >= structural_limit || !structural_collection_enabled {
                continue;
            }
            map_source_bytes = map_source_bytes.saturating_add(source.source_bytes);
            if map_source_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }

            // Redaction markers can be longer than the secret they replace (for example
            // `token="x"`). Bound the redacted representation before analysis so a crafted
            // repository cannot turn the 32 MiB raw-source budget into hundreds of MiB of retained
            // folded buffers. The bounded redactor also suppresses giant single lines before any
            // allocating per-line redactor sees them.
            let redaction = redact_high_confidence_secrets_bounded(&source.text, MAX_SOURCE_BYTES);
            if redaction.truncated {
                truncated = true;
            }
            map_redacted_bytes = map_redacted_bytes.saturating_add(redaction.text.len());
            if map_redacted_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }
            let safe = redaction.text;
            let Some(analysis) = self.analyze_source_cached(
                &hit.relative_path,
                &safe,
                &source.stamp,
                cancellation,
                *started + MAX_SEARCH_WALL_TIME,
            )?
            else {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            };
            let mut definition_names = analysis
                .symbols
                .iter()
                .map(|symbol| crate::core::unicode_search_fold(&symbol.name))
                .filter(|name| name.len() >= 2)
                .collect::<Vec<_>>();
            definition_names.sort();
            definition_names.dedup();

            // Shared analysis caches contain structural metadata only. Rehydrate display/ranking
            // signatures from the freshly verified, redacted source for this call so source-line
            // text never persists in the cross-agent cache.
            let safe_lines = safe.lines().collect::<Vec<_>>();
            let mut symbols = analysis
                .symbols
                .iter()
                .map(|symbol| RepoMapSymbol {
                    name: symbol.name.clone(),
                    kind: symbol.kind.clone(),
                    line: symbol.line,
                    signature: signature_from_lines(&safe_lines, symbol.line),
                })
                .collect::<Vec<_>>();
            symbols.sort_by(|a, b| {
                let score = |symbol: &RepoMapSymbol| {
                    let name = crate::core::unicode_search_fold(&symbol.name);
                    let signature = crate::core::unicode_search_fold(&symbol.signature);
                    query
                        .terms
                        .iter()
                        .map(|term| {
                            if name.as_str() == term.as_str() {
                                6usize
                            } else if name.contains(term.as_str()) {
                                4usize
                            } else if signature.contains(term.as_str()) {
                                2usize
                            } else {
                                0usize
                            }
                        })
                        .sum::<usize>()
                };
                score(b)
                    .cmp(&score(a))
                    .then_with(|| a.line.cmp(&b.line))
                    .then_with(|| a.name.cmp(&b.name))
            });
            symbols.truncate(12);

            // Tier 2 is source-only semantic analysis. The parsed facts are shared across agents
            // only while the verified source stamp remains unchanged.
            let semantics = analysis.semantics.clone();
            let semantic_query_bonus = semantics
                .references
                .iter()
                .map(|reference| {
                    let name = crate::core::unicode_search_fold(&reference.name);
                    let overlap = query
                        .terms
                        .iter()
                        .filter(|term| name.contains(term.as_str()))
                        .count() as f64;
                    let weight = match reference.kind.as_str() {
                        "implementation" => 1.0,
                        "call" => 0.9,
                        "type" => 0.85,
                        _ => 0.6,
                    };
                    overlap * weight
                })
                .sum::<f64>()
                .min(8.0);

            drop(safe_lines);
            let source_lower = crate::core::unicode_search_fold(&safe);
            map_folded_bytes = map_folded_bytes.saturating_add(source_lower.len());
            if map_folded_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }

            candidates.push(MapCandidate {
                relative_path: hit.relative_path.clone(),
                stamp: source.stamp,
                search_score: hit.score,
                source_lower,
                symbols,
                definition_names,
                semantics,
                analysis_cacheable: analysis.cacheable,
                semantic_query_bonus,
            });
        }

        // Canonicalize candidate order so sibling agents that discover the same file set in a
        // different ranking order can share the exact same structural graph cache entry.
        candidates.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));

        let graph_key = GraphCacheKey(
            candidates
                .iter()
                .map(|candidate| GraphCacheNode {
                    path: candidate.relative_path.clone(),
                    stamp: candidate.stamp.clone(),
                })
                .collect(),
        );
        let graph_cacheable = !candidates.is_empty()
            && candidates
                .iter()
                .all(|candidate| candidate.analysis_cacheable);
        let cached_graph = if graph_cacheable {
            self.graph_cache_get(&graph_key)?
        } else {
            None
        };
        let (edge_maps, centrality) = if let Some(cached) = cached_graph {
            (cached.edge_maps, cached.centrality)
        } else {
            let mut definition_targets = HashMap::<String, Vec<usize>>::new();
            for (to, candidate) in candidates.iter().enumerate() {
                for name in &candidate.definition_names {
                    definition_targets.entry(name.clone()).or_default().push(to);
                }
            }

            // Strongest evidence wins for each file pair: implementation .95, call .90, type .85,
            // exact reference .80, import .40, lexical coincidence .15.
            let mut edge_maps = vec![HashMap::<usize, (f64, String)>::new(); candidates.len()];
            for (from, candidate) in candidates.iter().enumerate() {
                if is_cancelled(cancellation) {
                    return Err(RepositoryAccessError::Cancelled);
                }
                if search_timed_out(started) {
                    truncated = true;
                    break;
                }
                for reference in &candidate.semantics.references {
                    let key = crate::core::unicode_search_fold(&reference.name);
                    let Some(targets) = definition_targets.get(&key) else {
                        continue;
                    };
                    let weight = match reference.kind.as_str() {
                        "implementation" => 0.95,
                        "call" => 0.90,
                        "type" => 0.85,
                        _ => 0.80,
                    };
                    for &to in targets {
                        upsert_repo_edge(&mut edge_maps, from, to, weight, reference.kind.as_str());
                    }
                }

                for import_path in &candidate.semantics.import_paths {
                    let import_lower = crate::core::unicode_search_fold(import_path);
                    for (to, target) in candidates.iter().enumerate() {
                        if to == from {
                            continue;
                        }
                        let path = crate::core::unicode_search_fold(&target.relative_path);
                        let stem = Path::new(&path)
                            .file_stem()
                            .and_then(|value| value.to_str())
                            .unwrap_or("");
                        let path_no_ext = Path::new(&path)
                            .with_extension("")
                            .to_string_lossy()
                            .replace('\\', "/");
                        let module_path = path_no_ext.trim_end_matches("/mod");
                        if (!stem.is_empty() && import_lower.ends_with(stem))
                            || (!module_path.is_empty()
                                && (import_lower.ends_with(module_path)
                                    || module_path.ends_with(import_lower.as_str())))
                        {
                            upsert_repo_edge(&mut edge_maps, from, to, 0.40, "import");
                        }
                    }
                }
            }

            // Preserve RC25's Aho-Corasick structural hint as a weak fallback only.
            let mut patterns = Vec::<String>::new();
            let mut pattern_targets = Vec::<Vec<usize>>::new();
            let mut pattern_ids = HashMap::<String, usize>::new();
            for (to, candidate) in candidates.iter().enumerate() {
                for symbol_lower in &candidate.definition_names {
                    if symbol_lower.len() < 4 {
                        continue;
                    }
                    if let Some(&pattern_id) = pattern_ids.get(symbol_lower) {
                        if !pattern_targets[pattern_id].contains(&to) {
                            pattern_targets[pattern_id].push(to);
                        }
                    } else {
                        let pattern_id = patterns.len();
                        pattern_ids.insert(symbol_lower.clone(), pattern_id);
                        patterns.push(symbol_lower.clone());
                        pattern_targets.push(vec![to]);
                    }
                }
            }
            if !truncated && !patterns.is_empty() {
                match AhoCorasick::new(patterns.iter().map(String::as_str)) {
                    Ok(matcher) => {
                        for (from, candidate) in candidates.iter().enumerate() {
                            if is_cancelled(cancellation) {
                                return Err(RepositoryAccessError::Cancelled);
                            }
                            if search_timed_out(started) {
                                truncated = true;
                                break;
                            }
                            for (match_count, found) in matcher
                                .find_overlapping_iter(candidate.source_lower.as_bytes())
                                .enumerate()
                            {
                                if match_count % 1024 == 0 {
                                    if is_cancelled(cancellation) {
                                        return Err(RepositoryAccessError::Cancelled);
                                    }
                                    if search_timed_out(started) {
                                        truncated = true;
                                        break;
                                    }
                                }
                                for &to in &pattern_targets[found.pattern().as_usize()] {
                                    upsert_repo_edge(&mut edge_maps, from, to, 0.15, "lexical");
                                }
                            }
                            if truncated {
                                break;
                            }
                        }
                    }
                    Err(_) => truncated = true,
                }
            }

            let weighted_edges = edge_maps
                .iter()
                .map(|targets| {
                    let mut edges = targets
                        .iter()
                        .map(|(to, (weight, _))| (*to, *weight))
                        .collect::<Vec<_>>();
                    edges.sort_by_key(|(to, _)| *to);
                    edges
                })
                .collect::<Vec<Vec<(usize, f64)>>>();
            let centrality = weighted_pagerank(&weighted_edges, 12);
            if truncated || !graph_cacheable {
                (edge_maps, centrality)
            } else {
                self.graph_cache_put(
                    graph_key,
                    CachedGraph {
                        edge_maps: edge_maps.clone(),
                        centrality: centrality.clone(),
                        last_used: 0,
                    },
                )?;
                (edge_maps, centrality)
            }
        };
        let candidate_paths = candidates
            .iter()
            .map(|candidate| candidate.relative_path.clone())
            .collect::<Vec<_>>();
        let mut entries = candidates
            .into_iter()
            .enumerate()
            .map(|(index, candidate)| {
                let mut semantic_links = edge_maps[index]
                    .iter()
                    .filter_map(|(target, (weight, kind))| {
                        candidate_paths.get(*target).map(|path| RepoMapLink {
                            relative_path: path.clone(),
                            kind: kind.clone(),
                            weight: *weight,
                        })
                    })
                    .collect::<Vec<_>>();
                semantic_links.sort_by(|a, b| {
                    b.weight
                        .partial_cmp(&a.weight)
                        .unwrap_or(Ordering::Equal)
                        .then_with(|| a.relative_path.cmp(&b.relative_path))
                });
                semantic_links.truncate(6);
                let links_to = semantic_links
                    .iter()
                    .map(|link| link.relative_path.clone())
                    .collect();
                RepoMapEntry {
                    relative_path: candidate.relative_path,
                    score: candidate.search_score
                        + candidate.semantic_query_bonus * 3.0
                        + centrality.get(index).copied().unwrap_or(0.0) * 50.0,
                    symbols: candidate.symbols,
                    links_to,
                    semantic_links,
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });
        entries.truncate(max_files.min(16));
        invalidated_evidence_paths.sort();
        invalidated_evidence_paths.dedup();
        Ok(RepositoryMapOutcome {
            entries,
            truncated,
            invalidated_evidence_paths,
        })
    }
}
