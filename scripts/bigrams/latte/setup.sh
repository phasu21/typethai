#!/bin/sh
# Install OpenThai2.0-LATTE-WordSeg-Lite, the Thai word segmenter the pair counts are built with, into its own
# directory: the pinned wheel (sha256 checked) and a venv with pinned NumPy and ONNX Runtime. Build time only: the
# model never ships with the app (DATA_LICENSES.md).
# usage: scripts/bigrams/latte/setup.sh <latte dir>      then: LATTE=<latte dir> scripts/bigrams/build.sh ...
# PYTHON picks the interpreter for the venv (default python3; ONNX Runtime 1.32 has wheels for CPython 3.11-3.14).
set -eu
D=$1
HERE=$(cd "$(dirname "$0")" && pwd)
PY=${PYTHON:-python3}
# iapp/OpenThai2.0-LATTE-WordSeg-Lite on Hugging Face, CC BY-SA 4.0 (model, lexicon) and Apache-2.0 (runtime code).
REV=d5f39bcfb592763845ad7a5048726b0f62fd47a2
WHEEL=openthai2_0_latte_wordseg_lite-0.1.2-py3-none-any.whl
WHEEL_SHA256=39d4162470d2acfb1f375e42847f920d49ee368b656cfe04cef42953731aa926
MODEL_SHA256=2f8d349528b3d8342f5cb9150a4e07e774f07e40e892367dbe18bd41ea1d29f3
sum=$(command -v sha256sum || echo "shasum -a 256")
check() { h=$($sum "$2" | cut -d' ' -f1); [ "$h" = "$1" ] || { echo "sha256 mismatch: $2 is $h, expected $1" >&2; exit 1; }; }

mkdir -p "$D/dl"
if [ ! -s "$D/dl/$WHEEL" ]; then
  curl -fsSL --retry 5 -o "$D/dl/$WHEEL.part" "https://huggingface.co/iapp/OpenThai2.0-LATTE-WordSeg-Lite/resolve/$REV/$WHEEL"
  mv "$D/dl/$WHEEL.part" "$D/dl/$WHEEL"
fi
check "$WHEEL_SHA256" "$D/dl/$WHEEL"
[ -x "$D/venv/bin/python" ] || "$PY" -m venv "$D/venv"
# -I everywhere: the venv's Python never imports from the current directory or the user's site-packages
"$D/venv/bin/python" -I -m pip install -q --disable-pip-version-check --only-binary=:all: -r "$HERE/requirements.txt"
"$D/venv/bin/python" -I -m pip install -q --disable-pip-version-check --no-deps --force-reinstall "$D/dl/$WHEEL"
pkg=$("$D/venv/bin/python" -I -c 'import importlib.util as u; print(u.find_spec("openthai_latte_wordseg_lite").submodule_search_locations[0])')
check "$MODEL_SHA256" "$pkg/assets/model.int8.onnx"
echo "LATTE ready in $D"
