#!/bin/sh
# Rebuilds packaging/macos/rustyfiles.icns from the project's SVG (a 1024 px render, then the
# sizes iconutil needs). Run after changing the logo.
set -eu
cd "$(dirname "$0")/.."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cargo run -q -p rf-mac --example svg2png crates/rf-app/assets/rustyfiles.svg "$TMP/icon-1024.png" 1024
mkdir -p "$TMP/rustyfiles.iconset"
for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" "64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
    set -- $spec
    sips -z "$1" "$1" "$TMP/icon-1024.png" --out "$TMP/rustyfiles.iconset/$2.png" >/dev/null
done
iconutil -c icns "$TMP/rustyfiles.iconset" -o packaging/macos/rustyfiles.icns
echo "packaging/macos/rustyfiles.icns"
