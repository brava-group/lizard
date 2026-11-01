#!/usr/bin/env bash
# Build, sign and (optionally) notarize the macOS release artifacts:
#
#   $DIST/lizard-<version>-macos-<arch>.dmg            Lizard.app on a drag-to-Applications DMG
#   $DIST/lizard-cli-<version>-macos-<arch>.zip      the headless CLI (when apps/lizard-cli exists)
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build]
#
# Signing (env):
#   MACOS_SIGN_IDENTITY   codesign identity (name or SHA-1). Default "-" = ad-hoc (local testing;
#                         Gatekeeper will reject the result on other Macs). In CI, import-cert.sh sets it.
#   MACOS_KEYCHAIN        keychain holding the identity (optional)
# Notarization (env; a real Developer ID identity and one credential method):
#   MACOS_NOTARY_PROFILE  credentials stored in Keychain with `xcrun notarytool store-credentials`
#   or APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

ARCH=universal
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/Lizard.app"
DMG="$DIST/lizard-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/lizard-cli-$VERSION-macos-$ARCH.zip"
# The agent connection flow installs official provider helpers on first use. Bundle a pinned
# Node.js/npm runtime so users of the signed app do not need a terminal or a system Node install.
NODE_VERSION=22.23.4
BINS=(lizard)
[ "$HAS_CLI" = 1 ] && BINS+=(lizard-cli)

NOTARIZE=0
NOTARY_ARGS=()
SIGNING_IDENTITY_RECORD=""
if [ "$IDENTITY" != "-" ]; then
  SIGNING_IDENTITY_RECORD="$(security find-identity -v -p codesigning ${MACOS_KEYCHAIN:+"$MACOS_KEYCHAIN"} \
    | grep -F 'Developer ID Application:' | grep -F "$IDENTITY" | head -n 1 || true)"
fi
if [ "$IDENTITY" = "-" ]; then
  warn "macOS: ad-hoc signing (no MACOS_SIGN_IDENTITY); artifacts are not notarized"
elif [ -z "$SIGNING_IDENTITY_RECORD" ]; then
  echo "error: MACOS_SIGN_IDENTITY must identify an installed Developer ID Application certificate" >&2
  exit 1
elif [ -n "${APPLE_TEAM_ID:-}" ] && [[ "$SIGNING_IDENTITY_RECORD" != *"($APPLE_TEAM_ID)"* ]]; then
  echo "error: Developer ID Application identity does not belong to APPLE_TEAM_ID" >&2
  exit 1
elif [ -n "${MACOS_NOTARY_PROFILE:-}" ]; then
  NOTARY_ARGS=(--keychain-profile "$MACOS_NOTARY_PROFILE")
  NOTARIZE=1
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; then
  NOTARY_ARGS=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
  NOTARIZE=1
else
  warn "macOS: no MACOS_NOTARY_PROFILE or complete Apple ID credentials; signed but not notarized"
fi

echo "==> Lizard $VERSION for macOS ($ARCH; ${BINS[*]}), identity: $IDENTITY, notarize: $NOTARIZE"

# ---- build -------------------------------------------------------------------------------------
if [ "$SKIP_BUILD" = 0 ]; then
  for t in "${TARGETS[@]}"; do
    if [ "$HAS_CLI" = 1 ]; then
      (cd "$ROOT" && cargo build --release --locked -p lizard -p lizard-cli --bins --target "$t")
    else
      (cd "$ROOT" && cargo build --release --locked -p lizard --bin lizard --target "$t")
    fi
  done
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in "${BINS[@]}"; do
  inputs=()
  for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

