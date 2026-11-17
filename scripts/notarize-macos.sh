#!/bin/sh
# Signs with a Developer ID and notarises a built rustyfiles.app.
#
#   CODESIGN_ID="Developer ID Application: Your Name (TEAMID)" \
#   APPLE_ID="you@example.com" TEAM_ID="TEAMID" APP_PASSWORD="xxxx-xxxx-xxxx-xxxx" \
#   scripts/notarize-macos.sh
#
# APP_PASSWORD is an app-specific password made at appleid.apple.com (not the account
# password). For key-based auth use `xcrun notarytool` directly with --key/--key-id/--issuer.
#
# The build must exist first: `UNIVERSAL=1 scripts/build-macos.sh` (with CODESIGN_ID set it
# signs with the hardened runtime).
set -eu
cd "$(dirname "$0")/.."

APP="target/rustyfiles.app"
ZIP="target/rustyfiles-notarize.zip"
: "${CODESIGN_ID:?set CODESIGN_ID to a Developer ID Application identity}"
: "${APPLE_ID:?set APPLE_ID}"
: "${TEAM_ID:?set TEAM_ID}"
: "${APP_PASSWORD:?set APP_PASSWORD (app-specific password)}"

codesign --verify --deep --strict --verbose=2 "$APP"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

xcrun notarytool submit "$ZIP" --apple-id "$APPLE_ID" --team-id "$TEAM_ID" --password "$APP_PASSWORD" --wait
xcrun stapler staple "$APP"
spctl -a -vv "$APP" || true
echo "notarised and stapled: $APP"
