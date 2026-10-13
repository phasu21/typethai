// Train n-gram models + vocab from the train split. Writes data/model/model.json.
import { readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { trainNgram, prune } from "../src/ngram.ts";
import type { ModelBundle } from "../src/detector.ts";

const th = JSON.parse(readFileSync("data/splits/th.train.json", "utf8")) as { toks: { gold: string; label: string }[] }[];
const en = JSON.parse(readFileSync("data/splits/en.train.json", "utf8")) as string[];
const code = JSON.parse(readFileSync("data/splits/code.train.json", "utf8")) as string[];

// v4: also train on chat-train Thai tokens (data/chat/train.json) and mine a brand/abbreviation list.
const CHAT = existsSync("data/chat/train.json") && !process.argv.includes("--no-chat");
const chat: string[] = CHAT ? JSON.parse(readFileSync("data/chat/train.json", "utf8")) : [];
const THAI = /[\u0E00-\u0E7F]/;
const chatToks = chat.flatMap((l) => l.split(/\s+/).filter(Boolean));
const thaiTokens = [...th.flatMap((l) => l.toks.filter((t) => t.label === "pos").map((t) => t.gold)),
  ...chatToks.filter((w) => THAI.test(w) && /^[\u0E00-\u0E7F]+$/.test(w))];
// Brands: ASCII tokens inside Thai chat-train lines (real typed English), core lowercased, freq >= 2, plus hand list.
const bfreq = new Map<string, number>();
for (const w of chatToks) {
  if (!/^[\x21-\x7E]+$/.test(w)) continue;
  const c = w.replace(/^[()\[\]{}"'`.,;:!?<>]+|[()\[\]{}"'`.,;:!?<>]+$/g, "").toLowerCase();
  if (/^[a-z][a-z0-9&\-]*$/.test(c)) bfreq.set(c, (bfreq.get(c) ?? 0) + 1);
}
const hand = existsSync("data/brands_hand.txt") ? readFileSync("data/brands_hand.txt", "utf8").split("\n").map((s) => s.trim().toLowerCase()).filter(Boolean) : [];
const brands = CHAT ? [...new Set([...hand, ...[...bfreq].filter(([, c]) => c >= 2).map(([w]) => w)])] : [];
const enTokens = [...en, ...code].flatMap((l) => l.split(/\s+/).filter(Boolean));
const thai = prune(trainNgram(thaiTokens), 2);
const english = prune(trainNgram(enTokens), 2);

const freq = new Map<string, number>();
for (const t of en.flatMap((l) => l.split(/\s+/))) { // prose only: code identifiers pollute the vocab
  const w = t.toLowerCase().replace(/^[^a-z]+|[^a-z]+$/g, "");
  if (w) freq.set(w, (freq.get(w) ?? 0) + 1);
}
const englishWords = [...freq].filter(([, c]) => c >= 3).map(([w]) => w);
const thaiWords = readFileSync("data/raw/words_th.txt", "utf8").split("\n").map((s) => s.trim()).filter(Boolean);

const bundle: ModelBundle = { thai, english, thaiWords, englishWords, params: { wDict: 2, wEnDict: 2, wShort: 0, threshold: 1 }, brands, collapse: CHAT };
mkdirSync("data/model", { recursive: true });
writeFileSync("data/model/model.json", JSON.stringify(bundle));
console.log(`brands=${brands.length} chat=${CHAT} thai tokens=${thaiTokens.length} en tokens=${enTokens.length} thaiNgrams=${Object.keys(thai.counts).length} enNgrams=${Object.keys(english.counts).length} enWords=${englishWords.length} thaiWords=${thaiWords.length}`);
