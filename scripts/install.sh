#!/bin/sh
set -eu

# Installer for a published Sippion release. By default it resolves the newest
# published release, including prereleases. Artifact-attestation verification
# is required by default and is bound to the release build workflow plus the
# exact commit referenced by the selected release tag.
: "${SIPPION_ATTESTATION_REPOSITORY:=Sitten-Tokyo/Sippion}"
: "${SIPPION_REQUIRE_ATTESTATION:=1}"
: "${SIPPION_RELEASE_TAG:=}"
: "${SIPPION_RELEASE_SHA:=}"
: "${SIPPION_RELEASE_BASE_URL:=}"
: "${SIPPION_INSTALL_VERIFY_ONLY:=0}"

case "$SIPPION_REQUIRE_ATTESTATION" in
  0|1) ;;
  *) echo "SIPPION_REQUIRE_ATTESTATION must be 0 or 1." >&2; exit 2 ;;
esac
case "$SIPPION_INSTALL_VERIFY_ONLY" in
  0|1) ;;
  *) echo "SIPPION_INSTALL_VERIFY_ONLY must be 0 or 1." >&2; exit 2 ;;
esac

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 2; }
command -v mktemp >/dev/null 2>&1 || { echo "mktemp is required" >&2; exit 2; }
command -v grep >/dev/null 2>&1 || { echo "grep is required" >&2; exit 2; }
command -v sed >/dev/null 2>&1 || { echo "sed is required" >&2; exit 2; }
command -v awk >/dev/null 2>&1 || { echo "awk is required" >&2; exit 2; }

