"""Partial-word test. usage: latte_tail.py <tail.jsonl> <vocab.txt> <every N>"""
import json, sys, bisect, collections
from openthai_latte_wordseg_lite import Segmenter
cases = [json.loads(l) for i, l in enumerate(open(sys.argv[1], encoding="utf-8")) if i % int(sys.argv[3]) == 0]
vocab = sorted(l.strip() for l in open(sys.argv[2], encoding="utf-8") if l.strip())
def extends(p):  # a vocab word strictly longer than p starts with p (Completer::extends)
    i = bisect.bisect_left(vocab, p)
    while i < len(vocab) and vocab[i].startswith(p):
        if len(vocab[i]) > len(p): return True
        i += 1
    return False
seg = Segmenter(batch_chars=0)
c = collections.Counter()
def cls(start, h): return "exact" if start == h else ("tail_longer" if start < h else "tail_shorter")
for r in cases:
    run, h = r["run"], r["head_len"]
    key = ("L3" if r["len"] == 3 else "Lfull-1", "headdict" if r["head_in_dict"] else "headOOV")
    c[key + ("n",)] += 1
    at = r["tt_at"]; c[key + ("thai_tail", "none" if at is None else cls(at, h))] += 1
    for unit in ("short", "long"):
        toks = seg.tokenize(run, unit=unit)
        starts = []; p = 0
        for t in toks: starts.append(p); p += len(t)
        c[key + (unit, cls(starts[-1], h))] += 1
        c[key + (unit, "cut_at_head")] += h in starts
        # hybrid: longest tail starting at a LATTE cut that a vocab word extends
        cand = [s for s in starts if extends(run[s:])]
        c[key + (unit + "+vocab", "none" if not cand else cls(min(cand), h))] += 1
        fb = cls(at, h) if at is not None else ("none" if not cand else cls(min(cand), h))
        c[key + ("tt>" + unit, fb)] += 1
out = {}
for k in sorted({k[:2] for k in c}):
    n = c[k + ("n",)]
    out["/".join(k)] = {"n": n, **{f"{m}:{res}": round(100 * c[k + (m, res)] / n, 1) for m in ("thai_tail", "short", "long", "short+vocab", "long+vocab", "tt>short", "tt>long") for res in ("exact", "tail_longer", "tail_shorter", "none", "cut_at_head") if c[k + (m, res)]}}
print(json.dumps(out, indent=1, ensure_ascii=False))
