#!/bin/sh
# Pinned upstream binary distribution. Never execute an unchecked download.
set -eu
cd "$(dirname "$0")/.."
ARCHIVE=target/sparkle/Sparkle-2.10.1.tar.xz
HASH=1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27
mkdir -p target/sparkle
if [ ! -f "$ARCHIVE" ]; then
    curl --fail --location --retry 2 https://github.com/sparkle-project/Sparkle/releases/download/2.10.1/Sparkle-2.10.1.tar.xz -o "$ARCHIVE.tmp"
    mv "$ARCHIVE.tmp" "$ARCHIVE"
fi
test "$(shasum -a 256 "$ARCHIVE" | cut -d ' ' -f 1)" = "$HASH"
if [ ! -f target/sparkle/Sparkle.framework/Sparkle ] || [ ! -x target/sparkle/bin/sign_update ]; then
    tar -xJf "$ARCHIVE" -C target/sparkle
fi
