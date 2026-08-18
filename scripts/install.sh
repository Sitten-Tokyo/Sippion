#!/bin/sh
set -eu

# One-line installer for a published Sippion release.
# The release URL is intentionally configurable because the OSS repository owner
# may differ between forks. Set SIPPION_RELEASE_BASE_URL in the one-liner or
# replace the default after publishing the canonical repository.
: "${SIPPION_RELEASE_BASE_URL:=https://github.com/OWNER/REPOSITORY/releases/latest/download}"
case "$SIPPION_RELEASE_BASE_URL" in
  *OWNER/REPOSITORY*)
    echo "Set SIPPION_RELEASE_BASE_URL to the published Sippion release URL." >&2
    exit 2
    ;;
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

command -v curl >/dev/null 2>&1 || { echo "curl is required" >&2; exit 2; }
command -v mktemp >/dev/null 2>&1 || { echo "mktemp is required" >&2; exit 2; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/sippion-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT HUP INT TERM
binary="$tmp/$artifact"
checksum="$tmp/$artifact.sha256"
base=${SIPPION_RELEASE_BASE_URL%/}

curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
  "$base/$artifact" --output "$binary"
curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
  "$base/$artifact.sha256" --output "$checksum"

expected=$(awk 'NR == 1 { print $1; exit }' "$checksum")
if [ -z "$expected" ]; then
  echo "The release checksum is empty." >&2
  exit 2
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$binary" | awk '{ print $1 }')
else
  actual=$(shasum -a 256 "$binary" | awk '{ print $1 }')
fi
if [ "$actual" != "$expected" ]; then
  echo "Sippion checksum verification failed." >&2
  exit 1
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
