#!/usr/bin/env sh
set -eu

VERSION="${DAIRO_CLI_VERSION:-latest}"
INSTALL_DIR="${DAIRO_INSTALL_DIR:-$HOME/.dairo/bin}"
BASE_URL="${DAIRO_DOWNLOAD_BASE_URL:-https://dairo.app/downloads/cli}"

# Release tags are v-prefixed; accept both "0.1.0" and "v0.1.0".
case "$VERSION" in
  latest|v*) ;;
  *) VERSION="v$VERSION" ;;
esac

shell_profile() {
  shell_name="$(basename "${SHELL:-}")"
  case "$shell_name" in
    zsh) printf '%s\n' "$HOME/.zshrc" ;;
    bash) printf '%s\n' "$HOME/.bashrc" ;;
    fish) printf '%s\n' "$HOME/.config/fish/config.fish" ;;
    *) printf '%s\n' "$HOME/.profile" ;;
  esac
}

path_line() {
  shell_name="$(basename "${SHELL:-}")"
  case "$shell_name" in
    fish) printf 'fish_add_path "%s"\n' "$INSTALL_DIR" ;;
    *) printf 'export PATH="%s:$PATH"\n' "$INSTALL_DIR" ;;
  esac
}

source_command() {
  profile="$1"
  printf 'source "%s"\n' "$profile"
}

print_manual_path_steps() {
  profile="$1"
  line="$2"
  reload="$3"
  profile_dir="$(dirname "$profile")"

  if [ "$profile_dir" != "$HOME" ]; then
    printf '  mkdir -p "%s"\n' "$profile_dir"
  fi
  printf "  echo '%s' >> \"%s\"\n" "$line" "$profile"
  printf '  %s\n' "$reload"
}

add_to_path_prompt() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac

  profile="$(shell_profile)"
  line="$(path_line)"
  reload="$(source_command "$profile")"

  if [ -t 1 ] && ( : >/dev/tty ) 2>/dev/null; then
    {
      printf '\nMake Dairo easier to run?\n\n'
      printf 'This lets you type `dairo` anywhere in Terminal instead of using:\n'
      printf '  %s/dairo\n\n' "$INSTALL_DIR"
      printf 'Add Dairo to your terminal path? [y/N] '
    } >/dev/tty

    IFS= read -r answer </dev/tty || answer=""
    case "$answer" in
      y|Y|yes|YES|Yes)
        mkdir -p "$(dirname "$profile")"
        if [ -f "$profile" ] && grep -F "$line" "$profile" >/dev/null 2>&1; then
          printf 'Dairo is already listed in %s.\n' "$profile"
        else
          printf '\n%s' "$line" >> "$profile"
          printf 'Added Dairo to %s.\n' "$profile"
        fi
        printf 'To use `dairo` in this terminal, run:\n  %s\n' "$reload"
        printf 'Or open a new terminal.\n'
        ;;
      *)
        printf 'No problem. You can add Dairo later with:\n'
        print_manual_path_steps "$profile" "$line" "$reload"
        ;;
    esac
  else
    printf 'Add Dairo to your terminal later with:\n'
    print_manual_path_steps "$profile" "$line" "$reload"
  fi
}

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

# musl systems (Alpine, busybox-based containers) and distros whose glibc is
# older than the 2.35 the gnu binaries are built against both get the static
# musl build, which runs on any Linux.
linux_flavor() {
  if [ -e /lib/ld-musl-x86_64.so.1 ] || [ -e /lib/ld-musl-aarch64.so.1 ]; then
    printf 'musl'
    return
  fi
  glibc_version="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')"
  if [ -z "$glibc_version" ]; then
    printf 'musl'
    return
  fi
  glibc_major="${glibc_version%%.*}"
  glibc_rest="${glibc_version#*.}"
  glibc_minor="${glibc_rest%%.*}"
  if [ "$glibc_major" -gt 2 ] 2>/dev/null || { [ "$glibc_major" -eq 2 ] && [ "$glibc_minor" -ge 35 ]; } 2>/dev/null; then
    printf 'gnu'
  else
    printf 'musl'
  fi
}

case "$platform-$cpu" in
  darwin-arm64) target="aarch64-apple-darwin" ;;
  darwin-x64) target="x86_64-apple-darwin" ;;
  linux-arm64) target="aarch64-unknown-linux-$(linux_flavor)" ;;
  linux-x64) target="x86_64-unknown-linux-$(linux_flavor)" ;;
  *) echo "Dairo CLI is not available for $platform-$cpu" >&2; exit 1 ;;
esac

asset="dairo-$target.tar.gz"
url="$BASE_URL/$VERSION/$asset"
checksums_url="$BASE_URL/$VERSION/checksums.txt"

# Minimal container images (e.g. amazonlinux:2023) ship without tar/gzip.
for tool in tar gzip; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "$tool is required to install Dairo CLI (e.g. dnf install -y tar gzip, or apt-get install -y $tool)" >&2
    exit 1
  fi
done

tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT

echo "Downloading Dairo CLI $VERSION for $target..."
if command -v curl >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 --retry 3 -fsSL "$url" -o "$tmp/$asset"
  curl --proto '=https' --tlsv1.2 --retry 3 -fsSL "$checksums_url" -o "$tmp/checksums.txt"
elif command -v wget >/dev/null 2>&1; then
  # busybox wget (Alpine base images) does not support the GNU wget flags.
  if wget --help 2>&1 | grep -q -- --https-only; then
    wget --https-only --tries=3 -qO "$tmp/$asset" "$url"
    wget --https-only --tries=3 -qO "$tmp/checksums.txt" "$checksums_url"
  else
    wget -qO "$tmp/$asset" "$url"
    wget -qO "$tmp/checksums.txt" "$checksums_url"
  fi
else
  echo "curl or wget is required" >&2
  exit 1
fi

expected="$(awk -v file="$asset" '$2 == file { print $1 }' "$tmp/checksums.txt")"
if [ -z "$expected" ]; then
  echo "Could not find checksum for $asset" >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp/$asset" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')"
else
  echo "sha256sum or shasum is required to verify the download" >&2
  exit 1
fi
if [ "$actual" != "$expected" ]; then
  echo "Checksum mismatch for $asset" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"

# Prove the downloaded binary runs before touching any existing install.
if ! "$tmp/dairo" --version >/dev/null 2>&1; then
  echo "Downloaded dairo binary failed to run on this system; leaving any existing install untouched." >&2
  exit 1
fi

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/dairo" "$INSTALL_DIR/dairo"

echo "Dairo CLI installed to $INSTALL_DIR/dairo (checksum verified)"
add_to_path_prompt
"$INSTALL_DIR/dairo" --version
