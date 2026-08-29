#!/usr/bin/env bash
# Builds and installs a local development build of the Gridded QuickLook
# app + preview extension, ad-hoc signed, for the current user.
#
# This mirrors the workflow used by rkrug/parquet-spotlight-quicklook and
# openrocket/macOS-QuickLook-extension: no Apple Developer account or
# provisioning profile is needed for local Quick Look testing, just an
# ad-hoc code signature (`codesign --sign -`).
#
# Requires a *full* Xcode install (not just the Command Line Tools) for the
# `xcodegen generate` + `xcodebuild` steps -- `xcodegen` itself only needs
# the CLT, but `xcodebuild` needs the full Xcode.app to build a macOS app
# extension target. Each Xcode-dependent step below is guarded with a
# clear error message rather than failing on an inscrutable `xcodebuild`
# error.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APPLE_DIR="$ROOT_DIR/apple"
PROJECT_PATH="$APPLE_DIR/GriddedQuickLook.xcodeproj"
SCHEME="GriddedQuickLook"
# Release, not Debug: Debug builds use Xcode's debug-dylib mechanism (a
# stub main executable that loads PreviewExtension.debug.dylib), which
# does not reliably load outside Xcode's own run harness -- the extension
# process starts but the principal class never instantiates.
CONFIGURATION="Release"
BUILD_DIR="$ROOT_DIR/build"
APP_NAME="GriddedQuickLook.app"
APP_DST="$HOME/Applications/$APP_NAME"

log() { echo "==> $*"; }
fail() {
  echo "FAIL: $*" >&2
  exit 1
}

require_full_xcode() {
  local dev_dir
  if ! dev_dir="$(xcode-select -p 2>/dev/null)"; then
    fail "xcode-select -p failed; is Xcode or the Command Line Tools installed?"
  fi
  if [[ "$dev_dir" != *"Xcode.app"* ]]; then
    fail "$(cat <<EOF
xcode-select is pointed at '$dev_dir', which looks like the Command Line
Tools rather than a full Xcode install. Building the QuickLook app
extension requires full Xcode (xcodebuild needs it to build a macOS
app-extension target).

Install Xcode from the App Store, then run:
  sudo xcode-select --switch /Applications/Xcode.app
and re-run this script.
EOF
)"
  fi
}

log "Building gridded-ffi release staticlib..."
( cd "$ROOT_DIR" && cargo build --release -p gridded-ffi )

log "Generating Xcode project (xcodegen)..."
# xcodegen is mise-managed; in non-interactive shells mise's shims may not be
# on PATH, so fall back to `mise exec` before giving up.
if command -v xcodegen >/dev/null 2>&1; then
  xcodegen generate --spec "$APPLE_DIR/project.yml"
elif command -v mise >/dev/null 2>&1; then
  ( cd "$ROOT_DIR" && mise exec -- xcodegen generate --spec "$APPLE_DIR/project.yml" )
else
  fail "xcodegen not found on PATH. It's declared in mise.toml -- try 'mise install' or 'mise run xcodeproj'."
fi

require_full_xcode

log "Building $SCHEME ($CONFIGURATION) with xcodebuild..."
xcodebuild \
  -project "$PROJECT_PATH" \
  -scheme "$SCHEME" \
  -configuration "$CONFIGURATION" \
  -derivedDataPath "$BUILD_DIR/DerivedData" \
  CODE_SIGN_IDENTITY="-" \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGNING_ALLOWED=YES \
  build

BUILT_APP="$BUILD_DIR/DerivedData/Build/Products/$CONFIGURATION/$APP_NAME"
if [[ ! -d "$BUILT_APP" ]]; then
  fail "Expected build product not found at $BUILT_APP"
fi

# NOTE: no re-signing here. xcodebuild already ad-hoc signs both the app
# and the embedded extension ("Sign to Run Locally") WITH their
# entitlements. A blanket `codesign --force --deep --sign -` would strip
# the appex's com.apple.security.app-sandbox entitlement (--deep applies
# entitlements only to the outer bundle), and macOS silently refuses to
# host an un-sandboxed app extension -- previews just never appear.

log "Installing to $APP_DST..."
mkdir -p "$HOME/Applications"
rm -rf "$APP_DST"
ditto "$BUILT_APP" "$APP_DST"
xattr -cr "$APP_DST" || true

log "Verifying code signature..."
if ! codesign --verify --deep --strict "$APP_DST"; then
  fail "Installed app failed code signature verification: $APP_DST"
fi

log "Opening $APP_NAME once so macOS registers it and its extension..."
open "$APP_DST"

log "Registering the preview extension with pluginkit (best effort)..."
APPEX="$APP_DST/Contents/PlugIns/PreviewExtension.appex"
if [[ -d "$APPEX" ]]; then
  pluginkit -a "$APPEX" >/dev/null 2>&1 || true
else
  echo "NOTE: expected extension bundle not found at $APPEX; skipping pluginkit registration." >&2
fi

log "Resetting Quick Look's cache and daemons..."
qlmanage -r >/dev/null 2>&1 || true
qlmanage -r cache >/dev/null 2>&1 || true
killall QuickLookUIService >/dev/null 2>&1 || true

cat <<EOF

Installed: $APP_DST

Finish enabling the extension in:
  System Settings -> General -> Login Items & Extensions -> Quick Look
  (look for "Gridded QuickLook Preview" and turn it on)

Then select a .nc/.h5 file, a .zarr store, or an .icechunk repo in Finder
and press Space to preview it. (Directory stores need the .zarr/.icechunk
extension on the folder name for Finder to offer a preview.)
EOF
