#!/bin/sh
# Sign nested code from the inside out, keeping Sparkle's own helper entitlements.
set -eu
FRAMEWORK="$1/Contents/Frameworks/Sparkle.framework"
IDENTITY="${CODESIGN_ID:--}"
for CODE in Versions/B/XPCServices/Downloader.xpc Versions/B/XPCServices/Installer.xpc Versions/B/Autoupdate Versions/B/Updater.app .; do
    if [ "$IDENTITY" = - ]; then
        codesign --force --sign - --preserve-metadata=entitlements "$FRAMEWORK/$CODE"
    else
        codesign --force --options runtime --timestamp --sign "$IDENTITY" --preserve-metadata=entitlements "$FRAMEWORK/$CODE"
    fi
done
