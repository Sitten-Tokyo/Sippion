#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryAccessError {
    InvalidRelativePath,
    NonUtf8Path,
    DeniedPath,
    PrunedPath,
    NotRegularFile,
    NotFound,
    TooLarge,
    NonUtf8Source,
    HardLinkedFile,
    ConcurrentModification,
    Io,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedSource {
    pub text: String,
    /// Raw bytes actually read. Local scan budgets must use this value.
    pub source_bytes: usize,
    /// Stamp from the exact open file handle after the read completed.
    pub stamp: SourceStamp,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub relative_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub excerpt: String,
    pub score: f64,
    // Internal identity captured during exact verification. These fields are deliberately not
    // model-visible; structural mapping uses them to reject a file that changed after evidence
    // was collected. The content fingerprint also covers Windows same-size/same-mtime rewrites.
    source_stamp: Option<SourceStamp>,
    source_fingerprint: Option<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchCoverage {
    /// True only when metadata discovery completed within the entry/path/time guards.
    pub discovery_complete: bool,
    pub eligible_files: usize,
    pub indexed_files: usize,
    pub partial_index_files: usize,
    /// Policy exclusions that can hide searchable content (for example ignore-rule-hidden content,
    /// pruned/denied/binary paths, symlinks, >2 MiB files, stable non-UTF-8 sources, or hard-linked
    /// files). Directories with .gitignore/.ignore controls and pruned directory subtrees contribute
    /// conservative sentinels because the exact number of hidden files is intentionally not scanned.
    pub policy_excluded_files: usize,
    pub scanned_files: usize,
    pub scanned_bytes: usize,
    /// Cumulative adaptive scan allowance granted to this tool call.
    pub scan_budget_bytes: usize,
    /// Configured hard ceiling for adaptive expansion.
    pub scan_budget_cap_bytes: usize,
    /// Number of adaptive retrieval rounds used.
    pub adaptive_rounds: usize,
    /// 0..=1000 confidence score for deterministic model-visible reporting.
    pub confidence_milli: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SearchHit>,
    /// True when discovery, RAM-index coverage, verification, candidate generation, or source
    /// scanning did not cover the full eligible search space.
    pub truncated: bool,
    pub coverage: SearchCoverage,
    /// True only when granting more adaptive scan bytes can plausibly improve completeness.
    /// Candidate-cap truncation alone is incomplete but not scan-budget-expandable.
    adaptive_expandable: bool,
}

#[derive(Debug)]
pub(crate) struct OptimizedSearchOutcome {
    pub(crate) outcome: SearchOutcome,
    pub(crate) snapshots: HashMap<String, Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SourceStamp {
    len: u64,
    modified_nanos: Option<u128>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    ctime: i64,
    #[cfg(unix)]
    ctime_nsec: i64,
    #[cfg(unix)]
    nlink: u64,
    #[cfg(windows)]
    volume_serial_number: Option<u64>,
    #[cfg(windows)]
    file_index: Option<u64>,
    #[cfg(windows)]
    last_write_time: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RootIdentity {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(windows)]
    volume_serial_number: u64,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(all(not(unix), not(windows)))]
    created_nanos: Option<u128>,
}

#[derive(Debug, Clone)]
struct DiscoveredFile {
    path: String,
    stamp: Option<SourceStamp>,
}

struct DiscoveryOutcome {
    files: Vec<DiscoveredFile>,
    policy_excluded_files: usize,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct IndexedDocument {
    stamp: Option<SourceStamp>,
    document_len: usize,
    terms: Vec<(u64, u16)>,
    substring_grams: Vec<u32>,
    term_truncated: bool,
}

#[derive(Debug, Default)]
struct RamIndex {
    files: HashMap<String, IndexedDocument>,
    total_entries: usize,
    saturated: bool,
}

#[derive(Debug, Clone)]
struct PendingFile {
    file: DiscoveredFile,
    path_bonus: usize,
    changed: bool,
}

#[derive(Debug, Clone)]
struct RankedCandidate {
    relative_path: String,
    term_frequencies: Vec<usize>,
    document_len: usize,
    path_bonus: usize,
    has_content: bool,
    score: f64,
}

#[derive(Debug, Clone)]
enum VerifiedEvidence {
    None,
    Visible {
        start_line: u32,
        end_line: u32,
        excerpt: String,
        exact_bonus: usize,
        structure_bonus: f64,
    },
    Redacted,
}

#[derive(Debug, Clone)]
struct VerifiedCandidate {
    stamp: SourceStamp,
    fingerprint: (u64, u64),
    document_len: usize,
    term_frequencies: Vec<usize>,
    evidence: VerifiedEvidence,
    /// Optional request-local source snapshot for avoiding a second full read during structural
    /// mapping. This is never stored in RepositoryAccess and is capped per file.
    snapshot_source: Option<Arc<str>>,
}

#[derive(Debug, Default)]
struct ScanLaneOutcome {
    bytes: usize,
    files: usize,
    incomplete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexFlightClaim {
    Leader,
    AlreadyIndexed,
    TimedOut,
}

#[derive(Debug, Clone)]
struct SearchMemory {
    session_id: String,
    agent_id: Option<String>,
    terms: Vec<String>,
    paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct CachedRepoMapSymbol {
    name: String,
    kind: String,
    line: u32,
}

#[derive(Debug, Clone)]
struct CachedAnalysis {
    stamp: SourceStamp,
    // Deliberately structural only: never retain source-line signatures in the shared cache.
    symbols: Vec<CachedRepoMapSymbol>,
    semantics: SemanticFacts,
    cacheable: bool,
    last_used: u64,
}

#[derive(Debug, Default)]
struct AnalysisCacheState {
    entries: HashMap<String, CachedAnalysis>,
    inflight: HashSet<String>,
    tick: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GraphCacheNode {
    path: String,
    stamp: SourceStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct GraphCacheKey(Vec<GraphCacheNode>);

#[derive(Debug, Clone)]
struct CachedGraph {
    edge_maps: Vec<HashMap<usize, (f64, String)>>,
    centrality: Vec<f64>,
    last_used: u64,
}

#[derive(Debug, Default)]
struct GraphCacheState {
    entries: HashMap<GraphCacheKey, CachedGraph>,
    tick: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapLink {
    pub relative_path: String,
    pub kind: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoMapEntry {
    pub relative_path: String,
    pub score: f64,
    pub symbols: Vec<RepoMapSymbol>,
    pub links_to: Vec<String>,
    pub semantic_links: Vec<RepoMapLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepositoryMapOutcome {
    pub entries: Vec<RepoMapEntry>,
    pub truncated: bool,
    /// Search evidence that could not be confirmed against the current file generation while
    /// building this context. Callers must not render excerpts for these paths.
    pub invalidated_evidence_paths: Vec<String>,
}

#[derive(Debug, Clone)]
struct MapCandidate {
    relative_path: String,
    is_expansion: bool,
    stamp: SourceStamp,
    search_score: f64,
    source_lower: String,
    symbols: Vec<RepoMapSymbol>,
    definition_names: Vec<String>,
    semantics: SemanticFacts,
    analysis_cacheable: bool,
    semantic_query_bonus: f64,
}

/// One process owns one trusted project root. Model paths are resolved component-by-component
/// through capability directory handles, and symlinks are refused rather than followed.
pub struct RepositoryAccess {
    root_path: PathBuf,
    root_dir: Dir,
    // Stable identity of the capability-opened project root. Ambient path walking is permitted only
    // while the configured root path still resolves to this exact directory.
    root_identity: RootIdentity,
    max_scan_budget_bytes: usize,
    // Search index is deliberately RAM-only. It stores hashed lexical statistics and file stamps,
    // never source bodies, and disappears with the process.
    ram_index: Mutex<RamIndex>,
    // Windows cannot safely reuse metadata-only freshness across top-level searches at this MSRV.
    // Serialize those searches so one request cannot clear another request's in-progress index.
    #[cfg(windows)]
    windows_search_serial: Mutex<()>,
    // Structural source is re-read on Windows and analysis/graph cache keys include a content
    // fingerprint. Serialize top-level map construction so those verified caches can be reused
    // across requests without stale same-size/same-mtime replacements racing one another.
    #[cfg(windows)]
    windows_map_serial: Mutex<()>,
    // File-level single-flight prevents concurrent cold-start queries from reading/indexing the
    // same unchanged file multiple times.
    index_inflight: Mutex<HashSet<String>>,
    index_ready: Condvar,
    // Engram-inspired memory is deliberately volatile: paths/query terms plus bounded coordination
    // identifiers only, never source bodies or secrets, and never written to disk.
    session_memory: Mutex<VecDeque<SearchMemory>>,
    // Parsed structural facts are safe to share across subagents because the cache stores only
    // symbols/semantic facts keyed by a verified source stamp; source bodies are never retained.
    analysis_cache: Mutex<AnalysisCacheState>,
    analysis_ready: Condvar,
    graph_cache: Mutex<GraphCacheState>,
}

struct IndexFlightGuard<'a> {
    repository: &'a RepositoryAccess,
    path: String,
}

impl<'a> IndexFlightGuard<'a> {
    fn new(repository: &'a RepositoryAccess, path: String) -> Self {
        Self { repository, path }
    }
}

impl Drop for IndexFlightGuard<'_> {
    fn drop(&mut self) {
        self.repository.release_index_flight(&self.path);
    }
}

struct AnalysisFlightGuard<'a> {
    repository: &'a RepositoryAccess,
    path: String,
}

impl<'a> AnalysisFlightGuard<'a> {
    fn new(repository: &'a RepositoryAccess, path: String) -> Self {
        Self { repository, path }
    }
}

impl Drop for AnalysisFlightGuard<'_> {
    fn drop(&mut self) {
        let mut state = match self.repository.analysis_cache.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.inflight.remove(&self.path);
        drop(state);
        self.repository.analysis_ready.notify_all();
    }
}
