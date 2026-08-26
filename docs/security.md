# Security and trust boundary

Sippion is deliberately local and read-only:

- transport is local stdio MCP; there is no network client, listener, provider
  credential handling, model API proxy, daemon, or persistent repository index;
- each process is bound to one intentionally selected project root and does not
  write the repository or create a repository-wide home-directory root;
- setup-generated registrations use fail-closed automatic project-root
  discovery instead of trusting a client-supplied `cwd` as the final boundary;
- setup preserves unrelated client settings and does not create a secret store,
  persistent configuration backup, or automatic tool approval;
- repository reads refuse symlinks and reject multi-hard-linked files on Unix
  and Windows; root identity and source metadata are revalidated around reads;
- sensitive credential/config and pruned dependency/build paths use ASCII
  case-insensitive policy matching so case-insensitive filesystems cannot bypass
  them with alternate casing;
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

## Project-root selection

Setup-generated Codex, Claude Code, and Antigravity registrations launch
`sippion mcp --root-auto`. Automatic discovery starts at the process current
directory and selects the nearest recognized project boundary: a Git worktree
marker or a supported project manifest such as `Cargo.toml`, `package.json`,
`pyproject.toml`, or `go.mod`. It does not continue past a nearer project
manifest merely to prefer a farther ancestor `.git` marker. Marker symlinks are
not accepted.

Home-directory resolution is itself part of the root security boundary. If the
current user's home cannot be determined and canonicalized, guarded automatic
root selection fails closed rather than continuing with the home/ancestor check
disabled. Explicit `--root` uses the same fail-closed rule unless the user has
also supplied the deliberate `--allow-broad-root` override.

On Unix, automatic discovery also stops before trusting a directory writable by
group or other users. This prevents an attacker from widening a marker-less
project into a shared ancestor by placing a forged boundary marker such as
`/tmp/.git`. An intentionally selected shared project can still be supplied as
an explicit root subject to the explicit-root guards.

On Windows, Sippion keeps `unsafe` code forbidden and the stable safe Rust
filesystem surface does not expose sufficient DACL writeability information to
prove that an arbitrary ancestor is not shared-writable. Instead of silently
accepting that uncertainty, `--root-auto` is constrained to the canonical
current-user profile subtree. A trusted project outside that subtree remains
available through intentional explicit `sippion mcp --root <path>` selection.

Automatic discovery fails closed when it cannot identify a project or when the
candidate is the user's home directory, an ancestor of that home directory, or
the filesystem root. Explicit `sippion mcp --root <path>` applies the same broad
root guard. A deliberately broad manual scan requires `--allow-broad-root`;
setup never adds that override. This keeps a client that happens to launch from
an unexpectedly broad working directory from silently granting Sippion the
same broad read scope.

## Client-configuration mutation safety

`sippion setup` and `sippion uninstall` modify only their documented per-user
client configuration and rule files. Text files managed with Sippion BEGIN/END
markers are treated conservatively: exactly one ordered marker pair is required
before an existing managed block can be replaced or removed. Missing halves,
duplicate markers, or reversed markers cause the operation to fail closed
without modifying that file. Managed files that are themselves symlinks are
also refused rather than followed or replaced.

Before setup, doctor, or uninstall mutates/accepts managed configuration,
Sippion also checks the managed parent directories (`~/.codex`, `~/.claude`,
`~/.gemini`, and `~/.gemini/config`). A parent that is a symlink or a
non-directory is refused. This prevents a harmless-looking leaf filename from
silently redirecting configuration mutation through a symlinked parent tree.

Before a setup attempt, Sippion snapshots all six managed client configuration
and rule files in memory together with their existing permission metadata and
any legacy `.sippion-backup` siblings. If any client update fails, files touched
by that setup attempt are restored to those pre-attempt snapshots, including
removing files that did not exist beforehand and restoring Unix permission bits.
A rollback failure is reported separately instead of being hidden by the
original setup error.

Uninstall now uses the same transaction boundary. It snapshots the six managed
files plus legacy backup siblings before removal. If any removal fails after an
earlier target was already changed, Sippion restores the complete pre-attempt
snapshot rather than leaving a partially uninstalled client configuration.
Rollback failures are again reported separately.

Setup no longer creates persistent `.sippion-backup` files because a full copy
of a client configuration can retain credentials that the user later removes
from the live file. Legacy backup files left by older Sippion releases are
removed transactionally during setup and are also cleaned by uninstall. If a
setup attempt fails after legacy-backup cleanup, the pre-attempt snapshots
restore those older backup files as part of the same rollback boundary.

On Unix, MCP client configuration files are created with owner-only `0600`
permissions and existing files are repaired to `0600` even when their contents
are already current. Rule files preserve their previous mode. Replacement temp
files use exclusive creation and are written with a private mode before atomic
rename. On Windows, a staged replacement is written into an existing managed
file in place so its existing ACL and file identity are preserved; a failed
write immediately restores the original bytes. Transactional uninstall rollback
uses the same in-place principle for existing Windows files so their ACL/file
identity is not replaced merely to restore content.

