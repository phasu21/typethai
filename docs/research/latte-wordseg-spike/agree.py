"""Boundary agreement dict vs LATTE on Thai-internal cuts. usage: agree.py <dict jsonl> <latte zw file> <dict words file>"""
import json, sys
def thai(c): return "ก" <= c <= "๎" and not ("๐" <= c <= "๙")
words = set(l.rstrip("\n") for l in open(sys.argv[3], encoding="utf-8"))
tp = nd = nl = 0; thai_chars = dict_cov = latte_cov = 0
for dl, ll in zip(open(sys.argv[1], encoding="utf-8"), open(sys.argv[2], encoding="utf-8")):
    dt = json.loads(dl)["tokens"]; lt = ll.rstrip("\n").split("​")
    text = "".join(dt)
    if "".join(lt) != text: continue
    def cuts(toks):
        out, p = set(), 0
        for t in toks[:-1]:
            p += len(t)
            if thai(text[p - 1]) and thai(text[p]): out.add(p)
        return out
    a, b = cuts(dt), cuts(lt); tp += len(a & b); nd += len(a); nl += len(b)
    for toks, k in ((dt, 0), (lt, 1)):
        for t in toks:
            if all(thai(c) for c in t):
                if k == 0: thai_chars += len(t)
                if t in words:
                    if k == 0: dict_cov += len(t)
                    else: latte_cov += len(t)
print(json.dumps({"cut_agreement_F1": round(200 * tp / (nd + nl), 1), "dict_cuts": nd, "latte_cuts": nl,
                  "thai_chars_in_dict_words_dict": round(100 * dict_cov / thai_chars, 1), "thai_chars_in_dict_words_latte": round(100 * latte_cov / thai_chars, 1)}))
