#!/bin/sh
set -eu

# One-command bootstrap for Sippion. This bootstrap is intended to be invoked
# from a commit-SHA-pinned raw GitHub URL. It resolves the newest non-draft
# published release (including prereleases), pins all downloads to that tag,
# verifies the installer checksum before execution, then delegates binary
# verification, installation, and client registration.
#
# The default path verifies published SHA-256 checksums only and needs no
# GitHub CLI or authentication. Set SIPPION_STRICT_PROVENANCE=1 to also verify
# GitHub artifact attestations before anything runs (requires `gh` with
# `gh attestation` support and GitHub authentication).

repo=Sitten-Tokyo/Sippion
: "${SIPPION_BOOTSTRAP_VERIFY_ONLY:=0}"
: "${SIPPION_STRICT_PROVENANCE:=0}"

case "$SIPPION_BOOTSTRAP_VERIFY_ONLY" in
  0|1) ;;
  *) echo "SIPPION_BOOTSTRAP_VERIFY_ONLY must be 0 or 1." >&2; exit 2 ;;
esac
case "$SIPPION_STRICT_PROVENANCE" in
  0|1) ;;
  *) echo "SIPPION_STRICT_PROVENANCE must be 0 or 1." >&2; exit 2 ;;
esac

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 2; }
command -v mktemp >/dev/null 2>&1 || { echo "mktemp is required" >&2; exit 2; }
command -v awk >/dev/null 2>&1 || { echo "awk is required" >&2; exit 2; }
command -v grep >/dev/null 2>&1 || { echo "grep is required" >&2; exit 2; }
command -v sed >/dev/null 2>&1 || { echo "sed is required" >&2; exit 2; }
if [ "$SIPPION_STRICT_PROVENANCE" = "1" ]; then
  command -v gh >/dev/null 2>&1 || { echo "Strict provenance verification requires the GitHub CLI." >&2; exit 2; }
  gh attestation --help >/dev/null 2>&1 || {
    echo "Strict provenance verification requires a GitHub CLI version with 'gh attestation' support." >&2
    exit 2
  }
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{ print $1 }'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    echo "sha256sum or shasum is required" >&2
    return 2
  fi
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sippion-bootstrap.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

resolve_published_tag() {
  if [ "$SIPPION_STRICT_PROVENANCE" = "1" ] || command -v gh >/dev/null 2>&1; then
    gh release list --repo "$repo" --exclude-drafts --limit 1 --json tagName --jq '.[0].tagName'
    return
  fi

  # Without gh, deliberately use the public unauthenticated endpoint. Public
  # release listing never exposes drafts.
  release_json="$tmp/releases.json"
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2026-03-10' \
    "https://api.github.com/repos/$repo/releases?per_page=1" \
    --output "$release_json"
  sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$release_json" | head -n 1
}

tag=$(resolve_published_tag)
if ! printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'; then
  echo "Could not resolve a valid non-draft published Sippion release tag." >&2
  exit 2
fi
release_sha=""
if [ "$SIPPION_STRICT_PROVENANCE" = "1" ]; then
  release_sha=$(gh api "repos/$repo/commits/$tag" --jq '.sha')
  if ! printf '%s\n' "$release_sha" | grep -Eq '^[0-9a-f]{40}$'; then
    echo "Could not resolve the published release tag to a commit SHA." >&2
    exit 2
  fi
fi

release_base="https://github.com/$repo/releases/download/$tag"
installer="$tmp/install.sh"
installer_checksum="$tmp/install.sh.sha256"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$release_base/install.sh" --output "$installer"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$release_base/install.sh.sha256" --output "$installer_checksum"

expected=$(awk 'NR == 1 { print $1; exit }' "$installer_checksum")
if ! printf '%s\n' "$expected" | grep -Eq '^[0-9A-Fa-f]{64}$'; then
  echo "The Sippion installer checksum is invalid." >&2
  exit 2
fi
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
actual=$(sha256_file "$installer")
if [ "$actual" != "$expected" ]; then
  echo "Sippion installer checksum verification failed." >&2
  exit 1
fi

if [ "$SIPPION_STRICT_PROVENANCE" = "1" ]; then
  if ! gh attestation verify "$installer" \
    --repo "$repo" \
    --signer-workflow "$repo/.github/workflows/release-draft.yml" \
    --source-digest "$release_sha" >/dev/null; then
    echo "Sippion installer provenance verification failed." >&2
    exit 1
  fi
fi

if [ "$SIPPION_BOOTSTRAP_VERIFY_ONLY" = "1" ]; then
  if [ "$SIPPION_STRICT_PROVENANCE" = "1" ]; then
    echo "Verified Sippion bootstrap installer provenance for $tag."
  else
    echo "Verified Sippion bootstrap installer checksum for $tag."
  fi
  exit 0
fi

SIPPION_REQUIRE_ATTESTATION="$SIPPION_STRICT_PROVENANCE" \
SIPPION_RELEASE_TAG="$tag" \
SIPPION_ATTESTATION_REPOSITORY="$repo" \
sh "$installer"

printf '\nSippion is installed and pre-registered for Codex, Claude Code, Antigravity, and OpenCode.\n'
printf 'Restart those AI clients to reload their MCP settings.\n'
