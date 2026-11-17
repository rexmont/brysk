#!/bin/sh
# Fetches the original project, fast-forwards main, and merges it into the macOS port branch.
#
#   scripts/sync-upstream.sh
#
# Run it on a clean tree. It stops at the first conflict; resolve in the usual way, then
# `cargo build` and `cargo test -p rf-mac`. See docs/upstream.md.
set -eu
cd "$(dirname "$0")/.."

if [ -n "$(git status --porcelain)" ]; then
    echo "working tree is not clean" >&2
    exit 1
fi

current=$(git rev-parse --abbrev-ref HEAD)
git fetch upstream --prune
git switch main
git merge --ff-only upstream/main
git push origin main
git switch macos
if ! git merge --no-edit main; then
    echo
    echo "conflicts above. docs/upstream.md says what each kind means."
    echo "when done: git add -A && git commit && cargo build && cargo test -p rf-mac"
    exit 1
fi
echo "upstream merged into macos; run: cargo build && cargo test -p rf-mac"
