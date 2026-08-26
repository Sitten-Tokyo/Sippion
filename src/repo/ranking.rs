use super::*;

pub(super) fn upsert_repo_edge(
    edges: &mut [HashMap<usize, (f64, String)>],
    from: usize,
    to: usize,
    weight: f64,
    kind: &str,
) {
    if from == to
        || from >= edges.len()
        || to >= edges.len()
        || !weight.is_finite()
        || weight <= 0.0
    {
        return;
    }
    let slot = edges[from]
        .entry(to)
        .or_insert_with(|| (weight, kind.to_string()));
    if weight > slot.0 {
        *slot = (weight, kind.to_string());
    }
}

pub(super) fn search_confidence(query: &NormalizedQuery, outcome: &SearchOutcome) -> f64 {
    if outcome.hits.is_empty() {
        // A policy-excluded file is intentionally not adaptive-scan-expandable, but it still
        // prevents a repository-wide absence claim because its contents were never inspected.
        return if outcome.truncated {
            0.05
        } else if outcome.coverage.policy_excluded_files > 0 {
            0.35
        } else {
            0.98
        };
    }

    let top = outcome.hits.iter().take(3).collect::<Vec<_>>();
    let mut covered = HashSet::<&str>::new();
    for hit in &top {
        let path = crate::core::unicode_search_fold(&hit.relative_path);
        let excerpt = crate::core::unicode_search_fold(&hit.excerpt);
        for term in &query.terms {
            if path.contains(term.as_str()) || excerpt.contains(term.as_str()) {
                covered.insert(term.as_str());
            }
        }
    }
    let query_coverage = covered.len() as f64 / query.terms.len().max(1) as f64;
    let top_score = top.first().map_or(0.0, |hit| hit.score.max(0.0));
    let second_score = top.get(1).map_or(0.0, |hit| hit.score.max(0.0));
    let margin = if top_score <= f64::EPSILON {
        0.0
    } else {
        ((top_score - second_score).max(0.0) / top_score).min(1.0)
    };
    let evidence_depth = (outcome.hits.len().min(6) as f64 / 6.0).min(1.0);
    let index_coverage = if outcome.coverage.eligible_files == 0 {
        1.0
    } else {
        outcome.coverage.indexed_files as f64 / outcome.coverage.eligible_files as f64
    };
    let completion_bonus = if outcome.truncated || outcome.coverage.policy_excluded_files > 0 {
        0.0
    } else {
        0.08
    };
    (query_coverage * 0.48
        + margin * 0.20
        + evidence_depth * 0.12
        + index_coverage * 0.20
        + completion_bonus)
        .clamp(0.0, 1.0)
}

