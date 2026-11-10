#!/usr/bin/env bash
# Create the Finder drag-to-Applications disk image from a signed Lizard.app.
# Usage: packaging/macos/make-dmg.sh /path/to/Lizard.app /path/to/output.dmg
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 Lizard.app output.dmg" >&2
  exit 2
fi
if ! command -v create-dmg >/dev/null 2>&1; then
  echo "error: create-dmg is required (brew install create-dmg)" >&2
  exit 1
fi

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="$1"
DMG="$2"
if [ ! -d "$APP" ] || [ ! -f "$APP/Contents/Resources/Lizard.icns" ]; then
  echo "error: expected a packaged Lizard.app with its icon: $APP" >&2
  exit 1
fi

STAGE="$(mktemp -d "${TMPDIR:-/tmp}/lizard-dmg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
ditto "$APP" "$STAGE/Lizard.app"

# Finder uses the background image's pixel dimensions as point dimensions, even on
# Retina displays, so the art is 720x440 pixels for the 720x440-point window.
# create-dmg writes the Finder layout and compresses with hdiutil; it does not use makehybrid,
# which would add FinderInfo to signed files inside the app bundle.
create-dmg \
  --volname Lizard \
  --volicon "$APP/Contents/Resources/Lizard.icns" \
  --background "$HERE/dmg-background.png" \
  --window-size 720 440 \
  --icon-size 120 \
  --text-size 12 \
  --icon Lizard.app 184 222 \
  --hide-extension Lizard.app \
  --app-drop-link 536 222 \
  --filesystem HFS+ \
  --format UDZO \
  --no-internet-enable \
  --overwrite \
  "$DMG" "$STAGE"
