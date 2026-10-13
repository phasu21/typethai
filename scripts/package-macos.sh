#!/bin/bash
# Build typeThai.app for one macOS target and zip it.
# usage: scripts/package-macos.sh <aarch64-apple-darwin|x86_64-apple-darwin> <version> <typethai.ttm> [out dir]
set -euo pipefail
target=$1; version=$2; model=$3; out=${4:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
case $target in aarch64-*) arch=aarch64;; x86_64-*) arch=x86_64;; *) echo "bad target $target"; exit 2;; esac
bin="$root/rust/target/$target/release/typethai-desktop"
[ -x "$bin" ] || (cd "$root/rust" && cargo build --release -p typethai-desktop --target "$target")
app="$out/typeThai.app"; rm -rf "$app"; mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/typethai-desktop"
cp "$model" "$app/Contents/Resources/typethai.ttm"
cp "$root/LICENSE" "$root/DATA_LICENSES.md" "$app/Contents/Resources/"
sed "s/__VERSION__/${version#v}/g" "$root/packaging/Info.plist" > "$app/Contents/Info.plist"
codesign --force --deep --sign - --identifier io.github.phasu21.typethai "$app"
codesign -dv "$app"
zip="$out/typeThai-$version-macos-$arch.zip"; rm -f "$zip"
(cd "$out" && ditto -c -k --keepParent typeThai.app "$(basename "$zip")")
echo "$zip"