pub(super) fn stable_term_hash(text: &str) -> u64 {
    // Stable FNV-1a keeps the RAM index compact and avoids retaining repository tokens verbatim.
    let folded = crate::core::unicode_search_fold(text);
    let mut hash = 0xcbf29ce484222325u64;
    for byte in folded.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(super) fn identifier_fragments(identifier: &str) -> Vec<String> {
    let mut fragments = Vec::new();
    for coarse in identifier.split(['_', '-']) {
        if coarse.len() < 2 {
            continue;
        }
        let chars = coarse.chars().collect::<Vec<_>>();
        if chars.is_empty() {
            continue;
        }
        let mut start = 0usize;
        for index in 1..chars.len() {
            let previous = chars[index - 1];
            let current = chars[index];
            let next = chars.get(index + 1).copied();
            let camel_boundary = current.is_uppercase()
                && (previous.is_lowercase()
                    || previous.is_ascii_digit()
                    || (previous.is_uppercase() && next.is_some_and(char::is_lowercase)));
            if camel_boundary {
                let fragment = chars[start..index].iter().collect::<String>();
                if fragment.len() >= 2 {
                    fragments.push(crate::core::unicode_search_fold(&fragment));
                }
                start = index;
            }
        }
        let fragment = chars[start..].iter().collect::<String>();
        if fragment.len() >= 2 {
            fragments.push(crate::core::unicode_search_fold(&fragment));
        }
    }
    fragments
}

pub(super) fn substring_gram_key(window: &[u8]) -> u32 {
    let mut key = (window.len() as u32) << 24;
    for (index, byte) in window.iter().enumerate() {
        let shift = 16usize.saturating_sub(index * 8);
        key |= u32::from(byte.to_ascii_lowercase()) << shift;
    }
    key
}

pub(super) fn unicode_scalar_gram_key(ch: char) -> u32 {
    // ASCII substring keys use top-byte namespaces 2 and 3. Reserve the high bit for Unicode
    // scalar sketches so the key families cannot collide structurally.
    let mut hash = 0x811c9dc5u32;
    let mut encoded = [0u8; 4];
    for byte in ch.encode_utf8(&mut encoded).as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    0x8000_0000 | (hash & 0x7fff_ffff)
}

pub(super) fn query_substring_grams(term: &str) -> Vec<u32> {
    if term.is_ascii() {
        if term.len() < 2 {
            return Vec::new();
        }
        let bytes = term.as_bytes();
        let width = if bytes.len() == 2 { 2 } else { 3 };
        let mut grams = bytes
            .windows(width)
            .map(substring_gram_key)
            .collect::<Vec<_>>();
        grams.sort_unstable();
        grams.dedup();
        return grams;
    }

    // Query normalization already applies Unicode-aware lowercase. Requiring every folded scalar
    // preserves candidate recall for Unicode substrings; exact source verification still removes
    // hash/order false positives before any evidence becomes model-visible.
    let mut grams = term
        .chars()
        .map(unicode_scalar_gram_key)
        .collect::<Vec<_>>();
    grams.sort_unstable();
    grams.dedup();
    grams
}

pub(super) fn add_index_term(
    counts: &mut HashMap<u64, u16>,
    term: &str,
    term_truncated: &mut bool,
) {
    if term.len() < 2 {
        return;
    }
    let hash = stable_term_hash(term);
    if let Some(value) = counts.get_mut(&hash) {
        *value = value.saturating_add(1);
    } else if counts.len() < MAX_INDEX_UNIQUE_TERMS_PER_FILE {
        counts.insert(hash, 1);
    } else {
        *term_truncated = true;
    }
}

pub(super) fn build_indexed_document(text: &str, stamp: Option<SourceStamp>) -> IndexedDocument {
    let mut counts = HashMap::<u64, u16>::new();
    let mut substring_grams = HashSet::<u32>::new();
    let mut document_len = 0usize;
    let mut term_truncated = false;

    for part in text
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|part| part.len() >= 2)
    {
        document_len = document_len.saturating_add(1);
        let lower = crate::core::unicode_search_fold(part);
        add_index_term(&mut counts, &lower, &mut term_truncated);
        for fragment in identifier_fragments(part) {
            add_index_term(&mut counts, &fragment, &mut term_truncated);
        }

        // Candidate sketches never retain source bodies or plaintext tokens. ASCII keeps compact
        // two/three-byte grams. Tokens containing Unicode additionally get hashed scalar sketches,
        // so queries such as "認証" can nominate "ユーザー認証処理" for exact source verification.
        // ASCII runs inside mixed identifiers also keep ordinary substring recall.
        for ascii_run in lower.split(|ch: char| !ch.is_ascii()) {
            let bytes = ascii_run.as_bytes();
            for width in [2usize, 3usize] {
                if bytes.len() < width {
                    continue;
                }
                for window in bytes.windows(width) {
                    if substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {
                        term_truncated = true;
                        break;
                    }
                    substring_grams.insert(substring_gram_key(window));
                }
            }
        }
        if !lower.is_ascii() {
            for ch in lower.chars() {
                if substring_grams.len() >= MAX_INDEX_SUBSTRING_GRAMS_PER_FILE {
                    term_truncated = true;
                    break;
                }
                substring_grams.insert(unicode_scalar_gram_key(ch));
            }
        }
    }

    let mut terms = counts.into_iter().collect::<Vec<_>>();
    terms.sort_unstable_by_key(|(hash, _)| *hash);
    let mut substring_grams = substring_grams.into_iter().collect::<Vec<_>>();
    substring_grams.sort_unstable();
    IndexedDocument {
        stamp,
        document_len: document_len.max(1),
        terms,
        substring_grams,
        term_truncated,
    }
}

