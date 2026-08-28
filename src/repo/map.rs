use super::*;

const MAX_SEMANTIC_EXPANSION_FILES: usize = 8;
const MAX_EXPANSION_IMPORTS_PER_SEED: usize = 16;

struct BuiltMapCandidate {
    candidate: MapCandidate,
    redacted_bytes: usize,
    folded_bytes: usize,
    redaction_truncated: bool,
}

fn normalized_module_name(value: &str) -> String {
    let mut normalized = crate::core::unicode_search_fold(value)
        .replace("::", "/")
        .replace(['.', '\\'], "/");
    while normalized.starts_with("./") || normalized.starts_with("../") {
        normalized = normalized
            .strip_prefix("./")
            .or_else(|| normalized.strip_prefix("../"))
            .unwrap_or(&normalized)
            .to_string();
    }
    for prefix in ["crate/", "self/", "super/"] {
        normalized = normalized.strip_prefix(prefix).unwrap_or(&normalized).to_string();
    }
    normalized.trim_matches('/').to_string()
}

fn normalized_path_module(path: &str) -> String {
    let folded = crate::core::unicode_search_fold(path).replace('\\', "/");
    let no_ext = Path::new(&folded)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/");
    no_ext
        .trim_end_matches("/mod")
        .trim_end_matches("/index")
        .trim_matches('/')
        .to_string()
}

fn ranked_symbols(
    query: &NormalizedQuery,
    safe: &str,
    analysis: &CachedAnalysis,
) -> Vec<RepoMapSymbol> {
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
    symbols
}

