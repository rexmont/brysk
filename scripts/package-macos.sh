#!/bin/bash
# Builds, signs (Developer ID, hardened runtime), packages a DMG, notarises it and staples it.
#
#   BRYSK_DEVELOPER_ID='Developer ID Application: Name (TEAMID)' \
#   BRYSK_NOTARY_PROFILE='dioramac-notary' \
#   scripts/package-macos.sh
#
# The notary profile is a `xcrun notarytool store-credentials <name>` keychain entry. Set
# UNIVERSAL=0 for an Apple-silicon-only build (the default is universal, arm64 + x86_64).
set -euo pipefail
cd "$(dirname "$0")/.."

: "${BRYSK_DEVELOPER_ID:?Set this to your Developer ID Application identity.}"
: "${BRYSK_NOTARY_PROFILE:?Set this to your notarytool Keychain profile.}"
IDENTITY="$BRYSK_DEVELOPER_ID"
if [[ "$IDENTITY" != "Developer ID Application:"* ]]; then
    echo "BRYSK_DEVELOPER_ID must be the full Developer ID Application identity." >&2
    exit 2
fi
if ! security find-identity -v -p codesigning | grep -F "\"$IDENTITY\"" >/dev/null; then
    echo "The Developer ID Application identity and its private key must be installed." >&2
    exit 2
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
UNIVERSAL="${UNIVERSAL:-1}"
APP="target/release/Brysk.app"
DMG="dist/Brysk-$VERSION.dmg"
mkdir -p dist

# Build with the usual script (ad-hoc first; the signatures below replace it).
if [[ "$UNIVERSAL" = "1" ]]; then
    UNIVERSAL=1 PROFILE=release ./scripts/build-macos.sh
else
    PROFILE=release ./scripts/build-macos.sh
fi

# The licence and attribution must be sealed into both signed bundles. Copy them again here
# so a stale or hand-built bundle cannot produce a release without them.
for bundle in "$APP" "$APP/Contents/Helpers/DirectLook.app"; do
    mkdir -p "$bundle/Contents/Resources"
    cp LICENSE NOTICE "$bundle/Contents/Resources/"
    test -f "$bundle/Contents/Resources/LICENSE"
    test -f "$bundle/Contents/Resources/NOTICE"
done

# Sign the nested helper first, then the app. `--deep` is only for verification: signing it
# that way leaves the helper with the parent's identity requirements.
codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP/Contents/Helpers/DirectLook.app"
codesign --force --options runtime --timestamp --sign "$IDENTITY" --identifier com.rexmont.brysk.finder-agent "$APP/Contents/MacOS/brysk-finder-agent"
CODESIGN_ID="$IDENTITY" ./scripts/sign-sparkle.sh "$APP"
codesign --force --options runtime --timestamp --sign "$IDENTITY" \
    --entitlements packaging/macos/Brysk.entitlements "$APP"
codesign --verify --deep --strict "$APP"
echo "signed: $(codesign -dvv "$APP" 2>&1 | grep '^Authority=' | head -1)"

STAGING="$(mktemp -d "$PWD/dist/.package.XXXXXXXX")"
trap 'rm -rf "$STAGING"' EXIT
mkdir "$STAGING/image"
ditto "$APP" "$STAGING/image/Brysk.app"
ln -s /Applications "$STAGING/image/Applications"
# hdiutil's automatic estimate can underallocate the temporary filesystem for universal bundles.
IMAGE_KB="$(du -sk "$STAGING/image" | awk '{print int($1 * 1.2 + 32768)}')"
hdiutil create -quiet -size "${IMAGE_KB}k" -volname Brysk -srcfolder "$STAGING/image" -format UDZO "$STAGING/Brysk.dmg"
codesign --force --timestamp --sign "$IDENTITY" "$STAGING/Brysk.dmg"

SUBMISSION="dist/Brysk-$VERSION.submission.json"
if ! xcrun notarytool submit "$STAGING/Brysk.dmg" --keychain-profile "$BRYSK_NOTARY_PROFILE" --wait \
        --output-format json > "$SUBMISSION"; then
    cat "$SUBMISSION" >&2
    exit 1
fi
REQUEST="$(plutil -extract id raw -o - "$SUBMISSION")"
LOG="dist/Brysk-$VERSION.notarization.json"
xcrun notarytool log "$REQUEST" --keychain-profile "$BRYSK_NOTARY_PROFILE" "$LOG"
if [[ "$(plutil -extract status raw -o - "$SUBMISSION")" != Accepted ]]; then
    echo "Notarization failed; see $LOG" >&2
    exit 1
fi
xcrun stapler staple "$STAGING/Brysk.dmg"
xcrun stapler validate "$STAGING/Brysk.dmg"
spctl -a -vv -t open --context context:primary-signature "$STAGING/Brysk.dmg"
hdiutil verify -quiet "$STAGING/Brysk.dmg"
mv -f "$STAGING/Brysk.dmg" "$DMG"
(cd dist && shasum -a 256 "Brysk-$VERSION.dmg") | tee "$DMG.sha256"
python3 scripts/make-macos-appcast.py "$DMG" "$APP" dist/appcast.xml
echo "$DMG"
