#!/usr/bin/env bash
set -euo pipefail
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
arch=$(dpkg --print-architecture)
root="dist/clipvault_${version}_${arch}"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/share/applications" "$root/usr/share/doc/clipvault"
install -m755 target/release/clipvault "$root/usr/bin/clipvault"
cp README.md LICENSE "$root/usr/share/doc/clipvault/"
printf 'Package: clipvault\nVersion: %s\nArchitecture: %s\nMaintainer: ErsatzHitman\nDepends: libx11-6, libxcb1, libxrandr2, libxi6, libxtst6, libxkbcommon0, libwayland-client0, libgl1\nDescription: Clipboard history with cloud semantic search\n' "$version" "$arch" > "$root/DEBIAN/control"
cp packaging/clipvault.desktop "$root/usr/share/applications/"
dpkg-deb --build --root-owner-group "$root"
tar -czf "dist/clipvault-linux-${arch}.tar.gz" -C target/release clipvault -C ../../ README.md LICENSE
rm -rf "$root"
