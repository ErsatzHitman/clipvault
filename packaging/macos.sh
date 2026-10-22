#!/usr/bin/env bash
set -euo pipefail
app=dist/ClipVault.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/clipvault "$app/Contents/MacOS/clipvault"
cp packaging/Info.plist "$app/Contents/Info.plist"
cp README.md LICENSE "$app/Contents/Resources/"
codesign --force --deep --sign - "$app"
hdiutil create -volname ClipVault -srcfolder "$app" -ov -format UDZO dist/ClipVault-macOS.dmg
rm -rf "$app"
