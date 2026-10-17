#!/usr/bin/env bash
# Build, sign and notarize the macOS plugin dylib locally.
#
# Local mirror of the release workflow's macOS leg (.github/workflows/
# release.yml), for testing Developer ID-signed dylibs against a signed
# Open CAD Studio without cutting a release: a Developer ID-signed host
# runs with the hardened runtime, whose library validation only loads
# plugins signed with the same Team ID — an ad-hoc dylib is refused with
# "mapping process and mapped file (non-platform) have different Team IDs".
#
# Differences from CI, on purpose: the certificate comes from the login
# keychain (no .p12 import) and notarization uses a notarytool keychain
# profile instead of raw credentials. plugin.toml metadata is not
# regenerated — the dylib is the artifact under test.
#
# Inputs (environment):
#   DEVELOPER_ID    "Developer ID Application: ..." identity. Auto-derived
#                   from the keychain when unset.
#   NOTARY_PROFILE  notarytool keychain profile (default: landsurvey-notary).
#                   Set to "" to skip notarization (signing-only round).
#
# Flags:
#   --install       after signing (and notarizing, unless skipped), copy the
#                   dylib into the installed plugin directory, backing up the
#                   one that was there.
#
# Output: dist/opencad.landsurvey-macos-aarch64.dylib (signed).
set -euo pipefail

cd "$(dirname "$0")/.."
ASSET=opencad.landsurvey-macos-aarch64.dylib
PLUGIN_DIR="$HOME/Library/Application Support/OpenCADStudio/plugins/opencad.landsurvey"
NOTARY_PROFILE="${NOTARY_PROFILE-landsurvey-notary}"
INSTALL=false
[ "${1:-}" = "--install" ] && INSTALL=true

if [ -z "${DEVELOPER_ID:-}" ]; then
    DEVELOPER_ID="$(security find-identity -v -p codesigning \
        | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)"
fi
[ -n "$DEVELOPER_ID" ] || { echo "no Developer ID Application identity in the keychain" >&2; exit 1; }
# Library validation compares Team IDs; the expected one is the
# parenthesized suffix of the signing identity itself.
TEAM_ID=$(sed -n 's/.*(\([A-Z0-9]\{10\}\))$/\1/p' <<<"$DEVELOPER_ID")
echo "==> signing as: $DEVELOPER_ID (team ${TEAM_ID:-?})"

echo "==> cargo build (--locked, like CI)"
cargo build --release --locked

echo "==> stage + sign"
mkdir -p dist
cp target/release/*.dylib "dist/$ASSET"
codesign --force --timestamp --options runtime -s "$DEVELOPER_ID" "dist/$ASSET"
codesign --verify --strict --verbose=2 "dist/$ASSET"
got=$(codesign -dv "dist/$ASSET" 2>&1 | sed -n 's/^TeamIdentifier=//p')
[ -n "$TEAM_ID" ] && [ "$got" = "$TEAM_ID" ] || { echo "dylib Team ID '$got' != identity team '$TEAM_ID'" >&2; exit 1; }

if [ -n "$NOTARY_PROFILE" ]; then
    echo "==> notarize (profile: $NOTARY_PROFILE)"
    # A bare dylib can't be stapled; Gatekeeper looks the ticket up online.
    ZIP=$(mktemp -t notarize).zip
    ditto -c -k --norsrc --noextattr "dist/$ASSET" "$ZIP"
    OUT=$(mktemp -t notary).json
    xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" \
        --wait --timeout 30m --output-format json > "$OUT" || true
    cat "$OUT"; echo
    read -r ID STATUS < <(python3 -c '
import json, sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    d = {}
print(d.get("id") or "-", d.get("status") or "-")' "$OUT")
    rm -f "$ZIP" "$OUT"
    if [ "$STATUS" != "Accepted" ]; then
        # The log names the exact per-file cause of a rejection.
        [ "$ID" != - ] && xcrun notarytool log "$ID" --keychain-profile "$NOTARY_PROFILE" || true
        echo "notarization $STATUS" >&2
        exit 1
    fi
else
    echo "==> NOTARY_PROFILE empty: skipping notarization"
fi

if $INSTALL; then
    echo "==> install into $PLUGIN_DIR"
    mkdir -p "$PLUGIN_DIR"
    [ -f "$PLUGIN_DIR/$ASSET" ] && cp "$PLUGIN_DIR/$ASSET" "$PLUGIN_DIR/$ASSET.bak"
    cp "dist/$ASSET" "$PLUGIN_DIR/$ASSET"
    echo "    previous dylib kept as $ASSET.bak (restart Open CAD Studio to load)"
fi

echo "==> done: dist/$ASSET"
