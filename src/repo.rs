use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use aho_corasick::AhoCorasick;
#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use ignore::WalkBuilder;

use crate::core::{CoordinationContext, MAX_QUERY_TERMS, NormalizedQuery};
use crate::hybrid::{
    bm25_score, extract_symbols, structural_line_bonus, term_statistics, weighted_pagerank,
};
use crate::syntax::{
    SemanticFacts, extract_ast_symbols_bounded, extract_semantic_facts_bounded,
    supports_tree_sitter_path,
};

pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DISCOVERED_FILES: usize = 50_000;
pub const MAX_DISCOVERED_ENTRIES: usize = 100_000;
pub const MAX_DISCOVERED_PATH_BYTES: usize = 16 * 1024 * 1024;
/// Default hard ceiling for adaptive retrieval. A normal call starts at 32 MiB and expands only
/// when bounded evidence remains incomplete and confidence is low.
pub const MAX_SCAN_BYTES: usize = 512 * 1024 * 1024;
pub const ADAPTIVE_INITIAL_SCAN_BYTES: usize = 32 * 1024 * 1024;
pub const MIN_CONFIGURED_SCAN_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CONFIGURED_SCAN_BYTES: usize = 512 * 1024 * 1024;
const MAX_INDEX_UNIQUE_TERMS_PER_FILE: usize = 32_768;
const MAX_INDEX_SUBSTRING_GRAMS_PER_FILE: usize = 16_384;
const MAX_INDEX_TOTAL_ENTRIES: usize = 4_000_000;
pub const MAX_SEARCH_RESULTS: usize = 32;
pub const MAX_SEARCH_CANDIDATES: usize = 512;
pub const DEFAULT_CONTEXT_LINES: usize = 8;
pub const MAX_SEARCH_EXCERPT_BYTES: usize = 2 * 1024;
pub const MAX_SEARCH_WALL_TIME: Duration = Duration::from_secs(20);
const ADAPTIVE_CONFIDENCE_STOP: f64 = 0.78;
const MAX_ANALYSIS_CACHE_FILES: usize = 256;
const MAX_GRAPH_CACHE_ENTRIES: usize = 64;
const MAX_SESSION_MEMORY_RECORDS: usize = 128;
// Structural-map redaction must not amplify a bounded source file into a much larger retained
// buffer. Bounded redaction also refuses to run the allocating per-line redactors over an
// attacker-controlled giant/minified line; that line is suppressed instead.
const MAX_REPOSITORY_MAP_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_BOUNDED_REDACTION_LINE_BYTES: usize = 64 * 1024;
const REDACTED_OVERSIZE_LINE: &str = "[SIPPION_REDACTED_OVERSIZE_LINE]";
const REDACTED_MATCH_EXCERPT: &str = "[SIPPION_REDACTED_MATCH: matching source content suppressed]";

pub const BUILTIN_PRUNED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    "out",
    "vendor",
    ".terraform",
    ".gradle",
    ".dart_tool",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".turbo",
    ".parcel-cache",
    ".nuxt",
    ".svelte-kit",
    ".build",
    "pods",
    "deriveddata",
];

const BUILTIN_PRUNED_FILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "poetry.lock",
    "pdm.lock",
    "uv.lock",
    "pipfile.lock",
    "composer.lock",
    "gemfile.lock",
    "podfile.lock",
    "pubspec.lock",
];

const CONTENT_MATCH_BASE_SCORE: usize = MAX_QUERY_TERMS * 3 + 1;

const OBVIOUS_BINARY_EXTENSIONS: &[&str] = &[
    "7z", "a", "avif", "bin", "bmp", "bz2", "class", "db", "dll", "dylib", "eot", "exe", "flac",
    "gif", "gz", "heic", "heif", "ico", "jar", "jpeg", "jpg", "m4a", "mkv", "mov", "mp3", "mp4",
    "o", "obj", "ogg", "otf", "parquet", "pdf", "pdb", "pkl", "png", "pyc", "rar", "so", "sqlite",
    "sqlite3", "tar", "tgz", "ttf", "wav", "wasm", "webm", "webp", "woff", "woff2", "xz", "zip",
];

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
    // The same metadata limitation applies to the structural analysis and graph caches. Serialize
    // top-level map construction before discarding those cross-request caches on Windows so a
    // same-size/same-mtime replacement can never reuse stale structural facts.
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

mod access;
mod coordination;
mod map;
mod map_helpers;
mod policy;
mod ranking;
mod redaction;
mod search;

use map_helpers::*;
use policy::*;
use ranking::*;
use redaction::*;

const SENSITIVE_LITERAL_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "secret_key",
    "secretkey",
    "secret_key_base",
    "secretkeybase",
    "signing_key",
    "signingkey",
    "encryption_key",
    "encryptionkey",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "credential",
    "credentials",
    "secret_access_key",
    "awssecretaccesskey",
    "clientsecret",
    "accesstoken",
    "refreshtoken",
    "authtoken",
    "dbpassword",
    "databasepassword",
    "connection_string",
    "connectionstring",
    "database_url",
    "databaseurl",
    "authorization",
    "proxy_authorization",
    "proxyauthorization",
    "cookie",
    "set_cookie",
    "setcookie",
    "session_id",
    "sessionid",
    "session_token",
    "sessiontoken",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct RedactionOutcome {
    text: String,
    truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSensitiveValue {
    indent: usize,
    allow_same_indent: bool,
}

#[cfg(test)]
mod tests;
