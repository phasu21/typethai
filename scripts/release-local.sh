#!/bin/bash
# Release typeThai from this Mac and a Windows PC instead of the CI release workflow (about 5-8 minutes with warm
# caches instead of about 25). macOS is built and signed here; Windows is built over SSH on the PC, in parallel. Then
# the updater packages are signed and everything is published as one GitHub release. The tag is created only when the
# finished release is published, and the release workflow sees the uploaded files and skips its builds.
#
# usage: scripts/release-local.sh vX.Y.Z [--skip-tests] [--no-publish | --publish-only]
#   Commit the version bump and the release notes and push them first: the PC builds that exact commit from GitHub.
#   --no-publish leaves the signed files in dist/vX.Y.Z without touching GitHub; --publish-only publishes those files
#   (after checking their signatures) without building again.
# Needs: the aarch64-apple-darwin and x86_64-apple-darwin Rust targets, openssl with ed25519, gh (logged in), the
# release keys in ~/.config/typethai-release ($TYPETHAI_RELEASE_KEYS), and SSH to a Windows PC ($TYPETHAI_WIN_HOST,
# default vfx-alkane-1) with git, Rust and Inno Setup 6.
set -euo pipefail
tag=${1:?usage: scripts/release-local.sh vX.Y.Z [--skip-tests] [--no-publish | --publish-only]}; shift
tests=1; publish=1; build=1
for a in "$@"; do case $a in --skip-tests) tests=0;; --no-publish) publish=0;; --publish-only) build=0;; *) echo "unknown option $a" >&2; exit 2;; esac; done
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
# The release ships $sha, not the tree: this checkout is also where work in progress lives, so a dirty tree is worth
# saying out loud but must not block a release of the commit (untracked files are not in it either way).
[ -z "$(git status --porcelain --untracked-files=no)" ] || echo "release-local: the working tree has uncommitted changes; this release ships $sha and NOT them" >&2
git fetch -q origin
[ -n "$(git branch -r --contains HEAD)" ] || die "HEAD is not on GitHub: push it first (the PC builds it from there)"
sha=$(git rev-parse HEAD)
if [ $publish = 1 ] && gh release view "$tag" -R "$repo" --json isDraft -q .isDraft 2>/dev/null | grep -q false; then die "$tag is already published"; fi
for f in codesign.p12 codesign.p12.password update-ed25519.pem; do [ -f "$keys/$f" ] || die "missing $keys/$f"; done
pub=$(openssl pkey -in "$keys/update-ed25519.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 64)
grep -q "const PUBLIC_KEY: &str = \"$pub\"" rust/crates/typethai-desktop/src/update.rs || die "the update key does not match PUBLIC_KEY in update.rs"
[ $build = 0 ] || ssh -o BatchMode=yes -o ConnectTimeout=10 "$win" "echo ok" >/dev/null || die "cannot reach $win over SSH"

# The Windows side runs as one PowerShell script (-EncodedCommand avoids cmd.exe quoting).
# Native commands are checked through $LASTEXITCODE: with ErrorActionPreference=Stop, Windows PowerShell treats any
# stderr line (cargo's progress) as a failure.
winps() { ssh -o BatchMode=yes "$win" "powershell -NoProfile -NonInteractive -EncodedCommand $(printf '%s' "\$ProgressPreference='SilentlyContinue'; $1" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')"; }

if [ $build = 1 ]; then
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
  winps '$d = "C:\tmp\typethai-build"; if (-not (Test-Path "$d\.git")) { git clone -q https://github.com/'"$repo"' $d; if ($LASTEXITCODE) { exit 1 } }'
  scp -q -o BatchMode=yes "$model" "$win:C:/tmp/typethai-build/typethai.ttm"
  winps '
    $d = "C:\tmp\typethai-build"; $tag = "'"$tag"'"
    git -C $d fetch -q origin; git -C $d checkout -q -f '"$sha"'; if ($LASTEXITCODE) { throw "checkout failed" }
    Set-Location "$d\rust"; cmd /c "cargo build --release -p typethai-desktop 2>&1"; if ($LASTEXITCODE) { throw "cargo build failed" }
    $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    if (-not $iscc) { throw "Inno Setup 6 is not installed (winget install JRSoftware.InnoSetup)" }
    $dist = "$d\dist"; Remove-Item $dist -Recurse -Force -ErrorAction SilentlyContinue
    $pkg = "$dist\typeThai-$tag-windows-x86_64"; New-Item -ItemType Directory -Force $pkg -ErrorAction Stop | Out-Null
    Copy-Item "$d\rust\target\release\typethai-desktop.exe", "$d\LICENSE", "$d\DATA_LICENSES.md", "$d\README.md" $pkg -ErrorAction Stop
    Copy-Item "$d\typethai.ttm" "$pkg\typethai.ttm" -ErrorAction Stop
    Compress-Archive -Path $pkg -DestinationPath "$pkg.zip" -ErrorAction Stop
    & $iscc /Q "/DAppVersion=$($tag.TrimStart("v"))" "/DTag=$tag" "/DSrc=$pkg" "/O$dist" "$d\packaging\typethai.iss"; if ($LASTEXITCODE) { throw "iscc failed" }
    Get-ChildItem $dist -File | ForEach-Object { "built $($_.Name)" }' || exit 1
  for f in "typeThai-$tag-windows-x86_64.zip" "typeThai-$tag-windows-x86_64-setup.exe"; do
    scp -q -o BatchMode=yes "$win:C:/tmp/typethai-build/dist/$f" "$out/"
  done
) > "$out/windows.log" 2>&1 &
winjob=$!

