# Security and trust boundary

Sippion is deliberately local and read-only:

- transport is local stdio MCP; there is no network client, listener, provider
  credential handling, model API proxy, daemon, or persistent repository index;
- each process is bound to one trusted project root and does not write the
  repository or create a repository-wide home-directory root;
- setup preserves unrelated client settings and does not create a secret store
  or automatic tool approval;
- repository reads refuse symlinks and reject multi-hard-linked files on Unix
  and Windows; root identity and source metadata are revalidated around reads;
- production retrieval does not execute repository code, shell commands, build
  scripts, procedural macros, an LSP, or a compiler frontend;
- file size, discovery, scan, AST, wall-clock, concurrency, result, and output
  limits keep retrieval bounded;
- high-confidence credentials and private-key material are redacted before
  model-visible excerpts and structural output. A redacted-only match is
  represented without exposing its value.

These controls reduce exposure but do not replace a dedicated secret scanner or
an OS sandbox. The project root remains a trusted root, and clients should bind
one Sippion process to the intended project rather than a broad parent folder.

The security boundary is intentionally unchanged by the release and document
organization work in this repository.
