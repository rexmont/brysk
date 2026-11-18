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
if [ "$PROFILE_DIR" != debug ]; then
    APP="target/$PROFILE_DIR/Brysk.app"
fi
BIN="target/$PROFILE_DIR"
UNIVERSAL="${UNIVERSAL:-0}"
./scripts/prepare-sparkle.sh

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
cp packaging/macos/Sparkle-LICENSE "$APP/Contents/Resources/"
mkdir -p "$APP/Contents/Frameworks"
ditto target/sparkle/Sparkle.framework "$APP/Contents/Frameworks/Sparkle.framework"
./scripts/sign-sparkle.sh "$APP"
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
    for BUNDLE in "$APP" "$HELPER"; do
        cp packaging/macos/Brysk.icns "$BUNDLE/Contents/Resources/Brysk.icns"
        /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string Brysk" "$BUNDLE/Contents/Info.plist"
    done
fi

# A native background layer avoids the legacy icon's inset tile in modern macOS lists.
# Keep the existing ICNS for older systems and builds with only Command Line Tools.
if [ -d packaging/macos/Brysk.icon ] && xcrun --find actool >/dev/null 2>&1 &&
    xcrun actool --version --output-format human-readable-text | awk '/^short-bundle-version:/ { found = ($2 + 0 >= 26) } END { exit !found }'; then
    ICON_BUILD=$(mktemp -d)
    trap 'rm -rf "$ICON_BUILD"' EXIT
    xcrun actool packaging/macos/Brysk.icon --compile "$ICON_BUILD" --platform macosx \
        --minimum-deployment-target 13.0 --app-icon Brysk \
        --output-partial-info-plist "$ICON_BUILD/icon-info.plist" --output-format human-readable-text
    for BUNDLE in "$APP" "$HELPER"; do
        cp "$ICON_BUILD/Assets.car" "$BUNDLE/Contents/Resources/Assets.car"
        /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string Brysk" "$BUNDLE/Contents/Info.plist"
    done
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