case "$SIPPION_ATTESTATION_REPOSITORY" in
  */*) ;;
  *) echo "SIPPION_ATTESTATION_REPOSITORY must be owner/repository." >&2; exit 2 ;;
esac
if ! printf '%s\n' "$SIPPION_ATTESTATION_REPOSITORY" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$'; then
  echo "SIPPION_ATTESTATION_REPOSITORY contains unsupported characters." >&2
  exit 2
fi

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sippion-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

resolve_latest_published_tag() {
  if command -v gh >/dev/null 2>&1; then
    # Authenticated release listings can include drafts for repository writers,
    # so filter publish state explicitly instead of trusting array position.
    gh api "repos/$SIPPION_ATTESTATION_REPOSITORY/releases?per_page=20" \
      --jq '[.[] | select(.draft == false and .published_at != null)][0].tag_name // empty'
    return
  fi

  # Checksum-only controlled environments may intentionally omit gh. Use the
  # unauthenticated public endpoint there; it exposes published releases only.
  release_json="$tmp/releases.json"
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2026-03-10' \
    "https://api.github.com/repos/$SIPPION_ATTESTATION_REPOSITORY/releases?per_page=20" \
    --output "$release_json"
  sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$release_json" | head -n 1
}

resolve_tag_sha() {
  release_tag=$1
  object_type=$(gh api "repos/$SIPPION_ATTESTATION_REPOSITORY/git/ref/tags/$release_tag" --jq '.object.type')
  object_sha=$(gh api "repos/$SIPPION_ATTESTATION_REPOSITORY/git/ref/tags/$release_tag" --jq '.object.sha')
  while [ "$object_type" = "tag" ]; do
    object_type=$(gh api "repos/$SIPPION_ATTESTATION_REPOSITORY/git/tags/$object_sha" --jq '.object.type')
    object_sha=$(gh api "repos/$SIPPION_ATTESTATION_REPOSITORY/git/tags/$object_sha" --jq '.object.sha')
  done
  if [ "$object_type" != "commit" ] || ! printf '%s\n' "$object_sha" | grep -Eq '^[0-9a-f]{40}$'; then
    echo "Release tag $release_tag does not resolve to a commit SHA." >&2
    return 2
  fi
  printf '%s\n' "$object_sha"
}

if [ -z "$SIPPION_RELEASE_BASE_URL" ]; then
  if [ -z "$SIPPION_RELEASE_TAG" ]; then
    SIPPION_RELEASE_TAG=$(resolve_latest_published_tag)
  fi
  if ! printf '%s\n' "$SIPPION_RELEASE_TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'; then
    echo "Resolved release tag is invalid: $SIPPION_RELEASE_TAG" >&2
    exit 2
  fi
  SIPPION_RELEASE_BASE_URL="https://github.com/$SIPPION_ATTESTATION_REPOSITORY/releases/download/$SIPPION_RELEASE_TAG"
else
  case "$SIPPION_RELEASE_BASE_URL" in
    https://*) ;;
    *) echo "SIPPION_RELEASE_BASE_URL must be an absolute HTTPS URL." >&2; exit 2 ;;
  esac
  if [ "$SIPPION_REQUIRE_ATTESTATION" = "1" ] && [ -z "$SIPPION_RELEASE_TAG" ]; then
    echo "SIPPION_RELEASE_TAG is required with a custom release base URL when attestation verification is enabled." >&2
    exit 2
  fi
  if [ -n "$SIPPION_RELEASE_TAG" ] && ! printf '%s\n' "$SIPPION_RELEASE_TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'; then
    echo "Resolved release tag is invalid: $SIPPION_RELEASE_TAG" >&2
    exit 2
  fi
fi

case "$SIPPION_RELEASE_BASE_URL" in
  https://*) ;;
  *) echo "SIPPION_RELEASE_BASE_URL must be an absolute HTTPS URL." >&2; exit 2 ;;
esac

release_sha=""
if [ "$SIPPION_REQUIRE_ATTESTATION" = "1" ]; then
  command -v gh >/dev/null 2>&1 || {
    echo "GitHub CLI is required for artifact-attestation verification." >&2
    exit 2
  }
  gh attestation --help >/dev/null 2>&1 || {
    echo "A GitHub CLI version with 'gh attestation' support is required." >&2
    exit 2
  }
  release_sha=$(resolve_tag_sha "$SIPPION_RELEASE_TAG")
  if [ -n "$SIPPION_RELEASE_SHA" ]; then
    if ! printf '%s\n' "$SIPPION_RELEASE_SHA" | grep -Eq '^[0-9a-f]{40}$'; then
      echo "SIPPION_RELEASE_SHA must be a 40-character lowercase Git commit SHA." >&2
      exit 2
    fi
    if [ "$SIPPION_RELEASE_SHA" != "$release_sha" ]; then
      echo "SIPPION_RELEASE_SHA does not match the selected release tag." >&2
      exit 2
    fi
  fi
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
  Darwin:arm64|Darwin:aarch64) artifact=sippion-macos-aarch64 ;;
  Darwin:x86_64|Darwin:amd64) artifact=sippion-macos-x86_64 ;;
  Linux:x86_64|Linux:amd64) artifact=sippion-linux-x86_64 ;;
  *)
    echo "Unsupported platform: $os/$arch. Required targets are macOS arm64/x86_64 and Linux x86_64." >&2
    exit 2
    ;;
esac

binary="$tmp/$artifact"
checksum="$tmp/$artifact.sha256"
base=${SIPPION_RELEASE_BASE_URL%/}

curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$base/$artifact" --output "$binary"
curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
  "$base/$artifact.sha256" --output "$checksum"

expected=$(awk 'NR == 1 { print $1; exit }' "$checksum")
case "$expected" in
  ''|*[!0-9A-Fa-f]*)
    echo "The release checksum is not a valid SHA-256 digest." >&2
    exit 2
    ;;
esac
if [ "${#expected}" -ne 64 ]; then
  echo "The release checksum is not a valid SHA-256 digest." >&2
  exit 2
fi
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')

if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$binary" | awk '{ print $1 }')
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$binary" | awk '{ print $1 }')
else
  echo "sha256sum or shasum is required" >&2
  exit 2
fi
if [ "$actual" != "$expected" ]; then
  echo "Sippion checksum verification failed." >&2
  exit 1
fi

if [ "$SIPPION_REQUIRE_ATTESTATION" = "1" ]; then
  if ! gh attestation verify "$binary" \
    --repo "$SIPPION_ATTESTATION_REPOSITORY" \
    --signer-workflow "$SIPPION_ATTESTATION_REPOSITORY/.github/workflows/release-build.yml" \
    --source-digest "$release_sha" >/dev/null; then
    echo "Sippion GitHub artifact attestation verification failed." >&2
    exit 1
  fi
else
  echo "Warning: GitHub artifact attestation verification was explicitly disabled." >&2
fi

if [ "$SIPPION_INSTALL_VERIFY_ONLY" = "1" ]; then
  if [ -n "$release_sha" ]; then
    echo "Verified Sippion release artifact $artifact from $release_sha."
  else
    echo "Verified Sippion release artifact $artifact by checksum only."
  fi
  exit 0
fi

install_dir=${SIPPION_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$install_dir"
install_path="$install_dir/sippion"
previous_binary="$tmp/previous-sippion"
had_previous=0

if [ -L "$install_path" ]; then
  echo "Refusing to replace symlinked install path: $install_path" >&2
  exit 2
fi
if [ -e "$install_path" ]; then
  if [ ! -f "$install_path" ]; then
    echo "Refusing to replace non-file install path: $install_path" >&2
    exit 2
  fi
  cp "$install_path" "$previous_binary"
  had_previous=1
fi

install -m 0755 "$binary" "$install_path"
if "$install_path" setup; then
  :
else
  setup_status=$?
  if [ "$had_previous" = "1" ]; then
    if ! install -m 0755 "$previous_binary" "$install_path"; then
      rollback_path="$install_path.sippion-rollback"
      cp "$previous_binary" "$rollback_path" 2>/dev/null || true
      echo "Sippion setup failed and the previous binary could not be restored; a recovery copy was attempted at $rollback_path." >&2
      exit 2
    fi
    echo "Sippion setup failed; the previous binary was restored." >&2
  else
    if ! rm -f "$install_path"; then
      echo "Sippion setup failed and the newly installed binary could not be removed: $install_path" >&2
      exit 2
    fi
    echo "Sippion setup failed; the newly installed binary was removed." >&2
  fi
  exit "$setup_status"
fi

echo "Installed Sippion at $install_path"
case ":${PATH}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH to call sippion directly." ;;
esac
