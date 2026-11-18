#!/bin/sh
# Builds Brysk.app for macOS: the two binaries, the Info.plist, and an ad-hoc signature
# (replace `-` with a Developer ID to ship it). A debug build by default; releases go
# through scripts/package-macos.sh.
#
#   scripts/build-macos.sh            # debug → target/Brysk.app
#   PROFILE=release scripts/build-macos.sh
#   CODESIGN_ID="Developer ID Application: …" scripts/build-macos.sh
#
# An ad-hoc build runs from anywhere, but Gatekeeper will not accept a downloaded copy; a
# notarised build is the packaging milestone that follows.
set -eu
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-dev}"
case "$PROFILE" in
    dev|debug) CARGO_PROFILE=dev; PROFILE_DIR=debug ;;
    release) CARGO_PROFILE=release; PROFILE_DIR=release ;;
    *) CARGO_PROFILE="$PROFILE"; PROFILE_DIR="$PROFILE" ;;
esac
APP="target/Brysk.app"
BIN="target/$PROFILE_DIR"
UNIVERSAL="${UNIVERSAL:-0}"

if [ "$UNIVERSAL" = "1" ]; then
    # One .app for Apple silicon and Intel. Rust needs the second target once:
    #   rustup target add x86_64-apple-darwin
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --profile "$CARGO_PROFILE" --target "$target" -p rf-app --bin brysk --bin directlook
    done
    mkdir -p "$BIN"
    lipo -create -output "$BIN/brysk" "target/aarch64-apple-darwin/$PROFILE_DIR/brysk" "target/x86_64-apple-darwin/$PROFILE_DIR/brysk"
    lipo -create -output "$BIN/directlook" "target/aarch64-apple-darwin/$PROFILE_DIR/directlook" "target/x86_64-apple-darwin/$PROFILE_DIR/directlook"
else
    cargo build --profile "$CARGO_PROFILE" -p rf-app --bin brysk --bin directlook
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# MIT: the copyright and permission notice travel with the binary.
cp LICENSE NOTICE "$APP/Contents/Resources/"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
cp "$BIN/brysk" "$APP/Contents/MacOS/brysk"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# DirectLook is an app of its own (a nested agent bundle): run as a bare binary inside
# Brysk.app, LaunchServices registers it as Brysk and `open Brysk.app` then
# activates the invisible helper instead of starting the file manager.
HELPER="$APP/Contents/Helpers/DirectLook.app"
mkdir -p "$HELPER/Contents/MacOS" "$HELPER/Contents/Resources"
cp packaging/macos/DirectLook-Info.plist "$HELPER/Contents/Info.plist"
cp "$BIN/directlook" "$HELPER/Contents/MacOS/directlook"
# The nested bundle is distributed as its own app, so it carries its own notice too.
cp LICENSE NOTICE "$HELPER/Contents/Resources/"

# The icon is optional: `packaging/macos/Brysk.icns` when it exists.
if [ -f packaging/macos/Brysk.icns ]; then
    cp packaging/macos/Brysk.icns "$APP/Contents/Resources/Brysk.icns"
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string Brysk" "$APP/Contents/Info.plist"
fi

# Mach-O binaries only; the ad-hoc identity is enough for local use.
if [ -n "${CODESIGN_ID:-}" ]; then
    # A Developer ID build: hardened runtime, which notarisation requires.
    SIGN_FLAGS="--force --options runtime --timestamp"
else
    SIGN_FLAGS="--force"
fi
# shellcheck disable=SC2086
codesign $SIGN_FLAGS --sign "${CODESIGN_ID:--}" "$APP/Contents/Helpers/DirectLook.app"
# shellcheck disable=SC2086
codesign $SIGN_FLAGS --sign "${CODESIGN_ID:--}" "$APP"

echo "built $APP"
echo "run: open $APP --args /Users/you"