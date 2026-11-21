#!/bin/sh
# model-v6: model-v5 with its English sections rebuilt so contractions (you're, it's, they're, don't) are words.
# usage: scripts/bigrams/model-v6.sh <data dir> <work dir> <model-v5.ttm> <out.ttm>
#   <data dir>  downloads from scripts/bigrams/fetch.sh (the same corpora as model-v5)
#   <work dir>  extracted text (reused when present), contraction frequencies, pair files, eval reports
# Needs a Python with pyarrow for the Wikipedia parquet (PYTHON, default python3).
# Steps, as build.sh does them for the English pairs at level 3:all (model-v4/v5's English level):
#   1. section 11 gets the contractions seen 10+ times in the English training runs (`tt-bigrams contractions`, scaled
#      to the frequency list), so `--vocab` keeps their pairs; every other word keeps its frequency;
#   2. section 13 is counted again from all the training text: corpus::word_runs keeps apostrophe words whole now, so
#      runs no longer end at a contraction;
#   3. the Thai sections, the dictionary, the word lists and the classifier stay exactly as in model-v5 (the Thai pairs
#      are the LATTE union, unchanged).
set -eu
DATA=$1 WORK=$2 V5=$3 OUT=$4
HERE=$(cd "$(dirname "$0")" && pwd)
BIN=$HERE/../../rust/target/release
PY=${PYTHON:-python3}
(cd "$HERE/../../rust" && cargo build --release -q -p typethai-core --bin tt-bigrams --bin tt-pack --bin tt-bigram-eval --bin tt-confusion-eval)

mkdir -p "$WORK"
[ -d "$WORK/text/train" ] || "$PY" -I "$HERE/extract.py" "$DATA" "$WORK/text"
"$BIN/tt-bigrams" contractions "$V5" "$WORK/contractions.tsv" --min-count 10 "$WORK"/text/train/*-en.txt
"$BIN/tt-pack" "$V5" "$WORK/base-v6.ttm" --enfreq-add "$WORK/contractions.tsv"
"$BIN/tt-bigrams" count "$WORK/base-v6.ttm" "$WORK/raw-th.tsv" "$WORK/raw-en.tsv" --vocab --en-only "$WORK"/text/train/*.txt
"$BIN/tt-bigrams" prune "$WORK/base-v6.ttm" en "$WORK/raw-en.tsv" "$WORK/en-bigrams.tsv" --vocab --min-count 3
"$BIN/tt-pack" "$WORK/base-v6.ttm" "$OUT" --enbigrams "$WORK/en-bigrams.tsv"

"$BIN/tt-bigram-eval" "$V5" "$WORK"/text/heldout/*.txt > "$WORK/eval-v5.txt"
"$BIN/tt-bigram-eval" "$OUT" "$WORK"/text/heldout/*.txt > "$WORK/eval-v6.txt"
printf '%-6s %10s %9s\n' model ttm_bytes en_pairs
for m in "$V5" "$OUT"; do printf '%-6s %10s %9s\n' "$(basename "$m")" "$(wc -c < "$m" | tr -d ' ')" "$(sed -n '1s/.*en pairs \([0-9]*\).*/\1/p' "$WORK/eval-$([ "$m" = "$V5" ] && echo v5 || echo v6).txt")"; done
echo "wrote $OUT"
