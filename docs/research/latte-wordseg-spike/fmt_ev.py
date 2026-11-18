import json, sys
for l in open(sys.argv[1]):
    p, k, j = l.split(" ", 2); d = json.loads(j); t = d["tally"]; n = t["pairs"]; m = t["targets"]
    print(f"{p:12s} {k} pairs={d['pairs_in_table']:7d} next1={100*t['next_top1']/n:5.2f} next3={100*t['next_top3']/n:5.2f} comp1={100*t['ship_top1']/m:5.2f} comp3={100*t['ship_top3']/m:5.2f} uni3={100*t['uni_top3']/m:5.2f} n={n}/{m}")
