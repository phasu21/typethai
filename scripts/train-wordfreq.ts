// Dictionary word frequencies from TRAIN text only (wiki train + Wisesight chat train), via DP segmentation.
import { readFileSync, writeFileSync } from "node:fs";
import { segment } from "../src/corrector.ts";
import { normalizeThai } from "../src/normalize.ts";
const words = new Set(readFileSync("data/raw/words_th.txt", "utf8").split("\n").map((s) => s.trim()).filter(Boolean));
const th = JSON.parse(readFileSync("data/splits/th.train.json", "utf8")) as { gold: string }[];
const chat = JSON.parse(readFileSync("data/chat/train.json", "utf8")) as string[];
const freq: Record<string, number> = {};
let n = 0;
for (const line of [...th.map((l) => l.gold), ...chat]) for (const tok of line.split(/\s+/)) {
  const runs = tok.match(/[\u0E00-\u0E7F]+/g) ?? [];
  for (const r of runs) for (const w of segment(normalizeThai(r), (x) => words.has(x))) if (words.has(w)) { freq[w] = (freq[w] ?? 0) + 1; n++; }
}
writeFileSync("data/model/wordfreq.json", JSON.stringify(freq));
console.log(`word tokens=${n} types=${Object.keys(freq).length}`);
