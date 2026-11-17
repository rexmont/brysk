#!/bin/sh
# Builds Brysk.app for macOS: a release build, the two binaries, the Info.plist, and an
# ad-hoc signature (replace `-` with a Developer ID to ship it).
#
#   scripts/build-macos.sh            # target/Brysk.app
#   CODESIGN_ID="Developer ID Application: …" scripts/build-macos.sh
#
# An ad-hoc build runs from anywhere, but Gatekeeper will not accept a downloaded copy; a
# notarised build is the packaging milestone that follows.
set -eu
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-release}"
APP="target/Brysk.app"
BIN="target/$PROFILE"
UNIVERSAL="${UNIVERSAL:-0}"

if [ "$UNIVERSAL" = "1" ]; then
    # One .app for Apple silicon and Intel. Rust needs the second target once:
    #   rustup target add x86_64-apple-darwin
    for target in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --profile "$PROFILE" --target "$target" -p rf-app --bin brysk --bin directlook
    done
    mkdir -p "$BIN"
    lipo -create -output "$BIN/brysk" "target/aarch64-apple-darwin/$PROFILE/brysk" "target/x86_64-apple-darwin/$PROFILE/brysk"
    lipo -create -output "$BIN/directlook" "target/aarch64-apple-darwin/$PROFILE/directlook" "target/x86_64-apple-darwin/$PROFILE/directlook"
else
    cargo build --profile "$PROFILE" -p rf-app --bin brysk --bin directlook
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
cp "$BIN/brysk" "$APP/Contents/MacOS/brysk"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# DirectLook is an app of its own (a nested agent bundle): run as a bare binary inside
# Brysk.app, LaunchServices registers it as Brysk and `open Brysk.app` then
# activates the invisible helper instead of starting the file manager.
HELPER="$APP/Contents/Helpers/DirectLook.app"
mkdir -p "$HELPER/Contents/MacOS"
cp packaging/macos/DirectLook-Info.plist "$HELPER/Contents/Info.plist"
cp "$BIN/directlook" "$HELPER/Contents/MacOS/directlook"

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