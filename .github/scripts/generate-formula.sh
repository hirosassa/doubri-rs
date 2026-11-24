#!/usr/bin/env bash
# Usage: generate-formula.sh <tag> <output-path>
# Generates the Homebrew formula for an existing release by reading the
# per-target `.sha256` assets that taiki-e/upload-rust-binary-action uploads.
set -euo pipefail

tag="$1"
out="$2"
version="${tag#v}"
repo="${GITHUB_REPOSITORY:-hirosassa/doubri-rs}"
base="https://github.com/${repo}/releases/download/${tag}"

checksums="$(mktemp -d)"
trap 'rm -rf "$checksums"' EXIT
gh release download "$tag" --repo "$repo" --pattern '*.sha256' --dir "$checksums"

# Each file holds "<hash>  doubri-<target>.tar.xz". Resolve every hash before
# writing the formula: an `exit` inside `$(...)` in the heredoc would not abort
# the script and would leave a formula with empty checksums.
sha() {
  local hash
  hash="$(cut -d' ' -f1 "${checksums}/doubri-$1.sha256")"
  [[ "$hash" =~ ^[0-9a-f]{64}$ ]] || { echo "invalid sha256 for $1: '$hash'" >&2; return 1; }
  printf '%s' "$hash"
}
sha_arm_macos="$(sha aarch64-apple-darwin)"
sha_intel_macos="$(sha x86_64-apple-darwin)"
sha_arm_linux="$(sha aarch64-unknown-linux-gnu)"
sha_intel_linux="$(sha x86_64-unknown-linux-gnu)"

cat > "$out" <<RUBY
class Doubri < Formula
  desc "A Rust implementation of doubri, a large-scale document deduplication toolkit"
  homepage "https://github.com/hirosassa/doubri-rs"
  version "${version}"
  license "MIT"

  on_macos do
    on_arm do
      url "${base}/doubri-aarch64-apple-darwin.tar.xz"
      sha256 "${sha_arm_macos}"
    end
    on_intel do
      url "${base}/doubri-x86_64-apple-darwin.tar.xz"
      sha256 "${sha_intel_macos}"
    end
  end

  on_linux do
    on_arm do
      url "${base}/doubri-aarch64-unknown-linux-gnu.tar.xz"
      sha256 "${sha_arm_linux}"
    end
    on_intel do
      url "${base}/doubri-x86_64-unknown-linux-gnu.tar.xz"
      sha256 "${sha_intel_linux}"
    end
  end

  def install
    bin.install "doubri"
  end
end
RUBY
