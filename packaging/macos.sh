#!/usr/bin/env bash
set -euo pipefail
app=dist/ClipVault.app
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/clipvault "$app/Contents/MacOS/clipvault"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
sed "s/0\.1\.0/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"
cp README.md LICENSE "$app/Contents/Resources/"
codesign --force --deep --sign - "$app"
hdiutil create -volname ClipVault -srcfolder "$app" -ov -format UDZO dist/ClipVault-macOS.dmg
rm -rf "$app"
