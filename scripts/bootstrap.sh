#!/bin/sh
set -eu

# Minimal immutable bootstrap for Sippion. It downloads a pinned GitHub CLI into
# a temporary directory, verifies that CLI against GitHub's published checksum,
# then uses a public attestation bundle to verify Sippion release provenance.
# Nothing from the temporary GitHub CLI is installed persistently.

repo=Sitten-Tokyo/Sippion
gh_version=2.97.0
gh_checksums_sha256=61905c69ec8660f310814ec98395cdd0c2d07aabf024c597ec45813984a02334
: "${SIPPION_BOOTSTRAP_VERIFY_ONLY:=0}"

case "$SIPPION_BOOTSTRAP_VERIFY_ONLY" in
  0|1) ;;
  *) echo "SIPPION_BOOTSTRAP_VERIFY_ONLY must be 0 or 1." >&2; exit 2 ;;
esac

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 2; }
command -v mktemp >/dev/null 2>&1 || { echo "mktemp is required" >&2; exit 2; }
command -v awk >/dev/null 2>&1 || { echo "awk is required" >&2; exit 2; }
command -v sed >/dev/null 2>&1 || { echo "sed is required" >&2; exit 2; }
command -v grep >/dev/null 2>&1 || { echo "grep is required" >&2; exit 2; }
command -v find >/dev/null 2>&1 || { echo "find is required" >&2; exit 2; }

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

verify_sha256() {
  expected=$1
  file=$2
  actual=$(sha256_file "$file")
  if [ "$actual" != "$expected" ]; then
    echo "SHA-256 verification failed for $(basename "$file")." >&2
    return 1
  fi
}

fetch_bundle() {
  digest=$1
  response=$2
  bundle=$3
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2026-03-10' \
    "https://api.github.com/repos/$repo/attestations/sha256:$digest?predicate_type=provenance&per_page=1" \
    --output "$response"
  bundle_url=$(sed -n 's/.*"bundle_url"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$response" | head -n 1 | sed 's/\\u0026/\&/g; s#\\/#/#g')
  case "$bundle_url" in
    https://*) ;;
    *) echo "Could not resolve a valid GitHub attestation bundle URL." >&2; return 1 ;;
  esac
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
    "$bundle_url" --output "$bundle"
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sippion-bootstrap.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
  Linux:x86_64|Linux:amd64)
    gh_archive="gh_${gh_version}_linux_amd64.tar.gz"
    gh_extract=tar
    ;;
  Darwin:arm64|Darwin:aarch64)
    gh_archive="gh_${gh_version}_macOS_arm64.zip"
    gh_extract=zip
    ;;
  Darwin:x86_64|Darwin:amd64)
    gh_archive="gh_${gh_version}_macOS_amd64.zip"
    gh_extract=zip
    ;;
  *)
    echo "Unsupported bootstrap platform: $os/$arch." >&2
    exit 2
    ;;
esac

case "$gh_extract" in
  tar) command -v tar >/dev/null 2>&1 || { echo "tar is required" >&2; exit 2; } ;;
  zip) command -v unzip >/dev/null 2>&1 || { echo "unzip is required" >&2; exit 2; } ;;
esac

gh_release_base="https://github.com/cli/cli/releases/download/v${gh_version}"
gh_checksums="$tmp/gh_${gh_version}_checksums.txt"
gh_archive_path="$tmp/$gh_archive"

curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$gh_release_base/gh_${gh_version}_checksums.txt" --output "$gh_checksums"
verify_sha256 "$gh_checksums_sha256" "$gh_checksums"

gh_expected=$(awk -v name="$gh_archive" '$2 == name { print $1; exit }' "$gh_checksums")
if ! printf '%s\n' "$gh_expected" | grep -Eq '^[0-9A-Fa-f]{64}$'; then
  echo "Pinned GitHub CLI checksum entry was not found." >&2
  exit 2
fi
gh_expected=$(printf '%s' "$gh_expected" | tr 'A-F' 'a-f')

curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$gh_release_base/$gh_archive" --output "$gh_archive_path"
verify_sha256 "$gh_expected" "$gh_archive_path"

mkdir -p "$tmp/gh"
case "$gh_extract" in
  tar) tar -xzf "$gh_archive_path" -C "$tmp/gh" ;;
  zip) unzip -q "$gh_archive_path" -d "$tmp/gh" ;;
esac
gh_bin=$(find "$tmp/gh" -type f -name gh -path '*/bin/gh' -print | head -n 1)
[ -n "$gh_bin" ] && [ -x "$gh_bin" ] || { echo "Verified GitHub CLI archive did not contain an executable gh under bin." >&2; exit 2; }

release_json="$tmp/releases.json"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  -H 'Accept: application/vnd.github+json' \
  -H 'X-GitHub-Api-Version: 2026-03-10' \
  "https://api.github.com/repos/$repo/releases?per_page=1" --output "$release_json"

tag=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$release_json" | head -n 1)
if ! printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'; then
  echo "Could not resolve a valid published Sippion release tag." >&2
  exit 2
fi

release_base="https://github.com/$repo/releases/download/$tag"
installer="$tmp/install.sh"
installer_checksum="$tmp/install.sh.sha256"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$release_base/install.sh" --output "$installer"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$release_base/install.sh.sha256" --output "$installer_checksum"

installer_expected=$(awk 'NR == 1 { print $1; exit }' "$installer_checksum")
if ! printf '%s\n' "$installer_expected" | grep -Eq '^[0-9A-Fa-f]{64}$'; then
  echo "The Sippion installer checksum is invalid." >&2
  exit 2
fi
installer_expected=$(printf '%s' "$installer_expected" | tr 'A-F' 'a-f')
verify_sha256 "$installer_expected" "$installer"
installer_actual=$(sha256_file "$installer")

installer_attestations="$tmp/installer-attestations.json"
installer_bundle="$tmp/installer-attestation.bundle.json"
fetch_bundle "$installer_actual" "$installer_attestations" "$installer_bundle"
"$gh_bin" attestation verify "$installer" --repo "$repo" --bundle "$installer_bundle" >/dev/null

if [ "$SIPPION_BOOTSTRAP_VERIFY_ONLY" = "1" ]; then
  echo "Verified Sippion bootstrap path for $tag."
  exit 0
fi

PATH="$(dirname "$gh_bin"):$PATH" \
SIPPION_RELEASE_TAG="$tag" \
SIPPION_ATTESTATION_REPOSITORY="$repo" \
sh "$installer"

printf '\nSippion is installed and registered for Codex, Claude Code, and Antigravity.\n'
printf 'Restart those AI clients to reload their MCP settings.\n'
