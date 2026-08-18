# RC30 package identity correction

- Bumped package version from `0.1.0-rc.29` to `0.1.0-rc.30`.
- Renamed the distribution root to `Sippion-v0.1-rc30-completeness-cache-hardening`.
- Updated the current README identity to RC30 while retaining RC29 change/validation documents as historical records.
- Added panic-safe in-flight cleanup for asynchronous `tools/call` workers. If a worker unwinds, its in-flight slot is released automatically; cleanup is identity-checked so a reused request id cannot cause a newer request to be removed.

RC30 exists to prevent two different payloads from being distributed under the same RC29 version identity after the post-RC29 hardening changes.

The original RC30 package did not include a completed release-validation result. Current verification
is reported separately from this historical change log.

## Post-package hardening fix

- Added an RAII in-flight guard around async workers so worker panic/unwind cannot permanently consume one of the four in-flight tool-call slots.
- Added regression tests for panic cleanup and request-id reuse safety.

## Additional concurrency / JSON-RPC hardening

- Async response writes no longer hold the in-flight request mutex while stdout may block. The
  registration remains active until the write finishes, so response-pending workers still count
  toward the four-call concurrency cap and request IDs cannot be reused prematurely.
- Duplicate/full-capacity error responses are emitted only after releasing the in-flight mutex.
- Async worker creation now uses `std::thread::Builder::spawn`; spawn failure releases the reserved
  in-flight slot and returns a JSON-RPC internal error instead of silently leaking capacity.
- Numeric JSON-RPC request IDs now accept finite fractional values for correlation/cancellation,
  while preserving typed string-vs-number tracking.
- Index and structural-analysis single-flight registrations now use RAII cleanup so panic/unwind
  cannot strand a path in the in-flight registry and make later requests wait until timeout.
- Added regression coverage for blocked stdout lock independence and panic-safe index/analysis
  single-flight cleanup.

## Post-review correctness hardening (2026-08-17)

- Unified `repo_context` retrieval and structural mapping under one shared 20-second wall-clock
  budget; structural analysis no longer starts a fresh 20-second timer after retrieval finishes.
- Removed provisional Top-N early termination during exact candidate verification. Sippion now
  verifies the bounded candidate set subject to the existing byte/time guards, applies final
  exact/structural/BM25 scoring, and only then truncates to the requested result count.
- Bound path-based discovery to the capability-opened project root identity. Root replacement or
  rename is rejected before/after discovery, path-only hits are capability-verified, and content
  hits must match the discovery-time source stamp before indexing or ranking.
- Added regression tests for shared wall-clock expiry, final Top-N reordering, and root-path
  replacement rejection.

## Post-review stale-evidence hardening (2026-08-18)

- Revalidate every search hit against the current source generation before its excerpt can be
  rendered into `repo_context`. Structural analysis remains capped independently, so lower-ranked
  hits are checked for identity/fingerprint consistency without being retained in the structural
  graph.
- Added `invalidated_evidence_paths` to structural-map output and filter those paths from final
  excerpts. Read failures, metadata-generation mismatches, and content-fingerprint mismatches now
  fail closed as incomplete context instead of allowing previously verified evidence to survive.
- This closes the Windows same-size/same-mtime adaptive-cache gap where an older verified excerpt
  could otherwise remain model-visible after a same-length rewrite within one top-level request.
- Added regression coverage for both an in-range fingerprint mismatch and a stale hit ranked beyond
  the structural-analysis limit.

## MCP / repository service decoupling (2026-08-18)

- Added an internal, transport-independent `RepositoryService` boundary between MCP dispatch and repository retrieval.
- Added `LocalRepositoryService` as the current in-process implementation; `sippion mcp --root ...` keeps the same public behavior and still requires no daemon.
- Moved adaptive `repo_context` execution, structural summary rendering, evidence packing, and repository-error translation out of the MCP transport layer into `src/service.rs`.
- MCP workers now share `Arc<dyn RepositoryService>` rather than `Arc<RepositoryAccess>`.
- Kept the current security/product boundary unchanged: local stdio MCP, project-scoped, read-only, no network, no persistent index, and no background daemon.
- Added service-boundary tests and a JSON-RPC-to-local-service regression test.