pub(super) fn indexed_query_frequencies(
    document: &IndexedDocument,
    query_terms: &[(u64, Vec<u32>)],
) -> Vec<usize> {
    query_terms
        .iter()
        .map(|(hash, grams)| {
            let exact = document
                .terms
                .binary_search_by_key(hash, |(term_hash, _)| *term_hash)
                .ok()
                .and_then(|index| document.terms.get(index))
                .map(|(_, count)| usize::from(*count))
                .unwrap_or(0);
            if exact > 0 {
                return exact;
            }
            if !grams.is_empty()
                && grams
                    .iter()
                    .all(|gram| document.substring_grams.binary_search(gram).is_ok())
            {
                // This is deliberately a one-hit fallback, matching the pre-index substring
                // behavior. False positives are removed by bounded source verification.
                1
            } else {
                0
            }
        })
        .collect()
}

pub(super) fn stratified_pending_lanes(
    pending: Vec<PendingFile>,
) -> (Vec<PendingFile>, Vec<PendingFile>, Vec<PendingFile>) {
    let mut priority = Vec::new();
    let mut ordinary = Vec::new();
    for file in pending {
        if file.changed || file.path_bonus > 0 {
            priority.push(file);
        } else {
            ordinary.push(file);
        }
    }
    priority.sort_by(|a, b| {
        b.changed
            .cmp(&a.changed)
            .then_with(|| b.path_bonus.cmp(&a.path_bonus))
            .then_with(|| a.file.path.cmp(&b.file.path))
    });

    // Roughly one eighth of ordinary files form a deterministic cross-repository sample. Files
    // already consumed by this lane are skipped when the round-robin broad lane is reached.
    let mut sample = ordinary
        .iter()
        .filter(|file| stable_term_hash(&file.file.path) % 8 == 0)
        .cloned()
        .collect::<Vec<_>>();
    sample.sort_by_key(|file| stable_term_hash(&file.file.path));

    let mut buckets = BTreeMap::<String, VecDeque<PendingFile>>::new();
    ordinary.sort_by(|a, b| a.file.path.cmp(&b.file.path));
    for file in ordinary {
        let bucket = file.file.path.split('/').next().unwrap_or("").to_string();
        buckets.entry(bucket).or_default().push_back(file);
    }
    let mut broad = Vec::new();
    loop {
        let mut progressed = false;
        for bucket in buckets.values_mut() {
            if let Some(file) = bucket.pop_front() {
                broad.push(file);
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    (priority, sample, broad)
}

pub(super) fn is_cancelled(cancellation: Option<&AtomicBool>) -> bool {
    cancellation.is_some_and(|flag| flag.load(AtomicOrdering::Relaxed))
}

pub(super) fn search_timed_out(started: &Instant) -> bool {
    started.elapsed() >= MAX_SEARCH_WALL_TIME
}

pub(super) fn first_term_match_byte(line: &str, terms: &[String]) -> Option<usize> {
    terms
        .iter()
        .filter_map(|term| crate::core::unicode_search_fold_find_byte(line, term))
        .min()
}

pub(super) fn bounded_search_excerpt(
    lines: &[&str],
    focus: usize,
    focus_match_byte: usize,
) -> (String, usize, usize) {
    let half = DEFAULT_CONTEXT_LINES / 2;
    let mut start = focus.saturating_sub(half);
    let mut end = (focus + half + 1).min(lines.len());

    loop {
        let joined = lines[start..end].join("\n");
        if joined.len() <= MAX_SEARCH_EXCERPT_BYTES {
            return (joined, start, end);
        }

        // Remove the larger outer neighbor first while always retaining the matched line. This
        // prevents a huge adjacent line from consuming the excerpt and hiding the actual match.
        if start < focus || end > focus + 1 {
            let left_bytes = if start < focus {
                lines[start].len().saturating_add(1)
            } else {
                0
            };
            let right_bytes = if end > focus + 1 {
                lines[end - 1].len().saturating_add(1)
            } else {
                0
            };
            if start < focus && (end <= focus + 1 || left_bytes >= right_bytes) {
                start += 1;
            } else if end > focus + 1 {
                end -= 1;
            }
            continue;
        }

        return (
            bounded_focus_line(lines[focus], focus_match_byte),
            focus,
            focus + 1,
        );
    }
}

pub(super) fn bounded_focus_line(line: &str, match_byte: usize) -> String {
    const PREFIX: &str = "[SIPPION_EXCERPT_TRUNCATED] ";
    const SUFFIX: &str = " [SIPPION_EXCERPT_TRUNCATED]";
    if line.len() <= MAX_SEARCH_EXCERPT_BYTES {
        return line.to_string();
    }

    let payload_budget = MAX_SEARCH_EXCERPT_BYTES
        .saturating_sub(PREFIX.len())
        .saturating_sub(SUFFIX.len());
    let mut start = match_byte.saturating_sub(payload_budget / 2);
    while start < line.len() && !line.is_char_boundary(start) {
        start += 1;
    }
    let mut end = start.saturating_add(payload_budget).min(line.len());
    while end > start && !line.is_char_boundary(end) {
        end -= 1;
    }

    // If clamping at EOF left unused budget, shift left without ever exceeding the byte budget.
    if end == line.len() && end.saturating_sub(start) < payload_budget {
        start = end.saturating_sub(payload_budget);
        while start < end && !line.is_char_boundary(start) {
            start += 1;
        }
    }

    let mut bounded = String::with_capacity(MAX_SEARCH_EXCERPT_BYTES);
    if start > 0 {
        bounded.push_str(PREFIX);
    }
    bounded.push_str(&line[start..end]);
    if end < line.len() {
        bounded.push_str(SUFFIX);
    }
    debug_assert!(bounded.len() <= MAX_SEARCH_EXCERPT_BYTES);
    bounded
}

pub(super) fn source_content_fingerprint(text: &str) -> (u64, u64) {
    // Two independently seeded FNV-1a lanes provide a compact, allocation-free content identity.
    // This is a consistency guard, not an authentication primitive.
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut left = 0xcbf2_9ce4_8422_2325u64;
    let mut right = 0x8422_2325_cbf2_9ce4u64;
    for &byte in text.as_bytes() {
        left ^= u64::from(byte);
        left = left.wrapping_mul(FNV_PRIME);
        right ^= u64::from(byte).wrapping_add(0x9d);
        right = right.wrapping_mul(FNV_PRIME);
    }
    (left, right)
}

pub(super) fn sort_hits(hits: &mut [SearchHit]) {
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.relative_path.cmp(&b.relative_path))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
}

pub(super) fn path_match_score(path: &str, terms: &[String]) -> usize {
    let path_lower = crate::core::unicode_search_fold(path);
    terms
        .iter()
        .filter(|term| path_lower.contains(term.as_str()))
        .count()
}

pub(super) fn hit_is_better(candidate: &SearchHit, current: &SearchHit) -> bool {
    candidate
        .score
        .partial_cmp(&current.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| current.start_line.cmp(&candidate.start_line))
        == Ordering::Greater
}

#[cfg(test)]
pub(super) fn prune_candidates_if_needed(hits: &mut Vec<SearchHit>, candidate_limit: usize) {
    if hits.len() < candidate_limit.saturating_mul(2) {
        return;
    }
    sort_hits(hits);
    if hits.len() > candidate_limit {
        hits.truncate(candidate_limit);
    }
}

pub(super) fn path_parts(path: &Path) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    for component in path.components() {
        if let Component::Normal(part) = component {
            // This feeds filesystem safety policy, which is intentionally ASCII case-insensitive.
            parts.push(part.to_str()?.to_ascii_lowercase());
        }
    }
    Some(parts)
}

pub(super) fn read_failure_makes_scan_incomplete(error: &RepositoryAccessError) -> bool {
    matches!(
        error,
        RepositoryAccessError::InvalidRelativePath
            | RepositoryAccessError::NonUtf8Path
            | RepositoryAccessError::NotRegularFile
            | RepositoryAccessError::NotFound
            | RepositoryAccessError::TooLarge
            | RepositoryAccessError::HardLinkedFile
            | RepositoryAccessError::ConcurrentModification
            | RepositoryAccessError::Io
    )
}