# ---- signing helpers ---------------------------------------------------------------------------
sign() {
  # Hardened runtime + secure timestamp for a real identity; ad-hoc can't be timestamped.
  local ts=(--timestamp)
  [ "$IDENTITY" = "-" ] && ts=(--timestamp=none)
  local kc=()
  [ -n "${MACOS_KEYCHAIN:-}" ] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" "${NOTARY_ARGS[@]}" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [ "$status" != "Accepted" ]; then
    if [ -n "$id" ]; then
      xcrun notarytool log "$id" "${NOTARY_ARGS[@]}" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# ---- icon --------------------------------------------------------------------------------------
# The .icns is generated here from the committed PNGs (assets/app-icon/lizard-<size>.png, made by
# `cargo xtask icon`); it is never committed.
ICONS="$ROOT/assets/app-icon"
ICONSET="$WORK/Lizard.iconset"
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  cp "$ICONS/lizard-$s.png" "$ICONSET/icon_${s}x${s}.png"
  cp "$ICONS/lizard-$((s * 2)).png" "$ICONSET/icon_${s}x${s}@2x.png"
done
iconutil -c icns -o "$WORK/Lizard.icns" "$ICONSET"

# ---- Lizard.app -------------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/lizard" "$APP/Contents/MacOS/Lizard"
cp "$WORK/Lizard.icns" "$APP/Contents/Resources/Lizard.icns"
NODE_RUNTIME="$APP/Contents/Resources/Agents/runtime"
mkdir -p "$NODE_RUNTIME/bin" "$NODE_RUNTIME/lib/node_modules" "$APP/Contents/Resources/Licenses"
NODE_BINS=()
for t in "${TARGETS[@]}"; do
  case "$t" in
    aarch64-apple-darwin)
      node_arch=arm64
      node_sha=c9e1dbc748b33ed2d14f42445191ecaf14c56259c7a9a26d0042c49ec2b66428 ;;
    x86_64-apple-darwin)
      node_arch=x64
      node_sha=efcc2b0db6f94bf53eddb038fe2b370277a042bd29e4f19b8f8e23598795b819 ;;
  esac
  node_name="node-v$NODE_VERSION-darwin-$node_arch"
  node_archive="$WORK/$node_name.tar.gz"
  curl -fsSL --retry 3 "https://nodejs.org/dist/v$NODE_VERSION/$node_name.tar.gz" -o "$node_archive"
  printf '%s  %s\n' "$node_sha" "$node_archive" | shasum -a 256 -c -
  mkdir -p "$WORK/$node_name"
  tar -xzf "$node_archive" -C "$WORK/$node_name" --strip-components=1
  NODE_BINS+=("$WORK/$node_name/bin/node")
  if [ ! -f "$NODE_RUNTIME/lib/node_modules/npm/bin/npm-cli.js" ]; then
    cp -R "$WORK/$node_name/lib/node_modules/npm" "$NODE_RUNTIME/lib/node_modules/npm"
    cp "$WORK/$node_name/LICENSE" "$APP/Contents/Resources/Licenses/Node-LICENSE"
  fi
done
lipo -create -output "$NODE_RUNTIME/bin/node" "${NODE_BINS[@]}"
ln -s ../lib/node_modules/npm/bin/npm-cli.js "$NODE_RUNTIME/bin/npm"
mkdir -p "$APP/Contents/Resources/Licenses"
for notice in LICENSE-MIT LICENSE-APACHE NOTICE; do
  cp "$ROOT/$notice" "$APP/Contents/Resources/Licenses/$notice"
done
cp "$ROOT/ATTRIBUTION.md" "$APP/Contents/Resources/Licenses/ATTRIBUTION.md"
cp "$ROOT/assets/fonts/nv-legible-next/OFL.txt" "$APP/Contents/Resources/Licenses/OFL-NV-Legible-Next.txt"
# Licences of the embedded craft-fonts fonts (only when built with CRAFT_FONTS_DIR).
if [ -n "${CRAFT_FONTS_DIR:-}" ]; then
  copy_font_licences "$APP/Contents/Resources/Licenses"
fi
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${LIZARD_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"

# Sign inside-out: nested code first, then the bundle itself (no --deep on the final signature).
sign --options runtime "$NODE_RUNTIME/bin/node"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/Lizard"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

if [ "$NOTARIZE" = 1 ]; then
  ditto -c -k --keepParent "$APP" "$WORK/Lizard-notarize.zip"
  notarize "$WORK/Lizard-notarize.zip"
  retry xcrun stapler staple "$APP"
  retry xcrun stapler validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/Lizard.app"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
# `makehybrid -hfs` adds FinderInfo to files inside the image, invalidating the already-signed
# app when the DMG is mounted. A direct compressed HFS+ image preserves the app's signature.
retry hdiutil create -srcfolder "$STAGE" -format UDZO -fs HFS+ -volname Lizard -ov -o "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  retry xcrun stapler staple "$DMG"
  retry xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

# ---- CLI ---------------------------------------------------------------------------------------
if [ "$HAS_CLI" = 1 ]; then
  echo "==> building $CLI_ZIP"
  CLI_DIR="$WORK/lizard-cli-$VERSION-macos-$ARCH"
  mkdir -p "$CLI_DIR"
  cp "$WORK/bin/lizard-cli" "$CLI_DIR/"
  copy_docs "$CLI_DIR"
  # Same Developer ID and hardened runtime as the app, with an explicit reverse-DNS identifier
  # (codesign would otherwise use the bare file name).
  sign --options runtime --identifier com.brava.lizard-cli "$CLI_DIR/lizard-cli"
  codesign --verify --strict --verbose=2 "$CLI_DIR/lizard-cli"
  rm -f "$CLI_ZIP"
  ditto -c -k --keepParent "$CLI_DIR" "$CLI_ZIP"
  # notarytool accepts a zip of the bare binary. A bare Mach-O can't carry a stapled ticket, so
  # Gatekeeper looks the notarization up online when a quarantined copy first runs.
  if [ "$NOTARIZE" = 1 ]; then notarize "$CLI_ZIP"; fi
  "$WORK/bin/lizard-cli" --version
else
  warn "apps/lizard-cli is not in this workspace: no CLI zip"
  rm -f "$CLI_ZIP"
fi

"$WORK/bin/lizard" --version
echo "==> done (check the artifacts as shipped with packaging/macos/verify.sh --arch $ARCH)"
ls -lh "$DMG"
