# v5 fresh held-out: Wongnai w_review_train.csv (a different file from the v4 held-out test_file.csv).
# Deterministic sample: every 20th review, lines containing Thai.
import csv, json, re
csv.field_size_limit(10**9)
rows = list(csv.reader(open("data/fresh/wongnai/w_review_train.csv", encoding="utf-8"), delimiter=";"))
lines = []
for i, r in enumerate(rows):
    if i % 20 or not r: continue
    for l in r[0].split("\n"):
        l = l.strip()
        if re.search("[\u0E00-\u0E7F]", l): lines.append(l)
json.dump(lines, open("data/fresh/wongnai2_lines.json", "w"), ensure_ascii=False)
print(f"rows={len(rows)} sampled_reviews={len(rows[::20])} lines={len(lines)}")
