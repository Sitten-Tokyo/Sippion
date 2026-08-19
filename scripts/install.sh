#!/bin/sh
set -eu

# Installer for a published Sippion release. By default it resolves the newest
# published release, including prereleases, through GitHub's public REST API.
# SIPPION_RELEASE_TAG pins installer and binary downloads to one release.
# SIPPION_RELEASE_BASE_URL is an explicit HTTPS override for mirrors/forks.
: "${SIPPION_ATTESTATION_REPOSITORY:=Sitten-Tokyo/Sippion}"
: "${SIPPION_REQUIRE_ATTESTATION:=1}"
: "${SIPPION_RELEASE_TAG:=}"
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

gh_supports_attestation=0
if command -v gh >/dev/null 2>&1 && gh attestation --help >/dev/null 2>&1; then
  gh_supports_attestation=1
fi
if [ "$SIPPION_REQUIRE_ATTESTATION" = "1" ] && [ "$gh_supports_attestation" != "1" ]; then
  echo "GitHub CLI with 'gh attestation' support is required for provenance verification." >&2
  exit 2
fi

extract_inline_bundle() {
  input=$1
  output=$2
  awk '
    BEGIN { target="\"bundle\""; window=""; found=0; started=0; depth=0; in_string=0; escaped=0 }
    {
      text=$0 "\n"
      for (i=1; i<=length(text); i++) {
        c=substr(text,i,1)
        if (!started) {
          if (!found) {
            window=window c
            if (length(window)>length(target)) window=substr(window,length(window)-length(target)+1)
            if (window==target) found=1
          } else if (c=="{") {
            started=1
            depth=1
            printf "%s", c
          }
          continue
        }
        printf "%s", c
        if (escaped) { escaped=0; continue }
        if (in_string) {
          if (c=="\\") escaped=1
          else if (c=="\"") in_string=0
          continue
        }
        if (c=="\"") in_string=1
        else if (c=="{") depth++
        else if (c=="}") {
          depth--
          if (depth==0) exit
        }
      }
    }
  ' "$input" > "$output"
  [ -s "$output" ] && [ "$(head -c 1 "$output")" = "{" ] || return 1
}

fetch_inline_bundle() {
  digest=$1
  response=$2
  bundle=$3
  curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
    -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' \
    "https://api.github.com/repos/$SIPPION_ATTESTATION_REPOSITORY/attestations/sha256:$digest?predicate_type=provenance&per_page=1" \
    --output "$response"
  if ! extract_inline_bundle "$response" "$bundle"; then
    echo "GitHub did not return an inline attestation bundle." >&2
    return 1
  fi
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sippion-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM

if [ -z "$SIPPION_RELEASE_BASE_URL" ]; then
  if [ -z "$SIPPION_RELEASE_TAG" ]; then
    release_json="$tmp/releases.json"
    curl --fail --location --proto '=https' --proto-redir '=https' --tlsv1.2 --silent --show-error \
      -H 'Accept: application/vnd.github+json' \
      -H 'X-GitHub-Api-Version: 2026-03-10' \
      "https://api.github.com/repos/$SIPPION_ATTESTATION_REPOSITORY/releases?per_page=1" \
      --output "$release_json"
    SIPPION_RELEASE_TAG=$(sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$release_json" | head -n 1)
  fi

  if ! printf '%s\n' "$SIPPION_RELEASE_TAG" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z][0-9A-Za-z.-]*)?$'; then
    echo "Resolved release tag is invalid: $SIPPION_RELEASE_TAG" >&2
    exit 2
  fi
  SIPPION_RELEASE_BASE_URL="https://github.com/$SIPPION_ATTESTATION_REPOSITORY/releases/download/$SIPPION_RELEASE_TAG"
fi

case "$SIPPION_RELEASE_BASE_URL" in
  https://*) ;;
  *) echo "SIPPION_RELEASE_BASE_URL must be an absolute HTTPS URL." >&2; exit 2 ;;
esac

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
else
  actual=$(shasum -a 256 "$binary" | awk '{ print $1 }')
fi
if [ "$actual" != "$expected" ]; then
  echo "Sippion checksum verification failed." >&2
  exit 1
fi

if [ "$gh_supports_attestation" = "1" ]; then
  attestation_json="$tmp/attestations.json"
  attestation_bundle="$tmp/attestation.bundle.json"
  fetch_inline_bundle "$actual" "$attestation_json" "$attestation_bundle"
  if ! gh attestation verify "$binary" \
    --repo "$SIPPION_ATTESTATION_REPOSITORY" \
    --bundle "$attestation_bundle" >/dev/null; then
    echo "Sippion GitHub artifact attestation verification failed." >&2
    exit 1
  fi
elif [ "$SIPPION_REQUIRE_ATTESTATION" = "1" ]; then
  echo "GitHub CLI with 'gh attestation' support is required for provenance verification." >&2
  exit 2
else
  echo "Warning: GitHub artifact attestation verification was explicitly disabled." >&2
fi

if [ "$SIPPION_INSTALL_VERIFY_ONLY" = "1" ]; then
  echo "Verified Sippion release artifact $artifact."
  exit 0
fi

install_dir=${SIPPION_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$install_dir"
install_path="$install_dir/sippion"
install -m 0755 "$binary" "$install_path"
"$install_path" setup

echo "Installed Sippion at $install_path"
case ":${PATH}:" in
  *:"$install_dir":*) ;;
  *) echo "Add $install_dir to PATH to call sippion directly." ;;
esac
