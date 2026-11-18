"""Segment text files with LATTE; tokens joined by U+200B. usage: latte_corpus.py <unit> <in dir> <out dir>"""
import sys, time
from pathlib import Path
from openthai_latte_wordseg_lite import Segmenter
unit, src, dst = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
seg = Segmenter(unit=unit)
for f in sorted(src.rglob("*.txt")):
    o = dst / f.relative_to(src); o.parent.mkdir(parents=True, exist_ok=True)
    if o.exists(): continue
    t, n = time.time(), 0
    with open(f, encoding="utf-8") as fi, open(str(o) + ".part", "w", encoding="utf-8") as fo:
        buf = []
        def flush():
            for toks in seg.tokenize_batch(buf): fo.write("​".join(toks) + "\n")
            buf.clear()
        for line in fi:
            line = line.rstrip("\n"); n += len(line); buf.append(line)
            if len(buf) >= 2000: flush()
        if buf: flush()
    Path(str(o) + ".part").rename(o)
    print(unit, f, f"{n/(time.time()-t):.0f} chars/s", flush=True)
