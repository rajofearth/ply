#!/bin/sh
# Assemble Ply.app on macOS after `cargo build --release`.
# Run from the repo root: sh scripts/bundle-macos.sh
set -eu

APP="Ply.app"
BIN="target/release/ply"
ICNS="assets/macos/Ply.icns"
PLIST="packaging/macos/Info.plist"

[ -f "$BIN" ] || { echo "missing $BIN, run cargo build --release first" >&2; exit 1; }
[ -f "$ICNS" ] || { echo "missing $ICNS" >&2; exit 1; }

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/ply"
cp "$ICNS" "$APP/Contents/Resources/Ply.icns"
cp "$PLIST" "$APP/Contents/Info.plist"
echo "wrote $APP"
