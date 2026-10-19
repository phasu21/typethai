#!/bin/sh
# Type-check the Windows backend from macOS: `cargo check --target x86_64-pc-windows-msvc -p typethai-desktop`.
#
# Two build scripts stand in the way on a non-Windows host, and neither is about typeThai's own code:
#   - gpui's build script embeds a manifest/resource for the Windows build and needs a resource compiler
#     (`llvm-rc`, from Homebrew's llvm; Xcode's clang has none);
#   - zstd-sys (through typethai-predict) would run `cc` for the msvc target, which this host cannot do. Its
#     `ZSTD_SYS_USE_PKG_CONFIG` path only has to answer a probe, so a stub pkg-config in a temp dir does it: `cargo
#     check` never links, so the paths the stub prints are never used.
# Extra arguments are passed to cargo, e.g. `scripts/check-windows.sh --message-format short`.
set -e
root=$(cd "$(dirname "$0")/.." && pwd)
if ! command -v llvm-rc >/dev/null 2>&1; then
    for p in /opt/homebrew/opt/llvm/bin /usr/local/opt/llvm/bin; do
        if [ -x "$p/llvm-rc" ]; then PATH="$p:$PATH"; break; fi
    done
    command -v llvm-rc >/dev/null 2>&1 || {
        echo "check-windows: no llvm-rc on PATH (brew install llvm); gpui's build script needs one for the Windows target" >&2
        exit 2
    }
fi
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cat > "$tmp/pkg-config" <<'STUB'
#!/bin/sh
# Only has to satisfy zstd-sys's probe; nothing is linked by `cargo check`.
for a in "$@"; do case "$a" in
    --version) echo 0.29.2; exit 0;;
    --modversion) echo 1.5.7; exit 0;;
    --atleast-version=*|--exact-version=*|--max-version=*) exit 0;;
esac; done
flags=""
for a in "$@"; do case "$a" in
    --libs) flags="$flags -lzstd";;
    --cflags) flags="$flags -I/usr/include";;
esac; done
echo "$flags"
STUB
chmod +x "$tmp/pkg-config"
echo "check-windows: target ${TARGET:-x86_64-pc-windows-msvc}" >&2
PATH="$PATH" PKG_CONFIG="$tmp/pkg-config" PKG_CONFIG_ALLOW_CROSS=1 ZSTD_SYS_USE_PKG_CONFIG=1 \
    cargo check --manifest-path "$root/rust/Cargo.toml" -p typethai-desktop --target "${TARGET:-x86_64-pc-windows-msvc}" "$@"
