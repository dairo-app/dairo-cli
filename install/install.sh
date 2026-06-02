#!/usr/bin/env sh
set -eu

REPO="dairo-app/dairo-cli"
VERSION="${DAIRO_CLI_VERSION:-latest}"
INSTALL_DIR="${DAIRO_INSTALL_DIR:-$HOME/.dairo/bin}"
BASE_URL="${DAIRO_DOWNLOAD_BASE_URL:-https://dairo.app/downloads/cli}"

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os" in
  darwin) platform="darwin" ;;
  linux) platform="linux" ;;
  *) echo "Dairo CLI is not available for OS: $os" >&2; exit 1 ;;
esac
case "$arch" in
  arm64|aarch64) cpu="arm64" ;;
  x86_64|amd64) cpu="x64" ;;
  *) echo "Dairo CLI is not available for architecture: $arch" >&2; exit 1 ;;
esac

case "$platform-$cpu" in
  darwin-arm64) target="aarch64-apple-darwin" ;;
  darwin-x64) target="x86_64-apple-darwin" ;;
  linux-arm64) target="aarch64-unknown-linux-gnu" ;;
  linux-x64) target="x86_64-unknown-linux-gnu" ;;
  *) echo "Dairo CLI is not available for $platform-$cpu" >&2; exit 1 ;;
esac

asset="dairo-$target.tar.gz"
version_path="$VERSION"
url="$BASE_URL/$version_path/$asset"
checksums_url="$BASE_URL/$version_path/checksums.txt"

tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

echo "Downloading Dairo CLI $VERSION for $target..."
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$url" -o "$tmp/$asset"
  curl -fsSL "$checksums_url" -o "$tmp/checksums.txt"
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$tmp/$asset" "$url"
  wget -qO "$tmp/checksums.txt" "$checksums_url"
else
  echo "curl or wget is required" >&2
  exit 1
fi

expected="$(awk -v file="$asset" '$2 == file { print $1 }' "$tmp/checksums.txt")"
if [ -z "$expected" ]; then
  echo "Could not find checksum for $asset" >&2
  exit 1
fi
actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
if [ "$actual" != "$expected" ]; then
  echo "Checksum mismatch for $asset" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/dairo" "$INSTALL_DIR/dairo"

echo "Dairo CLI installed to $INSTALL_DIR/dairo"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add this to your shell profile: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac
"$INSTALL_DIR/dairo" --version
