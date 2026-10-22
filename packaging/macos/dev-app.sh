#!/usr/bin/env bash
# Build and open a local Lizard.app with the committed macOS icon.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
. "$ROOT/packaging/env.sh"

APP="$ROOT/target/Lizard.app"
ICONSET="$ROOT/target/icon/Lizard.iconset"
ICONS="$ROOT/assets/app-icon"

(cd "$ROOT" && cargo build -p figcraft --bin lizard)
mkdir -p "$ICONSET" "$APP/Contents/MacOS" "$APP/Contents/Resources"
for size in 16 32 128 256 512; do
  cp "$ICONS/figcraft-$size.png" "$ICONSET/icon_${size}x${size}.png"
  cp "$ICONS/figcraft-$((size * 2)).png" "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns -o "$APP/Contents/Resources/Lizard.icns" "$ICONSET"
cp "$ROOT/target/debug/lizard" "$APP/Contents/MacOS/Lizard"
SHORT_VERSION="${VERSION%%-*}"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${FIGCRAFT_BUILD_SHA:-unknown}/g" \
  "$ROOT/packaging/macos/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
codesign --force --sign - "$APP"
open -n "$APP"
echo "Opened $APP"
