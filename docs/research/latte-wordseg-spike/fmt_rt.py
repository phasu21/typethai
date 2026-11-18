import json, sys
for l in sys.stdin:
    d = json.loads(l); t = d["tally"]; n = t["cases"]; g = lambda k: 100 * t.get(k, 0) / n
    print(f"{d['pairs'].split('/')[-1]:14s} prev={d['prev']:6s} n={n} prev=gold {g('prev_eq_gold'):5.1f} has_prev {g('has_prev'):5.1f} | next top1 {g('next_top1'):5.2f} top3 {g('next_top3'):5.2f} | compl top1 {g('ship_top1'):5.2f} top3 {g('ship_top3'):5.2f}")
