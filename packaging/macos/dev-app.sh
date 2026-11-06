#!/usr/bin/env bash
# Build and open a local Lizard.app with the committed macOS icon.
# Pass application arguments after the script name, for example --control 7979.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
. "$ROOT/packaging/env.sh"

APP="$ROOT/target/Lizard.app"
ICONSET="$ROOT/target/icon/Lizard.iconset"
ICONS="$ROOT/assets/app-icon"

(cd "$ROOT" && cargo build -p lizard -p lizard-cli --bins)
mkdir -p "$ICONSET" "$APP/Contents/MacOS" "$APP/Contents/Resources"
for size in 16 32 128 256 512; do
  cp "$ICONS/lizard-$size.png" "$ICONSET/icon_${size}x${size}.png"
  cp "$ICONS/lizard-$((size * 2)).png" "$ICONSET/icon_${size}x${size}@2x.png"
done
iconutil -c icns -o "$APP/Contents/Resources/Lizard.icns" "$ICONSET"
rm -f "$APP/Contents/Resources/Assets.car"
cp "$ROOT/target/debug/lizard" "$APP/Contents/MacOS/Lizard"
cp "$ROOT/target/debug/lizard-cli" "$APP/Contents/MacOS/lizard-cli"
SHORT_VERSION="${VERSION%%-*}"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${LIZARD_BUILD_SHA:-unknown}/g" \
  "$ROOT/packaging/macos/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
codesign --force --sign - "$APP/Contents/MacOS/lizard-cli"
codesign --force --sign - "$APP"
open -n "$APP" --args "$@"
echo "Opened $APP"
