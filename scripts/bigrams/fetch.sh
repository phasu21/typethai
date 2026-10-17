#!/bin/sh
# Download the corpora for the word-pair (bigram) sections into $1, one subdirectory per source.
# Files already downloaded are kept, so a rerun only fetches what is missing.
# usage: scripts/bigrams/fetch.sh <data dir>
set -eu
D=$1
mkdir -p "$D/scb" "$D/wisesight" "$D/tatoeba" "$D/wiki" "$D/vistec"
get() {
  [ -s "$2" ] && return 0
  curl -fsSL --retry 5 --retry-all-errors --speed-limit 10000 --speed-time 30 -o "$2.part" "$1"
  mv "$2.part" "$2"
}

# scb-mt-en-th-2020 v1.0 (CC BY-SA 4.0; the Common Voice pairs CC0).
get https://github.com/vistec-AI/dataset-releases/releases/download/scb-mt-en-th-2020_v1.0/scb-mt-en-th-2020.zip "$D/scb/scb-mt-en-th-2020.zip"

# Wisesight Sentiment (CC0), raw messages, pinned commit.
WS=c1d063649abc3a6870b5fd1aae26cd1c64bdde7b
for f in neg neu pos q; do
  get "https://raw.githubusercontent.com/PyThaiNLP/wisesight-sentiment/$WS/$f.txt" "$D/wisesight/$f.txt"
done

# Tatoeba sentences (CC BY 2.0 FR). The export is regenerated weekly, so SHA256SUMS records the one used.
for l in eng tha; do
  get "https://downloads.tatoeba.org/exports/per_language/$l/${l}_sentences.tsv.bz2" "$D/tatoeba/${l}_sentences.tsv.bz2"
done

# Wikipedia 2023-11-01 dump as parquet (wikimedia/wikipedia on Hugging Face, pinned revision). One shard per language.
HF=https://huggingface.co/datasets/wikimedia/wikipedia/resolve/b04c8d1ceb2f5cd4588862100d08de323dccfbaa
get "$HF/20231101.th/train-00002-of-00003.parquet" "$D/wiki/th.parquet"
get "$HF/20231101.en/train-00040-of-00041.parquet" "$D/wiki/en.parquet"

# VISTEC-TP-TH-2021 (CC BY-SA 3.0): Thai tweets 2017-2019, pinned commit of the OSKut repository.
VT=be9e16ac2dc94e2a4bdae562347cc8ecb38ae125
get "https://raw.githubusercontent.com/mrpeerat/OSKut/$VT/VISTEC-TP-TH-2021/VISTEC-TP-TH-2021.csv" "$D/vistec/VISTEC-TP-TH-2021.csv"

# scripts/bigrams/SHA256SUMS holds the files the shipped pairs were built from (downloaded 2026-10-15).
sum=$(command -v sha256sum || echo "shasum -a 256")
(cd "$D" && $sum scb/*.zip wisesight/*.txt tatoeba/*.bz2 wiki/*.parquet vistec/*.csv) > "$D/SHA256SUMS"
cat "$D/SHA256SUMS"
