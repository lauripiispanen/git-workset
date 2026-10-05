#!/bin/sh
# Run the test suite against the git versions shipped by older distributions,
# in Docker. CI runs the same images (.github/workflows/ci.yml).
#
#   scripts/test-old-git.sh                # all images
#   scripts/test-old-git.sh debian:12      # one image
#   scripts/test-old-git.sh debian:12 test_apply   # filter tests
set -eu

images="${1:-ubuntu:22.04 debian:12}"
filter="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"

for image in $images; do
  echo "==> $image"
  vol="git-workset-oldgit-$(echo "$image" | tr ':/.' '---')"
  docker run --rm \
    -v "$root:/src:ro" \
    -v "$vol:/cache" \
    -e CARGO_HOME=/cache/cargo -e RUSTUP_HOME=/cache/rustup \
    -e CARGO_TARGET_DIR=/cache/target \
    -w /src "$image" sh -ec '
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null
      apt-get install -y -qq git curl ca-certificates build-essential >/dev/null
      [ -x /cache/cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs \
        | sh -s -- -y -q --profile minimal --no-modify-path >/dev/null
      export PATH=/cache/cargo/bin:$PATH
      git --version
      cargo test -q -- '"$filter"'
    '
done
