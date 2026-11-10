#!/usr/bin/env bash
# Check the macOS release artifacts exactly as users download them:
#
#   $DIST/lizard-<version>-macos-<arch>.dmg         signed, notarized, ticket stapled
#   $DIST/lizard-cli-<version>-macos-<arch>.zip   (when apps/lizard-cli exists) unpacked here; the binary inside must be
#                                                    Developer ID signed with the hardened runtime,
#                                                    timestamped and notarized
#
# Usage: packaging/macos/verify.sh [--arch universal|aarch64|x86_64]
#
# With a real signing identity and notarization credentials (MACOS_NOTARY_PROFILE or the
# APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID combination used by package.sh), every
# check is strict and a failure exits non-zero. Without them (local ad-hoc builds, or a release run
# whose credentials are missing) only the signature's integrity is checked and the rest is a warning,
# matching package.sh. Nothing here reads or prints a secret's value; it only tests whether one is set.
#
# A bare Mach-O can't carry a stapled notarization ticket (only .app, .dmg and .pkg can), so for the
# CLI Gatekeeper looks the ticket up online the first time a quarantined copy runs. `spctl --assess
# --type install` does that same online lookup here, which is how this script proves the CLI was
# notarized.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"

ARCH=universal
while [ $# -gt 0 ]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    -h | --help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) WANT_ARCHS="x86_64 arm64" ;;
  aarch64) WANT_ARCHS="arm64" ;;
  x86_64) WANT_ARCHS="x86_64" ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

DMG="$DIST/lizard-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/lizard-cli-$VERSION-macos-$ARCH.zip"
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
STRICT=0
if [ "$IDENTITY" != "-" ]; then
  if [ -n "${MACOS_NOTARY_PROFILE:-}" ] || \
    { [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_PASSWORD:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ]; }; then
    STRICT=1
  fi
fi

FAILED=0
fail() {
  if [ "$STRICT" = 1 ]; then
    echo "error: $*" >&2
    FAILED=1
  else
    warn "$* (expected without signing secrets)"
  fi
}

# spctl's online ticket lookup can lag a few seconds behind notarytool's "Accepted".
assess() {
  local out attempt tries=1
  [ "$STRICT" = 1 ] && tries=5
  for attempt in $(seq "$tries"); do
    if out="$(spctl "$@" 2>&1)"; then
      printf '%s\n' "$out"
      return 0
    fi
    [ "$attempt" = "$tries" ] || sleep 10
  done
  printf '%s\n' "$out"
  return 1
}

FILES=("$DMG")
if [ "$HAS_CLI" = 1 ]; then FILES+=("$CLI_ZIP"); fi
for f in "${FILES[@]}"; do
  if [ ! -f "$f" ]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

WORK="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/lizard-verify.XXXXXX")"
MOUNT="$WORK/mount"
MOUNTED=0
cleanup() {
  if [ "$MOUNTED" = 1 ]; then hdiutil detach "$MOUNT" >/dev/null 2>&1 || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- CLI zip (when apps/lizard-cli exists) ----------------------------------------------------
if [ "$HAS_CLI" = 1 ]; then
  echo "==> $(basename "$CLI_ZIP")"
  ditto -x -k "$CLI_ZIP" "$WORK/cli"
  CLI="$WORK/cli/lizard-cli-$VERSION-macos-$ARCH/lizard-cli"
  if [ ! -x "$CLI" ]; then
    echo "error: $(basename "$CLI_ZIP") has no executable lizard-cli-$VERSION-macos-$ARCH/lizard-cli" >&2
    exit 1
  fi

  archs="$(lipo -archs "$CLI")"
  for a in $WANT_ARCHS; do
    case " $archs " in
      *" $a "*) ;;
      *) echo "error: lizard-cli lacks $a (has: $archs)" >&2; exit 1 ;;
    esac
  done

  # Integrity first: this must hold even for ad-hoc builds.
  codesign --verify --strict --verbose=2 "$CLI"
  details="$(codesign -d --verbose=2 "$CLI" 2>&1)"
  printf '%s\n' "$details" | grep -E '^(Identifier|Format|CodeDirectory|Authority|Timestamp|TeamIdentifier)[= ]' || true

  printf '%s\n' "$details" | grep -Eq '^CodeDirectory .*flags=0x[0-9a-f]+\([^)]*runtime' \
    || fail "lizard-cli is not signed with the hardened runtime"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' \
    || fail "lizard-cli is not signed with a Developer ID Application identity"
  printf '%s\n' "$details" | grep -q '^Timestamp=' \
    || fail "lizard-cli signature has no secure timestamp"
  if [ -n "${APPLE_TEAM_ID:-}" ]; then
    printf '%s\n' "$details" | grep -qx "TeamIdentifier=$APPLE_TEAM_ID" \
      || fail "lizard-cli is signed by a different team"
  fi
  if gk="$(assess --assess --type install -vv "$CLI")"; then
    printf '%s\n' "$gk"
    printf '%s\n' "$gk" | grep -q 'source=Notarized Developer ID' \
      || fail "Gatekeeper accepts lizard-cli but not as notarized"
  else
    printf '%s\n' "$gk"
    fail "Gatekeeper rejects lizard-cli (not notarized?)"
  fi
  "$CLI" --version
