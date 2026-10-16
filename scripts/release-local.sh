#!/bin/bash
# Release typeThai from this Mac and a Windows PC instead of the CI release workflow (about 5-8 minutes with warm
# caches instead of about 25). macOS is built and signed here; Windows is built over SSH on the PC, in parallel. Then
# the updater packages are signed and everything is published as one GitHub release. The tag is created only when the
# finished release is published, and the release workflow sees the uploaded files and skips its builds.
#
# usage: scripts/release-local.sh vX.Y.Z [--skip-tests] [--no-publish]
#   Commit the version bump and the release notes and push them first: the PC builds that exact commit from GitHub.
#   --no-publish leaves the signed files in dist/vX.Y.Z without touching GitHub.
# Needs: the aarch64-apple-darwin and x86_64-apple-darwin Rust targets, openssl with ed25519, gh (logged in), the
# release keys in ~/.config/typethai-release ($TYPETHAI_RELEASE_KEYS), and SSH to a Windows PC ($TYPETHAI_WIN_HOST,
# default vfx-alkane-1) with git, Rust and Inno Setup 6.
set -euo pipefail
tag=${1:?usage: scripts/release-local.sh vX.Y.Z [--skip-tests] [--no-publish]}; shift
tests=1; publish=1
for a in "$@"; do case $a in --skip-tests) tests=0;; --no-publish) publish=0;; *) echo "unknown option $a" >&2; exit 2;; esac; done
root=$(cd "$(dirname "$0")/.." && pwd); cd "$root"
win=${TYPETHAI_WIN_HOST:-vfx-alkane-1}
keys=${TYPETHAI_RELEASE_KEYS:-$HOME/.config/typethai-release}
repo=phasu21/typethai
out="$root/dist/$tag"
step() { printf '\n== %s\n' "$*"; }
die() { echo "release-local: $*" >&2; exit 1; }

