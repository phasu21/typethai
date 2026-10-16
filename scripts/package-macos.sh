#!/bin/bash
# Build a universal (arm64 + x86_64) typeThai.app, sign it, and package it twice: a drag-to-Applications DMG for
# installing and a zip that the in-app updater downloads.
# usage: scripts/package-macos.sh <version> <typethai.ttm> [out dir]
# Signing: with MACOS_SIGN_P12 (a .p12 file) and MACOS_SIGN_P12_PASSWORD set, the app is signed with that certificate
# through a throwaway keychain. The same certificate on every release keeps macOS's Accessibility permission across
# updates. Without them it is ad-hoc signed (local builds).
set -euo pipefail
version=$1; model=$2; out=${3:-dist}
root=$(cd "$(dirname "$0")/.." && pwd)
bins=()
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  bin="$root/rust/target/$target/release/typethai-desktop"
  [ -x "$bin" ] || (cd "$root/rust" && cargo build --release -p typethai-desktop --target "$target")
  bins+=("$bin")
done
mkdir -p "$out"
app="$out/typeThai.app"; rm -rf "$app"; mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
lipo -create -output "$app/Contents/MacOS/typethai-desktop" "${bins[@]}"
lipo -info "$app/Contents/MacOS/typethai-desktop"
cp "$model" "$app/Contents/Resources/typethai.ttm"
cp "$root/LICENSE" "$root/DATA_LICENSES.md" "$app/Contents/Resources/"
sed "s/__VERSION__/${version#v}/g" "$root/packaging/Info.plist" > "$app/Contents/Info.plist"

stage=$(mktemp -d); kcdir=$(mktemp -d); kc=""; search=()
cleanup() {
  if [ -n "$kc" ]; then
    [ ${#search[@]} -gt 0 ] && security list-keychains -d user -s "${search[@]}"
    security delete-keychain "$kc" 2>/dev/null || true
  fi
  rm -rf "$stage" "$kcdir"
}
trap cleanup EXIT
if [ -n "${MACOS_SIGN_P12:-}" ]; then
  kc="$kcdir/sign.keychain-db"; kpw=$(openssl rand -hex 16)
  security create-keychain -p "$kpw" "$kc"
  security set-keychain-settings "$kc"
  security unlock-keychain -p "$kpw" "$kc"
  security import "$MACOS_SIGN_P12" -k "$kc" -P "${MACOS_SIGN_P12_PASSWORD:-}" -T /usr/bin/codesign >/dev/null
  security set-key-partition-list -S apple-tool:,apple: -s -k "$kpw" "$kc" >/dev/null
  identity=$(security find-certificate -a -Z "$kc" | awk '/^SHA-1 hash:/ { print $3; exit }')
  # codesign needs the keychain on the search list; cleanup puts the old list back
  while read -r k; do search+=("$k"); done < <(security list-keychains -d user | tr -d '"' | sed 's/^ *//')
  security list-keychains -d user -s "$kc" "${search[@]}"
  codesign --force --deep --sign "$identity" --keychain "$kc" --identifier io.github.phasu21.typethai "$app"
else
  echo "warning: MACOS_SIGN_P12 not set; ad-hoc signing (macOS will ask for Accessibility again after each update)" >&2
  codesign --force --deep --sign - --identifier io.github.phasu21.typethai "$app"
fi
codesign --verify --deep --strict "$app"
codesign -dv "$app" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier)" || true
codesign -d -r- "$app" 2>&1 | grep designated || true

zip="$out/typeThai-$version-macos-universal.zip"; rm -f "$zip"
(cd "$out" && ditto -c -k --keepParent typeThai.app "$(basename "$zip")")
cp -R "$app" "$stage/"; ln -s /Applications "$stage/Applications"
dmg="$out/typeThai-$version-macos-universal.dmg"; rm -f "$dmg"
hdiutil create -volname "typeThai" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$dmg" >/dev/null
echo "$dmg"
echo "$zip"
