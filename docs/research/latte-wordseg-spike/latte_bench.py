"""LATTE short/long predictions for benchmark sets. usage: latte_bench.py <bench data dir> <out dir> set..."""
import json, sys, time
from pathlib import Path
from openthai_latte_wordseg_lite import Segmenter
data, out = Path(sys.argv[1]), Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
seg = Segmenter()
for s in sys.argv[3:]:
    rows = [json.loads(l) for l in open(data / f"{s}.jsonl", encoding="utf-8")]
    for unit in ("short", "long"):
        t = time.time(); toks = seg.tokenize_batch([r["text"] for r in rows], unit=unit); dt = time.time() - t
        with open(out / f"latte-{unit}.{s}.jsonl", "w", encoding="utf-8") as f:
            for r, tk in zip(rows, toks): f.write(json.dumps({"id": r["id"], "tokens": tk}, ensure_ascii=False) + "\n")
        print(s, unit, f"{sum(len(r['text']) for r in rows)/dt:.0f} chars/s", file=sys.stderr)
