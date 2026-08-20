# Security and trust boundary

Sippion is deliberately local and read-only:

- transport is local stdio MCP; there is no network client, listener, provider
  credential handling, model API proxy, daemon, or persistent repository index;
- each process is bound to one intentionally selected project root and does not
  write the repository or create a repository-wide home-directory root;
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

The filesystem root and repository content have different trust properties.
Users must intentionally select the project root because Sippion is allowed to
read eligible files inside it. The text inside that root does **not** become
trusted instructions merely because it was retrieved. Source comments, strings,
documentation, generated files, and other repository text may contain prompt
injection or social-engineering content. Clients and agents should treat
`repo_context` output strictly as repository data and must not follow tool-use,
network, credential, secret-disclosure, policy-override, or similar directions
found inside retrieved source text.

These controls reduce exposure but do not replace a dedicated secret scanner,
an OS sandbox, or the client's own tool-authorization policy. Bind one Sippion
process to the intended project rather than a broad parent folder, and avoid
combining untrusted repository analysis with automatic approval of write,
network, credential, or shell-capable tools.

## Client-configuration mutation safety

`sippion setup` and `sippion uninstall` modify only their documented per-user
client configuration and rule files. Text files managed with Sippion BEGIN/END
markers are treated conservatively: exactly one ordered marker pair is required
before an existing managed block can be replaced or removed. Missing halves,
duplicate markers, or reversed markers cause the operation to fail closed.

Setup and uninstall are transactional across all six managed client config/rule
files. Before mutation Sippion snapshots every target and its existing sibling
`.sippion-backup`. If any client operation fails, the full snapshot is restored,
including prior backup contents; files created by the failed transaction are
removed. A rollback failure is surfaced explicitly rather than being reported as
successful setup/uninstall.

Mutation refuses symlinked/non-regular target files, symlinked backup targets,
and symlinked parent directories. Before replacing an existing configuration
file, Sippion refreshes a sibling `.sippion-backup` copy from the immediate
pre-change contents. On Windows, where `rename` cannot replace an existing
destination portably, Sippion first renames the original to a unique rollback
path, installs the new file, and restores the original if installation fails.
If even restoration fails, the error reports the rollback path that still
contains the original bytes.

`sippion doctor` is intended for automation as well as humans: missing,
mismatched, malformed, or unreadable managed configuration makes the command
exit non-zero.

## Release and dependency supply chain

CI and release workflows pin third-party GitHub Actions to full commit SHAs.
CI audits `Cargo.lock` against the RustSec advisory database. Release binaries
are built from an explicitly selected tag resolved to an immutable commit,
checksummed, and given GitHub artifact attestations. Release installer scripts
are also published as checksummed, attested assets.

Release provenance is bound more narrowly than repository identity alone:
consumer verification requires the expected signer workflow and the exact
commit SHA resolved from the selected release tag. Reusable PR smoke builds do
not issue release-like attestations. Manual release dispatch is accepted only
when the workflow itself runs from the same tag commit being released, so the
attestation source commit cannot diverge from the bytes that were built.

### Default one-command installation

The README's bootstrap URL is pinned to an exact Sippion Git commit rather than
`main`, so the bootstrap script referenced by the documentation cannot change
when the branch moves. The bootstrap then:

1. queries GitHub Releases and explicitly selects the newest non-draft,
   published Sippion release, including prereleases;
2. resolves that release tag to its exact Git commit SHA and pins subsequent
   provenance checks to that SHA;
3. downloads the release installer and its `.sha256` file and verifies the
   installer SHA-256;
4. **before executing the installer**, verifies the installer's GitHub artifact
   attestation against `Sitten-Tokyo/Sippion/.github/workflows/release-draft.yml`
   and the release-tag commit SHA;
5. runs the authenticated installer with binary artifact-attestation
   verification explicitly required; and
6. the installer downloads the platform binary and matching `.sha256` file,
   verifies the binary SHA-256, verifies provenance against
   `Sitten-Tokyo/Sippion/.github/workflows/release-build.yml` and the same
   release-tag commit SHA, then installs it and runs `sippion setup`.

The default bootstrap therefore requires a GitHub CLI with `gh attestation`
support and working GitHub authentication. If provenance verification cannot be
performed or fails, installation stops. Checksums remain useful for corruption
and mismatch detection, but they are not treated as an independent authenticity
mechanism when both the artifact and checksum come from the same release.

The direct installers also make binary replacement transactional with setup: an
existing Sippion binary is retained until the new binary completes `sippion
setup`. If setup fails, the previous binary is restored; for a first install,
the failed new binary is removed. Symlink/reparse-point install destinations are
rejected.

### Checksum-only explicit opt-out

Direct use of `scripts/install.sh` or `scripts/install.ps1` also requires
artifact-attestation verification by default and fails closed unless a GitHub
CLI with `gh attestation` support is available and authenticated.

`SIPPION_REQUIRE_ATTESTATION=0` is an explicit opt-out for controlled
environments where artifact provenance has already been verified through
another trusted mechanism. In that mode the installer still verifies the
published SHA-256 but emits a warning because release-local checksums alone do
not provide independent authenticity if the release itself is compromised.
The primary README bootstrap does not use this opt-out.

`SIPPION_RELEASE_TAG` pins binary downloads to one release and
`SIPPION_RELEASE_SHA` can additionally pin the expected tag commit. An explicit
`SIPPION_RELEASE_BASE_URL` / `ReleaseBaseUrl` remains available for controlled
HTTPS mirrors and forks; when attestation verification is enabled, a custom base
URL must still be paired with a release tag whose source commit can be verified.
