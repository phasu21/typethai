# Fresh held-out chat set (v4): Wongnai restaurant reviews (wongnai-corpus, LGPL-3.0), test_file.csv.
# Deterministic sample: every 3rd review, split into non-empty lines that contain Thai.
import csv, json, re
csv.field_size_limit(10**9)
rows = list(csv.reader(open("data/fresh/wongnai/test_file.csv", encoding="utf-8"), delimiter=";"))[1:]
lines = []
for i, r in enumerate(rows):
    if i % 3: continue
    for l in r[1].split("\n"):
        l = l.strip()
        if re.search("[\u0E00-\u0E7F]", l): lines.append(l)
json.dump(lines, open("data/fresh/wongnai_lines.json", "w"), ensure_ascii=False)
print(f"reviews={len(rows[::3])} lines={len(lines)}")
