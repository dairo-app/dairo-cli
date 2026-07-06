#!/usr/bin/env sh
set -eu

REPO="dairo-app/dairo-cli"
VERSION="${DAIRO_CLI_VERSION:-latest}"
INSTALL_DIR="${DAIRO_INSTALL_DIR:-$HOME/.dairo/bin}"
BASE_URL="${DAIRO_DOWNLOAD_BASE_URL:-https://dairo.app/downloads/cli}"

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

add_to_path_prompt() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac

  profile="$(shell_profile)"
  line="$(path_line)"
  reload="$(source_command "$profile")"
  manual_cmd="mkdir -p \"$(dirname "$profile")\" && printf '\\n%s' '$line' >> \"$profile\" && $reload"

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
        printf 'No problem. You can add Dairo later with:\n  %s\n' "$manual_cmd"
        ;;
    esac
  else
    printf 'Add Dairo to your terminal later with:\n  %s\n' "$manual_cmd"
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
add_to_path_prompt
"$INSTALL_DIR/dairo" --version
