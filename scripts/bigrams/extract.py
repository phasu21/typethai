"""Turn the downloaded corpora into plain-text train and held-out files, one per source and language.

usage: python -I scripts/bigrams/extract.py <data dir> <out dir> [wiki chars per language]
Needs pyarrow for the Wikipedia parquet. Writes <out>/{train,heldout}/<source>-<lang>.txt.
A line goes to heldout when crc32 of its key (the scb English text, the message, the sentence id's text or the
article title) is 9 mod 10, so duplicates and both sides of a translation pair land in the same split.
"""
import bz2, csv, io, os, sys, zipfile, zlib

data, out = sys.argv[1], sys.argv[2]
wiki_chars = int(sys.argv[3]) if len(sys.argv) > 3 else 40_000_000
files = {}


def emit(source, lang, key, text):
    text = " ".join(text.split())
    if not text:
        return
    split = "heldout" if zlib.crc32(key.encode()) % 10 == 9 else "train"
    name = f"{split}/{source}-{lang}.txt"
    if name not in files:
        os.makedirs(os.path.join(out, split), exist_ok=True)
        files[name] = open(os.path.join(out, name), "w", encoding="utf-8")
    files[name].write(text + "\n")


# scb-mt-en-th-2020: only subsets whose upstream text is CC BY-SA compatible and whose Thai is human-written.
SCB = {"task_master_1": "taskmaster", "generated_reviews_translator": "reviews", "generated_reviews_crowd": "reviews",
       "mozilla_common_voice": "commonvoice"}
with zipfile.ZipFile(os.path.join(data, "scb/scb-mt-en-th-2020.zip")) as z:
    for sub, source in SCB.items():
        with z.open(f"scb-mt-en-th-2020/{sub}.csv") as f:
            for row in csv.DictReader(io.TextIOWrapper(f, encoding="utf-8")):
                emit(source, "th", row["en_text"], row["th_text"])
                if source != "reviews":  # the English reviews are machine-generated
                    emit(source, "en", row["en_text"], row["en_text"])

for name in ["neg", "neu", "pos", "q"]:
    with open(os.path.join(data, f"wisesight/{name}.txt"), encoding="utf-8") as f:
        for line in f:
            emit("wisesight", "th", line, line)

for lang, code in [("en", "eng"), ("th", "tha")]:
    with bz2.open(os.path.join(data, f"tatoeba/{code}_sentences.tsv.bz2"), "rt", encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) == 3:
                emit("tatoeba", lang, parts[2], parts[2])

import pyarrow.parquet as pq

for lang in ["th", "en"]:
    left = wiki_chars
    for batch in pq.ParquetFile(os.path.join(data, f"wiki/{lang}.parquet")).iter_batches(columns=["title", "text"]):
        for title, text in zip(batch.column("title").to_pylist(), batch.column("text").to_pylist()):
            for para in text.split("\n"):
                emit("wiki", lang, title, para)
            left -= len(text)
            if left <= 0:
                break
        if left <= 0:
            break

for f in files.values():
    f.close()
for name in sorted(files):
    path = os.path.join(out, name)
    print(f"{name}\t{sum(1 for _ in open(path, encoding='utf-8'))} lines\t{os.path.getsize(path)} bytes")