# ---------- macOS, here, from the commit ----------
# The Windows PC builds $sha from GitHub; the tests and the macOS build run in a throwaway worktree at the same commit,
# sharing the everyday target dir so the caches stay warm. Work in progress in the checkout cannot leak into a release.
src="$out/src"; rm -rf "$src"; git worktree add -q --detach "$src" "$sha"
trap 'git worktree remove --force "$src" 2>/dev/null || true' EXIT
if [ $tests = 1 ]; then
  step "tests"
  # debug build: it shares the cache of everyday `cargo test`; the release binary is checked from the DMG below
  (cd "$src/rust" && CARGO_TARGET_DIR="$root/rust/target" TYPETHAI_MODEL="$model" cargo test -q -p typethai-core -p typethai-desktop > "$out/tests.log" 2>&1) || { grep -E "FAILED|panicked" "$out/tests.log" >&2; die "tests failed (log: $out/tests.log)"; }
  grep -E "^test result: ok. [1-9]" "$out/tests.log" | sed 's/; 0 measured.*//'
fi
step "macOS universal build"
export MACOSX_DEPLOYMENT_TARGET=12.0
for t in aarch64-apple-darwin x86_64-apple-darwin; do (cd "$src/rust" && CARGO_TARGET_DIR="$root/rust/target" cargo build -q --release -p typethai-desktop --target $t); done
MACOS_SIGN_P12="$keys/codesign.p12" MACOS_SIGN_P12_PASSWORD=$(cat "$keys/codesign.p12.password") scripts/package-macos.sh "$tag" "$model" "$out/macos" >/dev/null
git worktree remove --force "$src"
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
else
  step "checking the files built in $out"
  pubpem=$(mktemp); sigbin=$(mktemp); openssl pkey -in "$keys/update-ed25519.pem" -pubout -out "$pubpem"
  for f in "$out/typeThai-$tag-macos-universal.zip" "$out/typeThai-$tag-windows-x86_64-setup.exe"; do
    [ -s "$f" ] && [ -s "$f.sig" ] || die "missing $f or its .sig: build first"
    xxd -r -p "$f.sig" > "$sigbin"; openssl pkeyutl -verify -pubin -inkey "$pubpem" -rawin -in "$f" -sigfile "$sigbin" >/dev/null || die "bad signature on $f"
  done
  rm -f "$pubpem" "$sigbin"
  for f in "typeThai-$tag-macos-universal.dmg" "typeThai-$tag-windows-x86_64.zip" typethai.ttm; do [ -s "$out/$f" ] || die "missing $out/$f"; done
fi

[ $publish = 1 ] || { step "built in $out (not published)"; exit 0; }

# ---------- publish ----------
# A draft has no tag yet; publishing it creates the tag, and the release workflow then finds the files and skips.
step "publishing $tag"
pre=""; case $tag in *-*) pre=--prerelease;; esac
gh release view "$tag" -R "$repo" >/dev/null 2>&1 || { git show "$sha:.github/release-notes.md" > "$out/notes.md"; gh release create "$tag" -R "$repo" --draft $pre --target "$sha" --title "typeThai $tag" --notes-file "$out/notes.md"; }
gh release upload "$tag" -R "$repo" --clobber "$out"/*.dmg "$out"/*.zip "$out"/*-setup.exe "$out"/*.sig "$out/typethai.ttm"
gh release edit "$tag" -R "$repo" --draft=false
git fetch -q origin --tags
gh release view "$tag" -R "$repo" --json url -q .url