fn semantic_query_bonus(query: &NormalizedQuery, semantics: &SemanticFacts) -> f64 {
    semantics
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
        .min(8.0)
}

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
        self.map_from_hits_with_snapshots_since(
            query,
            hits,
            &HashMap::new(),
            max_files,
            cancellation,
            started,
        )
    }

    pub(crate) fn map_from_hits_with_snapshots_since(
        &self,
        query: &NormalizedQuery,
        hits: &[SearchHit],
        snapshots: &HashMap<String, Arc<str>>,
        max_files: usize,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
    ) -> Result<RepositoryMapOutcome, RepositoryAccessError> {
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

            let source = match self.verified_source_for_map_hit(hit, snapshots)? {
                Some(source) => source,
                None => {
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
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }
            if hit
                .source_fingerprint
                .is_some_and(|expected| expected != source_content_fingerprint(&source.text))
            {
                truncated = true;
                invalidated_evidence_paths.push(hit.relative_path.clone());
                continue;
            }

            if hit_index >= structural_limit || !structural_collection_enabled {
                continue;
            }
            map_source_bytes = map_source_bytes.saturating_add(source.source_bytes);
            if map_source_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }

            let Some(built) = self.build_map_candidate(
                query,
                &hit.relative_path,
                source,
                hit.score,
                cancellation,
                started,
            )?
            else {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            };
            truncated |= built.redaction_truncated;
            map_redacted_bytes = map_redacted_bytes.saturating_add(built.redacted_bytes);
            map_folded_bytes = map_folded_bytes.saturating_add(built.folded_bytes);
            if map_redacted_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES
                || map_folded_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES
            {
                truncated = true;
                structural_collection_enabled = false;
                continue;
            }
            candidates.push(built.candidate);
        }

        // One-hop source-only semantic expansion lets a lexically discovered seed pull in a bounded
        // imported/module neighbor that did not itself contain the query terms. Expanded files are
        // structural evidence only; they do not manufacture a source excerpt or bypass verification.
        if structural_collection_enabled && !search_timed_out(started) {
            let expansion_paths = self.semantic_expansion_paths(&candidates, MAX_SEMANTIC_EXPANSION_FILES)?;
            for (path, expansion_score) in expansion_paths {
                if is_cancelled(cancellation) {
                    return Err(RepositoryAccessError::Cancelled);
                }
                if search_timed_out(started) {
                    truncated = true;
                    break;
                }
                let source = match self.read_source(&path) {
                    Ok(source) => source,
                    Err((_error, _)) => {
                        truncated = true;
                        continue;
                    }
                };
                map_source_bytes = map_source_bytes.saturating_add(source.source_bytes);
                if map_source_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES {
                    truncated = true;
                    break;
                }
                let Some(built) = self.build_map_candidate(
                    query,
                    &path,
                    source,
                    expansion_score,
                    cancellation,
                    started,
                )?
                else {
                    truncated = true;
                    continue;
                };
                truncated |= built.redaction_truncated;
                map_redacted_bytes = map_redacted_bytes.saturating_add(built.redacted_bytes);
                map_folded_bytes = map_folded_bytes.saturating_add(built.folded_bytes);
                if map_redacted_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES
                    || map_folded_bytes > MAX_REPOSITORY_MAP_SOURCE_BYTES
                {
                    truncated = true;
                    break;
                }
                candidates.push(built.candidate);
            }
        }

        candidates.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        candidates.dedup_by(|a, b| a.relative_path == b.relative_path);

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

    fn verified_source_for_map_hit(
        &self,
        hit: &SearchHit,
        snapshots: &HashMap<String, Arc<str>>,
    ) -> Result<Option<VerifiedSource>, RepositoryAccessError> {
        #[cfg(not(windows))]
        if let (Some(snapshot), Some(expected_stamp)) = (
            snapshots.get(hit.relative_path.as_str()),
            hit.source_stamp.as_ref(),
        ) {
            let current = match self.verified_metadata_stamp(&hit.relative_path) {
                Ok(stamp) => stamp,
                Err(_) => return Ok(None),
            };
            if &current != expected_stamp {
                return Ok(None);
            }
            if hit
                .source_fingerprint
                .is_some_and(|expected| expected != source_content_fingerprint(snapshot))
            {
                return Ok(None);
            }
            return Ok(Some(VerifiedSource {
                text: snapshot.to_string(),
                source_bytes: snapshot.len(),
                stamp: current,
            }));
        }

        match self.read_source(&hit.relative_path) {
            Ok(source) => Ok(Some(source)),
            Err((_error, _)) => Ok(None),
        }
    }

    fn build_map_candidate(
        &self,
        query: &NormalizedQuery,
        path: &str,
        source: VerifiedSource,
        search_score: f64,
        cancellation: Option<&AtomicBool>,
        started: &Instant,
    ) -> Result<Option<BuiltMapCandidate>, RepositoryAccessError> {
        let redaction = redact_high_confidence_secrets_bounded(&source.text, MAX_SOURCE_BYTES);
        let redaction_truncated = redaction.truncated;
        let redacted_bytes = redaction.text.len();
        let safe = redaction.text;
        let Some(analysis) = self.analyze_source_cached(
            path,
            &safe,
            &source.stamp,
            cancellation,
            *started + MAX_SEARCH_WALL_TIME,
        )?
        else {
            return Ok(None);
        };
        let mut definition_names = analysis
            .symbols
            .iter()
            .map(|symbol| crate::core::unicode_search_fold(&symbol.name))
            .filter(|name| name.len() >= 2)
            .collect::<Vec<_>>();
        definition_names.sort();
        definition_names.dedup();
        let symbols = ranked_symbols(query, &safe, &analysis);
        let semantics = analysis.semantics.clone();
        let semantic_query_bonus = semantic_query_bonus(query, &semantics);
        let source_lower = crate::core::unicode_search_fold(&safe);
        let folded_bytes = source_lower.len();
        Ok(Some(BuiltMapCandidate {
            candidate: MapCandidate {
                relative_path: path.to_string(),
                stamp: source.stamp,
                search_score,
                source_lower,
                symbols,
                definition_names,
                semantics,
                analysis_cacheable: analysis.cacheable,
                semantic_query_bonus,
            },
            redacted_bytes,
            folded_bytes,
            redaction_truncated,
        }))
    }

    fn semantic_expansion_paths(
        &self,
        seeds: &[MapCandidate],
        limit: usize,
    ) -> Result<Vec<(String, f64)>, RepositoryAccessError> {
        if limit == 0 || seeds.is_empty() {
            return Ok(Vec::new());
        }
        let existing = seeds
            .iter()
            .map(|candidate| candidate.relative_path.as_str())
            .collect::<HashSet<_>>();
        let index = self
            .ram_index
            .lock()
            .map_err(|_| RepositoryAccessError::Io)?;
        let mut by_stem = HashMap::<String, Vec<String>>::new();
        let mut by_module = HashMap::<String, String>::new();
        for path in index.files.keys() {
            if existing.contains(path.as_str()) {
                continue;
            }
            let module = normalized_path_module(path);
            if !module.is_empty() {
                by_module.entry(module.clone()).or_insert_with(|| path.clone());
            }
            if let Some(stem) = Path::new(path).file_stem().and_then(|value| value.to_str()) {
                let stem = crate::core::unicode_search_fold(stem);
                if stem.len() >= 2 {
                    by_stem.entry(stem).or_default().push(path.clone());
                }
            }
        }
        drop(index);

        let mut scores = HashMap::<String, f64>::new();
        for seed in seeds {
            for import in seed
                .semantics
                .import_paths
                .iter()
                .take(MAX_EXPANSION_IMPORTS_PER_SEED)
            {
                let module = normalized_module_name(import);
                if module.is_empty() {
                    continue;
                }
                if let Some(path) = by_module.get(&module) {
                    scores
                        .entry(path.clone())
                        .and_modify(|score| *score = score.max(seed.search_score * 0.08 + 9.0))
                        .or_insert(seed.search_score * 0.08 + 9.0);
                }
                let segments = module
                    .split('/')
                    .filter(|part| part.len() >= 2)
                    .collect::<Vec<_>>();
                for (distance, segment) in segments.iter().rev().take(3).enumerate() {
                    let Some(paths) = by_stem.get(*segment) else {
                        continue;
                    };
                    let base = 7.0 - distance as f64 * 1.5 + seed.search_score * 0.05;
                    for path in paths.iter().take(4) {
                        scores
                            .entry(path.clone())
                            .and_modify(|score| *score = score.max(base))
                            .or_insert(base);
                    }
                }
            }
        }
        let mut ranked = scores.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|(left_path, left_score), (right_path, right_score)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left_path.cmp(right_path))
        });
        ranked.truncate(limit);
        Ok(ranked)
    }
}

#[cfg(test)]
mod optimized_tests {
    use super::*;
    use crate::core::McpToolInput;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn import_neighbor_can_enter_structure_without_lexical_query_match() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("sippion-map-expand-{nonce}"));
        std::fs::create_dir_all(&root).expect("root");
        std::fs::write(
            root.join("main.rs"),
            "mod dependency;\nuse crate::dependency::Helper;\nfn unique_entrypoint() { let _ = Helper; }\n",
        )
        .expect("main");
        std::fs::write(root.join("dependency.rs"), "pub struct Helper;\n").expect("dep");
        let repository = RepositoryAccess::open(&root).expect("repo");
        let query = McpToolInput {
            q: "unique_entrypoint".into(),
            ..Default::default()
        }
        .normalize()
        .expect("query");
        let search = repository.search(&query, 8, None).expect("search");
        assert!(search.hits.iter().any(|hit| hit.relative_path == "main.rs"));
        assert!(!search.hits.iter().any(|hit| hit.relative_path == "dependency.rs"));
        let mapped = repository
            .map_from_hits(&query, &search.hits, 8, None)
            .expect("map");
        assert!(
            mapped
                .entries
                .iter()
                .any(|entry| entry.relative_path == "dependency.rs")
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
