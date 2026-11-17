#!/bin/sh
# Rebuilds packaging/macos/Brysk.icns from packaging/macos/icon.png (1024 px source, rounded
# corners already applied). Run after replacing the icon.
set -eu
cd "$(dirname "$0")/.."
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$TMP/Brysk.iconset"
sips -z 1024 1024 packaging/macos/icon.png --out "$TMP/icon-1024.png" >/dev/null
for spec in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" "64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" "256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" "1024 icon_512x512@2x"; do
    set -- $spec
    sips -z "$1" "$1" "$TMP/icon-1024.png" --out "$TMP/Brysk.iconset/$2.png" >/dev/null
done
iconutil -c icns "$TMP/Brysk.iconset" -o packaging/macos/Brysk.icns
echo "packaging/macos/Brysk.icns"