# ---------- checks ----------
[[ $tag =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "tag must look like v1.2.3"
version=$(sed -n 's/^version = "\(.*\)"/\1/p' rust/crates/typethai-desktop/Cargo.toml | head -1)
[ "v$version" = "$tag" ] || die "Cargo.toml says $version, not ${tag#v}: bump the version first"
[ -z "$(git status --porcelain)" ] || die "the working tree has changes: commit them first"
git fetch -q origin
[ -n "$(git branch -r --contains HEAD)" ] || die "HEAD is not on GitHub: push it first (the PC builds it from there)"
sha=$(git rev-parse HEAD)
if gh release view "$tag" -R "$repo" --json isDraft -q .isDraft 2>/dev/null | grep -q false; then die "$tag is already published"; fi
for f in codesign.p12 codesign.p12.password update-ed25519.pem; do [ -f "$keys/$f" ] || die "missing $keys/$f"; done
pub=$(openssl pkey -in "$keys/update-ed25519.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 64)
grep -q "const PUBLIC_KEY: &str = \"$pub\"" rust/crates/typethai-desktop/src/update.rs || die "the update key does not match PUBLIC_KEY in update.rs"
ssh -o BatchMode=yes -o ConnectTimeout=10 "$win" "echo ok" >/dev/null || die "cannot reach $win over SSH"

# The Windows side runs as one PowerShell script (-EncodedCommand avoids cmd.exe quoting).
winps() { ssh -o BatchMode=yes "$win" "powershell -NoProfile -NonInteractive -EncodedCommand $(printf '%s' "\$ProgressPreference='SilentlyContinue'; \$ErrorActionPreference='Stop'; $1" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"; }

# ---------- model (a pinned asset, as in the release workflow) ----------
model_tag=$(sed -n 's/^ *MODEL_TAG: *//p' .github/workflows/release.yml); model_sha=$(sed -n 's/^ *MODEL_SHA256: *//p' .github/workflows/release.yml)
model="$root/dist/model/$model_tag/typethai.ttm"
if ! echo "$model_sha  $model" | shasum -a 256 -c --status 2>/dev/null; then
  step "model $model_tag"
  rm -rf "$(dirname "$model")"; gh release download "$model_tag" -R "$repo" -p typethai.ttm -D "$(dirname "$model")"
  echo "$model_sha  $model" | shasum -a 256 -c --status || die "model sha256 mismatch"
fi
rm -rf "$out"; mkdir -p "$out"

# ---------- Windows, in the background ----------
step "Windows build on $win (log: $out/windows.log)"
(
  set -e
  winps '$d = "C:\tmp\typethai-build"; if (-not (Test-Path "$d\.git")) { git clone -q https://github.com/'"$repo"' $d }'
  scp -q -o BatchMode=yes "$model" "$win:C:/tmp/typethai-build/typethai.ttm"
  winps '
    $d = "C:\tmp\typethai-build"; $tag = "'"$tag"'"
    git -C $d fetch -q origin; git -C $d checkout -q -f '"$sha"'; if ($LASTEXITCODE) { throw "checkout failed" }
    Set-Location "$d\rust"; cargo build --release -p typethai-desktop 2>&1 | ForEach-Object { "$_" }; if ($LASTEXITCODE) { throw "cargo build failed" }
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw "Inno Setup 6 is not installed (winget install JRSoftware.InnoSetup)" }
    $dist = "$d\dist"; Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
    $pkg = "$dist\typeThai-$tag-windows-x86_64"; New-Item -ItemType Directory -Force $pkg | Out-Null
    Copy-Item "$d\rust\target\release\typethai-desktop.exe", "$d\LICENSE", "$d\DATA_LICENSES.md", "$d\README.md" $pkg
    Copy-Item "$d\typethai.ttm" "$pkg\typethai.ttm"
    Compress-Archive -Path $pkg -DestinationPath "$pkg.zip"
    & $iscc /Q "/DAppVersion=$($tag.TrimStart("v"))" "/DTag=$tag" "/DSrc=$pkg" "/O$dist" "$d\packaging\typethai.iss"; if ($LASTEXITCODE) { throw "iscc failed" }
    Get-ChildItem $dist -File | ForEach-Object { "built $($_.Name)" }'
  for f in "typeThai-$tag-windows-x86_64.zip" "typeThai-$tag-windows-x86_64-setup.exe"; do
    scp -q -o BatchMode=yes "$win:C:/tmp/typethai-build/dist/$f" "$out/"
  done
) > "$out/windows.log" 2>&1 &
winjob=$!

# ---------- macOS, here ----------
if [ $tests = 1 ]; then
  step "tests"
  (cd rust && TYPETHAI_MODEL="$model" cargo test -q --release -p typethai-core -p typethai-desktop 2>&1 | grep -E "^test result|FAILED|panicked" ) || die "tests failed"
  out_sim=$(cd rust && printf 'l;ylfu <Tab>\n' | cargo run -q --release -p typethai-desktop -- --simulate --model "$model" | tail -1)
  [ "$out_sim" = "สวัสดี " ] || die "simulate check failed: got '$out_sim'"
fi
step "macOS universal build"
export MACOSX_DEPLOYMENT_TARGET=12.0
for t in aarch64-apple-darwin x86_64-apple-darwin; do (cd rust && cargo build -q --release -p typethai-desktop --target $t); done
MACOS_SIGN_P12="$keys/codesign.p12" MACOS_SIGN_P12_PASSWORD=$(cat "$keys/codesign.p12.password") scripts/package-macos.sh "$tag" "$model" "$out/macos" >/dev/null
mv "$out/macos/typeThai-$tag-macos-universal.dmg" "$out/macos/typeThai-$tag-macos-universal.zip" "$out/"; rm -rf "$out/macos"
step "macOS smoke test (from the DMG)"
mnt=$(mktemp -d); hdiutil attach -nobrowse -readonly -mountpoint "$mnt" "$out/typeThai-$tag-macos-universal.dmg" >/dev/null
bin="$mnt/typeThai.app/Contents/MacOS/typethai-desktop"
ok=1; [ "$(lipo -archs "$bin")" = "x86_64 arm64" ] || ok=0; codesign --verify --deep --strict "$mnt/typeThai.app" || ok=0
smoke=$(printf 'l;ylfu <Tab>\n' | "$bin" --simulate | tail -1); [ "$smoke" = "สวัสดี " ] || ok=0
hdiutil detach -quiet "$mnt"; [ $ok = 1 ] || die "macOS smoke test failed"

step "waiting for Windows"
if ! wait $winjob; then tail -30 "$out/windows.log" >&2; die "Windows build failed (full log: $out/windows.log)"; fi
grep '^built ' "$out/windows.log" || true

# ---------- updater signatures ----------
step "signing the updater packages"
for f in "$out/typeThai-$tag-macos-universal.zip" "$out/typeThai-$tag-windows-x86_64-setup.exe"; do
  [ -s "$f" ] || die "missing $f"
  openssl pkeyutl -sign -inkey "$keys/update-ed25519.pem" -rawin -in "$f" | xxd -p | tr -d '\n' > "$f.sig"
  echo "signed $(basename "$f")"
done
cp "$model" "$out/typethai.ttm"
ls -la "$out" | grep -v windows.log

[ $publish = 1 ] || { step "built in $out (not published)"; exit 0; }

# ---------- publish ----------
# A draft has no tag yet; publishing it creates the tag, and the release workflow then finds the files and skips.
step "publishing $tag"
pre=(); case $tag in *-*) pre=(--prerelease);; esac
gh release view "$tag" -R "$repo" >/dev/null 2>&1 || gh release create "$tag" -R "$repo" --draft "${pre[@]}" --target "$sha" --title "typeThai $tag" --notes-file .github/release-notes.md
gh release upload "$tag" -R "$repo" --clobber "$out"/*.dmg "$out"/*.zip "$out"/*-setup.exe "$out"/*.sig "$out/typethai.ttm"
gh release edit "$tag" -R "$repo" --draft=false
git fetch -q origin --tags
gh release view "$tag" -R "$repo" --json url -q .url
