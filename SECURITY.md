# Security Policy

Sippion treats repository contents as untrusted data and is designed to remain local, read-only, no-network, and project-scoped while serving repository context. The full trust model, filesystem policy, redaction behavior, installer provenance model, and known boundaries are documented in [`docs/security.md`](docs/security.md).

## Supported versions

Before 1.0, security fixes are supported on the latest published release candidate and on `main`. Older release candidates may not receive backports. After a fix is released, users should upgrade to the newest release rather than relying on an older RC.

## Reporting a vulnerability

Please do **not** disclose exploit details, credentials, private repository contents, or unredacted secrets in a public issue or discussion.

1. Use GitHub's private vulnerability-reporting flow for this repository when it is available from the repository **Security** tab.
2. If that private flow is unavailable, open a minimal public issue asking the maintainer to establish a private contact channel. Do not include vulnerability details in that issue.
3. Include the affected Sippion version/commit, platform, impact, minimal reproduction information, and whether the issue crosses a documented trust boundary.

Reports involving installer/release integrity should include the release tag, asset name, expected/observed SHA-256 digest, and attestation-verification result when available.

## Scope priorities

High-priority reports include unintended writes, repository-code execution during retrieval, network access while serving context, root/symlink/hard-link boundary escapes, secret-redaction bypasses with realistic credential material, release provenance/checksum bypasses, and denial-of-service inputs that escape documented resource bounds.

A parser or semantic ranking inaccuracy alone is generally a correctness issue rather than a security issue unless it creates a trust-boundary bypass or exposes data outside the authorized repository scope.
