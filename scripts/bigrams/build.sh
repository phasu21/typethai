#!/bin/sh
# Build the word pair (bigram) sections and evaluate pruning levels.
# usage: scripts/bigrams/build.sh <data dir> <work dir> <base.ttm> <out.ttm> <min:top> [<min:top> ...]
#   <min:top>   --min-count and --top for tt-bigrams prune; top "all" keeps every follower
#   <data dir>  downloads from scripts/bigrams/fetch.sh
#   <work dir>  extracted text, pair files, one .ttm and eval report per level
#   <out.ttm>   base model + the pairs of the FIRST level listed
# Needs a Python with pyarrow for the Wikipedia parquet (PYTHON, default python3) and a release build of rust/.
# LATTE=<dir> (from scripts/bigrams/latte/setup.sh) adds the Thai pairs of the same training text split into words by
# LATTE-WordSeg-Lite (short units): each level's Thai table is then the dictionary pairs and the LATTE pairs, each pruned
# alike, with the counts of a pair found in both summed. The Thai pairs are built this way from model-v5 on.
set -eu
DATA=$1 WORK=$2 BASE=$3 OUT=$4; shift 4
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=$HERE/../../rust/target/release
PY=${PYTHON:-python3}
(cd "$HERE/../../rust" && cargo build --release -q -p typethai-core --bin tt-bigrams --bin tt-pack --bin tt-bigram-eval)

mkdir -p "$WORK"
[ -d "$WORK/text/train" ] || "$PY" -I "$HERE/extract.py" "$DATA" "$WORK/text"
[ -s "$WORK/raw-th.tsv" ] || "$BIN/tt-bigrams" count "$BASE" "$WORK/raw-th.tsv" "$WORK/raw-en.tsv" --vocab "$WORK"/text/train/*.txt
if [ -n "${LATTE:-}" ]; then
  "$LATTE/venv/bin/python" -I "$HERE/latte/segment.py" "$WORK/text/train" "$WORK/latte-short/train"
  # English pairs come from the plain text above only
  [ -s "$WORK/raw-th-latte.tsv" ] || "$BIN/tt-bigrams" count "$BASE" "$WORK/raw-th-latte.tsv" "$WORK/raw-en-latte.tsv" \
    --presegmented --vocab "$WORK"/latte-short/train/*.txt
fi

"$BIN/tt-bigram-eval" "$BASE" "$WORK"/text/heldout/*.txt > "$WORK/eval-base.txt"
for level in "$@"; do
  min=${level%:*} top=${level#*:} L=$WORK/min$min-top$top
  mkdir -p "$L"
  prune() {
    "$BIN/tt-bigrams" prune "$BASE" $1 "$2" "$3" --vocab --min-count "$min" $([ "$top" = all ] || echo --top "$top")
  }
  prune en "$WORK/raw-en.tsv" "$L/en-bigrams.tsv"
  if [ -n "${LATTE:-}" ]; then
    prune th "$WORK/raw-th.tsv" "$L/th-dict.tsv"
    prune th "$WORK/raw-th-latte.tsv" "$L/th-latte.tsv"
    "$BIN/tt-bigrams" merge "$L/th-bigrams.tsv" "$L/th-dict.tsv" "$L/th-latte.tsv"
  else
    prune th "$WORK/raw-th.tsv" "$L/th-bigrams.tsv"
  fi
  "$BIN/tt-pack" "$BASE" "$L/model.ttm" --thbigrams "$L/th-bigrams.tsv" --enbigrams "$L/en-bigrams.tsv"
  "$BIN/tt-bigram-eval" "$L/model.ttm" "$WORK"/text/heldout/*.txt > "$L/eval.txt"
done

first=${1%:*}; first=$WORK/min$first-top${1#*:}
cp "$first/model.ttm" "$OUT"
printf '%-14s %10s %9s %9s %8s %8s\n' level ttm_bytes th_pairs en_pairs th_ctx en_ctx
row() { printf '%-14s %10s %9s %9s %8s %8s\n' "$1" "$(wc -c < "$2" | tr -d ' ')" "$(sed -n '1s/.*th pairs \([0-9]*\).*/\1/p' "$3")" \
  "$(sed -n '1s/.*en pairs \([0-9]*\).*/\1/p' "$3")" "$(awk '/^ALL \/ th/{print $NF}' "$3")" "$(awk '/^ALL \/ en/{print $NF}' "$3")"; }
row base "$BASE" "$WORK/eval-base.txt"
for level in "$@"; do L=$WORK/min${level%:*}-top${level#*:}; row "$level" "$L/model.ttm" "$L/eval.txt"; done
echo "wrote $OUT"
