# 2026-08-16 post-RC29 hardening patch (historical record)

This patch contains the earlier follow-up fixes **2** and **3**, plus the later fixes **1**, **2**, and **4** requested after deeper review.

## Credential redaction hardening

`src/repo.rs` applies defense-in-depth redaction before model-visible excerpts and redacted structural analysis are produced.

The earlier patch added:

- `Bearer <credential>` and `Basic <credential>` authentication-scheme detection;
- JWT/JWE-shaped token redaction;
- `Cookie:` and `Set-Cookie:` value redaction;
- sensitive literal keys including authorization, proxy authorization, cookie, session ID, and session token variants;
- URL userinfo credential redaction such as `postgres://user:password@host/...`.

The follow-up patch additionally closes two gaps:

- **Multiline sensitive values:** a sensitive JSON/YAML/JS-style key whose value begins on a later significant line is tracked and its scalar value is redacted. This covers quoted and unquoted YAML scalars, pretty-printed JSON, same-indent JSON/JS formatting, comments/blank lines between key and value, and YAML `|` / `>` block scalars while preserving source line count. Obvious computed references such as `${PASSWORD_FROM_ENV}` and nested mapping shapes such as `type: string` are preserved.
- **Short explicit Authorization credentials:** `Authorization:` and `Proxy-Authorization:` headers using `Bearer` or `Basic` now redact any non-empty credential-shaped value regardless of length. This closes valid short examples such as `Basic YTpi` and one-character bearer values. Explicit placeholders beginning with `{`, `<`, or `$` remain visible; a bare word in an actual Authorization header is treated as a credential and redacted. Generic prose such as `use Bearer token in documentation` remains subject to the older conservative length threshold.

Regression tests cover multiline YAML/JSON, same-indent JSON, YAML block scalars (including `secret: |` on the declaration line), comments, computed/nested non-secret forms, short Authorization credentials, placeholder preservation, and private-key blocks nested under a sensitive key. Existing `[SIPPION_REDACTED_*]` markers are also exempted from the literal-assignment pass so redaction stages cannot corrupt one another's output.

## Non-UTF-8 / non-Unicode repository path hardening

Lossy path conversion is no longer used for repository policy normalization.

- `normalize_relative()` requires every normal path component to be representable as UTF-8 and returns `RepositoryAccessError::NonUtf8Path` otherwise.
- `path_parts()` no longer calls `to_string_lossy()`; it returns `None` for non-representable components.
- pruning/denial checks do not invent a replacement-character path. Discovery proceeds until strict normalization can reject the affected file and mark discovery incomplete rather than silently aliasing distinct names.
- `read_failure_makes_scan_incomplete()` and the MCP-facing error text recognize the new error variant.
- Unix and Windows path-normalization regression tests are included, plus a Unix discovery test proving a non-UTF-8 filename is skipped with incomplete coverage instead of being collapsed to a lossy display name.

The remaining `to_string_lossy()` calls are outside this vulnerable normalization path: one operates on a `String`-derived repository path during import-ranking normalization, and one is only for displaying an unknown CLI argument.

## Windows RAM-index freshness hardening

At the configured Rust MSRV, Sippion does not rely on Windows size + modification time as a cross-request content identity.

- Top-level searches on Windows are serialized with a dedicated mutex so one request cannot clear another request's in-progress index.
- The RAM lexical index is discarded once at the beginning of every top-level Windows search.
- Adaptive rounds inside that same search continue sharing the rebuilt index, preserving the bounded 32 -> 64 -> 128 -> 256 -> 512 MiB completeness-growth behavior.
- A Windows-only regression test seeds a deliberately stale RAM document using the current file's exact `SourceStamp`, then verifies that a search for the current file contents still succeeds. This directly exercises the same-stamp stale-index case.

This intentionally trades some cross-request Windows cache performance for fail-closed search correctness. Unix behavior is unchanged.

## Validation status

The packaging environment still has no `cargo`, `rustc`, or `rustfmt`, so compilation, unit tests, Clippy, rustfmt, and MCP conformance were **not** executed here. This patch therefore does not change the existing release-gate warning.

Source-level checks performed here:

- reviewed all new redaction state transitions and explicit Authorization-header paths;
- reviewed Windows index-reset locking so concurrent top-level Windows searches cannot clear each other's active index;
- checked Rust delimiter/string/comment balance with a local lexical scanner;
- regenerated and verified `SHA256SUMS` after the patch.

## 2026-08-17 Unicode recall / MCP statelessness / cancellation follow-up

A second 2026-08-17 review found and patched three release-blocking correctness issues:

- **Unicode substring candidate recall:** the RAM candidate index now stores hashed Unicode-scalar sketches for tokens containing non-ASCII text. Queries such as `認証` can therefore nominate a source token such as `ユーザー認証処理` for exact source verification instead of being filtered out before verification. ASCII two/three-byte grams remain in use, including for ASCII runs inside mixed Unicode identifiers. The sketches are only candidate data; exact source verification remains authoritative and model-visible source text is still subject to secret redaction.
- **Modern MCP requests remain stateless:** the process-wide `Undecided/Legacy/Modern` protocol-mode binding was removed. The server now retains only a legacy-initialize boolean for backward compatibility. A modern 2026-07-28 request is validated solely from that request's `_meta` metadata and is not rejected because an earlier legacy request used the same stdio process. Legacy requests without modern `_meta` still require a successful legacy `initialize`.
- **Cancellation/response serialization:** async tool calls remain registered in the in-flight map through the final cancellation check and response commit. Cancellation and response completion now serialize on the same in-flight registry lock, preventing the old window where the entry was removed before stdout was written and a cancellation could therefore fail to suppress the response.

Regression tests cover Unicode RAM-index recall, end-to-end Unicode substring search, ASCII recall inside a mixed Unicode identifier, modern requests before/after legacy initialization, and suppression of an already-cancelled async response.