The release installers apply the same transaction boundary to the installed
binary. They save the previous Sippion executable before replacement and restore
it, or remove a newly introduced executable, if `sippion setup` does not finish
successfully. PATH mutation on Windows occurs only after setup succeeds.

`sippion doctor` returns a non-zero process status whenever any expected MCP
configuration or global rule is missing, mismatched, malformed, unreadable, or
is located through a refused managed-parent symlink, so scripts and CI can use it
as an actual health check rather than parsing text. Legacy registrations using
`--root .` are reported as mismatched so a subsequent `setup` migrates them to
the guarded `--root-auto` form.

## Retrieval matching and completeness

Retrieval query terms, RAM-index terms, path ranking, excerpt matching, and
structural graph names use Unicode-aware lowercasing rather than ASCII-only
lowercasing. When Unicode lowercasing changes UTF-8 encoded length, excerpt
matching maps the folded match back to the original source byte position before
building a model-visible range; folded byte offsets are never reused directly
against the original string.

Filesystem safety policy remains intentionally separate: denied/pruned path
matching continues to use ASCII case folding so this retrieval-quality change
does not broaden or reinterpret the security policy.

Ignore controls affect whether Sippion can claim a complete repository-wide
`NO_MATCH`. Empty, whitespace-only, and comment-only `.gitignore` / `.ignore`
files cannot hide content and therefore no longer degrade that status. Files
with an effective rule still contribute a conservative exclusion sentinel.
Unreadable, non-UTF-8, or unusually large ignore controls are also treated
conservatively as potentially effective rather than being trusted as empty.

## Release and dependency supply chain

CI and release workflows pin third-party GitHub Actions to full commit SHAs.
CI audits `Cargo.lock` against the RustSec advisory database. Release binaries
are built from an explicitly selected tag resolved to an immutable commit,
checksummed, and given GitHub artifact attestations. Release installer scripts
are also published as checksummed, attested assets.

Release verification binds attestations to three independent facts: the
repository (`Sitten-Tokyo/Sippion`), the expected signer workflow, and the exact
commit SHA resolved from the selected release tag. Binary attestations must come
from `.github/workflows/release-build.yml`; installer attestations must come from
`.github/workflows/release-draft.yml`. Repository identity alone is not treated
as sufficient provenance.

Manual draft-release dispatches must run on the exact tag ref supplied as input;
the workflow refuses a dispatch whose GitHub Actions source SHA differs from the
resolved tag commit. Branch-triggered releases continue to require their
one-shot release branch to point exactly at current `main` and the manifest
version to match the release tag.

Pull-request supply-chain smoke tests exercise release builds without minting
new attestations. A separate consumer-side smoke check downloads the newest
non-draft published installer and Linux binary and verifies both with the same
repository, signer-workflow, and source-SHA policy used by installation.

### Default one-command installation

The README's bootstrap URL is pinned to an exact Sippion Git commit rather than
`main`, so the bootstrap script referenced by the documentation cannot change
when the branch moves. The bootstrap then:

1. asks GitHub CLI for the newest non-draft published Sippion release, including
   prereleases;
2. resolves that tag to one exact commit SHA and pins subsequent downloads to
   the tag;
3. downloads the release installer and its `.sha256` file and verifies the
   installer SHA-256;
4. **before executing the installer**, verifies the installer GitHub artifact
   attestation against `Sitten-Tokyo/Sippion`, the release-draft signer workflow,
   and the selected tag commit SHA;
5. runs the release installer with binary artifact-attestation verification
   explicitly required; and
6. the release installer downloads the platform binary and matching `.sha256`
   file, verifies the binary SHA-256, verifies its GitHub artifact attestation
   against the release-build signer workflow and the same release commit SHA,
   installs it for the current user, and runs transactional `sippion setup`.

The default bootstrap therefore requires a GitHub CLI with `gh attestation`
support and working GitHub authentication. If provenance verification cannot be
performed or fails, installation stops before unverified installer code runs.
`SIPPION_BOOTSTRAP_VERIFY_ONLY=1` succeeds only after installer provenance, not
just its release-local checksum, has been verified.

Checksums remain useful for corruption and mismatch detection, but they are not
treated as an independent authenticity mechanism when both an artifact and its
checksum come from the same release.

### Checksum-only explicit opt-out

Direct use of `scripts/install.sh` or `scripts/install.ps1` also requires strict
binary artifact-attestation verification by default and fails closed unless a
GitHub CLI with `gh attestation` support is available and authenticated.

`SIPPION_REQUIRE_ATTESTATION=0` is an explicit opt-out for controlled
environments where artifact provenance has already been verified through
another trusted mechanism. In that mode the installer still verifies the
published SHA-256 but emits a warning because release-local checksums alone do
not provide independent authenticity if the release itself is compromised.
The primary README bootstrap does not use this opt-out.

`SIPPION_RELEASE_TAG` pins binary downloads to one release. An explicit
`SIPPION_RELEASE_BASE_URL` / `ReleaseBaseUrl` remains available for controlled
HTTPS mirrors and forks; when strict attestation verification remains enabled,
a release tag is also required so the source commit can be verified.