fi

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> $(basename "$DMG")"
codesign --verify --strict --verbose=2 "$DMG"
mkdir -p "$MOUNT"
retry hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG" >/dev/null
MOUNTED=1
APP="$MOUNT/Lizard.app"
if [ ! -d "$APP" ]; then
  echo "error: DMG has no Lizard.app" >&2
  exit 1
fi
if [ "$(readlink "$MOUNT/Applications")" != /Applications ]; then
  echo "error: DMG has no drag-to-Applications link" >&2
  exit 1
fi
if [ ! -s "$MOUNT/.DS_Store" ] ||
   ! cmp -s "$ROOT/packaging/macos/dmg-background.png" "$MOUNT/.background/dmg-background.png"; then
  echo "error: DMG is missing the branded Finder layout" >&2
  exit 1
fi
codesign --verify --strict --deep --verbose=2 "$APP"
bundled_node="$APP/Contents/Resources/Agents/runtime/bin/node"
if [ ! -x "$bundled_node" ]; then
  echo "error: Lizard.app is missing its agent Node.js runtime" >&2
  exit 1
fi
codesign --verify --strict --verbose=2 "$bundled_node" || fail "the agent Node.js runtime has an invalid signature"
node_archs="$(lipo -archs "$bundled_node")"
for a in $WANT_ARCHS; do
  case " $node_archs " in
    *" $a "*) ;;
    *) fail "the agent Node.js runtime lacks $a (has: $node_archs)" ;;
  esac
done
# --version does not exercise V8. Run JavaScript to catch a hardened-runtime JIT crash.
if [ "$("$bundled_node" -e 'process.stdout.write("lizard-node-ok")')" != lizard-node-ok ]; then
  echo "error: the agent Node.js runtime cannot execute JavaScript" >&2
  exit 1
fi
if [ "$HAS_CLI" = 1 ]; then
  bundled_cli="$APP/Contents/MacOS/lizard-cli"
  if [ ! -x "$bundled_cli" ]; then
    echo "error: Lizard.app is missing the agent's lizard-cli bridge" >&2
    exit 1
  fi
  codesign --verify --strict --verbose=2 "$bundled_cli" || fail "the bundled agent bridge has an invalid signature"
  bundled_cli_archs="$(lipo -archs "$bundled_cli")"
  for a in $WANT_ARCHS; do
    case " $bundled_cli_archs " in
      *" $a "*) ;;
      *) fail "the bundled agent bridge lacks $a (has: $bundled_cli_archs)" ;;
    esac
  done
fi
bundle_id="$(plutil -extract CFBundleIdentifier raw -o - "$APP/Contents/Info.plist")"
if [ "$bundle_id" != com.brava.lizard ]; then
  echo "error: wrong bundle identifier: $bundle_id" >&2
  exit 1
fi
archs="$(lipo -archs "$APP/Contents/MacOS/Lizard")"
for a in $WANT_ARCHS; do
  case " $archs " in
    *" $a "*) ;;
    *) echo "error: Lizard.app lacks $a (has: $archs)" >&2; exit 1 ;;
  esac
done
for notice in LICENSE-MIT LICENSE-APACHE NOTICE; do
  if [ ! -f "$APP/Contents/Resources/Licenses/$notice" ]; then
    echo "error: Lizard.app is missing $notice" >&2
    exit 1
  fi
done
if [ "$STRICT" = 1 ]; then
  details="$(codesign -d --verbose=2 "$APP" 2>&1)"
  printf '%s\n' "$details" | grep -q '^Authority=Developer ID Application:' \
    || fail "Lizard.app is not signed with Developer ID Application"
  printf '%s\n' "$details" | grep -q '^Timestamp=' \
    || fail "Lizard.app signature has no secure timestamp"
  if [ -n "${APPLE_TEAM_ID:-}" ]; then
    printf '%s\n' "$details" | grep -qx "TeamIdentifier=$APPLE_TEAM_ID" \
      || fail "Lizard.app is signed by a different team"
  fi
  retry xcrun stapler validate "$APP" || fail "no notarization ticket stapled to Lizard.app"
  assess --assess --type execute -vv "$APP" || fail "Gatekeeper rejects Lizard.app"
  retry xcrun stapler validate "$DMG" || fail "no notarization ticket stapled to the DMG"
  assess --assess --type open --context context:primary-signature -vv "$DMG" \
    || fail "Gatekeeper rejects the DMG"
fi

if [ "$FAILED" != 0 ]; then
  echo "error: macOS artifacts failed verification" >&2
  exit 1
fi

if [ "$STRICT" = 1 ]; then
  result="signed (Developer ID, hardened runtime), notarized; DMG stapled"
else
  result="signature intact; Developer ID and notarization not required (no signing secrets)"
fi
echo "==> verified: $result"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  names="\`$(basename "$DMG")\`"
  if [ "$HAS_CLI" = 1 ]; then names="$names, \`$(basename "$CLI_ZIP")\`"; fi
  echo "- macOS $names: $result" >>"$GITHUB_STEP_SUMMARY"
fi
