"""Split the Thai training text into words with LATTE (short units), words joined by U+200B, for
`tt-bigrams count --presegmented`. Files already written are kept, so a rerun only does what is missing.

usage: <latte dir>/venv/bin/python -I scripts/bigrams/latte/segment.py <text dir> <out dir>
Reads <text dir>/*-th.txt (extract.py's train split), writes <out dir>/<same name>.
"""
import sys
import time
from pathlib import Path

from openthai_latte_wordseg_lite import Segmenter

src, dst = Path(sys.argv[1]), Path(sys.argv[2])
dst.mkdir(parents=True, exist_ok=True)
seg = Segmenter(unit="short")
for f in sorted(src.glob("*-th.txt")):
    out = dst / f.name
    if out.exists():
        continue
    part = out.with_suffix(".part")
    start, chars = time.time(), 0
    with open(f, encoding="utf-8") as fi, open(part, "w", encoding="utf-8") as fo:
        batch = []

        def flush():
            for words in seg.tokenize_batch(batch):
                fo.write("​".join(words) + "\n")
            batch.clear()

        for line in fi:
            line = line.rstrip("\n")
            chars += len(line)
            batch.append(line)
            if len(batch) >= 2000:
                flush()
        flush()
    part.rename(out)
    print(f"{f.name}: {chars} chars, {chars / max(time.time() - start, 1e-9):.0f} chars/s", flush=True)
