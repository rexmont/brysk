#!/bin/sh
# Builds rustyfiles.app for macOS: a release build, the two binaries, the Info.plist, and an
# ad-hoc signature (replace `-` with a Developer ID to ship it).
#
#   scripts/build-macos.sh            # target/rustyfiles.app
#   CODESIGN_ID="Developer ID Application: …" scripts/build-macos.sh
#
# An ad-hoc build runs from anywhere, but Gatekeeper will not accept a downloaded copy; a
# notarised build is the packaging milestone that follows.
set -eu
cd "$(dirname "$0")/.."

PROFILE="${PROFILE:-release}"
APP="target/rustyfiles.app"
BIN="target/$PROFILE"

cargo build --profile "$PROFILE" -p rf-app --bin rustyfiles --bin directlook

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp packaging/macos/Info.plist "$APP/Contents/Info.plist"
cp "$BIN/rustyfiles" "$APP/Contents/MacOS/rustyfiles"
cp "$BIN/directlook" "$APP/Contents/MacOS/directlook"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# The icon is optional: `packaging/macos/rustyfiles.icns` when it exists.
if [ -f packaging/macos/rustyfiles.icns ]; then
    cp packaging/macos/rustyfiles.icns "$APP/Contents/Resources/rustyfiles.icns"
    /usr/libexec/PlistBuddy -c "Add :CFBundleIconFile string rustyfiles" "$APP/Contents/Info.plist"
fi

# Mach-O binaries only; the ad-hoc identity is enough for local use.
codesign --force --deep --sign "${CODESIGN_ID:--}" "$APP/Contents/MacOS/rustyfiles" 2>/dev/null || true
codesign --force --deep --sign "${CODESIGN_ID:--}" "$APP/Contents/MacOS/directlook" 2>/dev/null || true
codesign --force --deep --sign "${CODESIGN_ID:--}" "$APP"

echo "built $APP"
echo "run: open $APP --args /Users/you"