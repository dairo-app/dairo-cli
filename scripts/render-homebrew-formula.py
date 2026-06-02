#!/usr/bin/env python3
import sys
from pathlib import Path

if len(sys.argv) != 4:
    raise SystemExit("usage: render-homebrew-formula.py <version> <tag> <checksums.txt>")

version, tag, checksums_path = sys.argv[1:]
checksums = {}
for line in Path(checksums_path).read_text().splitlines():
    parts = line.split()
    if len(parts) >= 2:
        checksums[parts[1]] = parts[0]

base = f"https://github.com/dairo-app/dairo-cli/releases/download/{tag}"

def block(asset: str, bin_name: str = "dairo") -> str:
    sha = checksums.get(asset)
    if not sha:
        raise SystemExit(f"missing checksum for {asset}")
    return f'''      url "{base}/{asset}"
      sha256 "{sha}"

      def install
        bin.install "{bin_name}" => "dairo"
      end'''

print(f'''class Dairo < Formula
  desc "Official Dairo command-line interface"
  homepage "https://dairo.app"
  version "{version}"
  license "MIT"

  on_macos do
    on_arm do
{block('dairo-aarch64-apple-darwin.tar.gz')}
    end

    on_intel do
{block('dairo-x86_64-apple-darwin.tar.gz')}
    end
  end

  on_linux do
    on_arm do
{block('dairo-aarch64-unknown-linux-gnu.tar.gz')}
    end

    on_intel do
{block('dairo-x86_64-unknown-linux-gnu.tar.gz')}
    end
  end

  test do
    assert_match version.to_s, shell_output("#{{bin}}/dairo --version")
  end
end
''')
