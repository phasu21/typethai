// Chat splits from Wisesight (data/chat/ws_*.txt): deterministic 80/20 train/dev by line hash.
// Note: Wisesight was the one-shot "fresh" set in v3; from v4 on it is a chat train/dev source.
import { readFileSync, writeFileSync } from "node:fs";
const THAI = /[\u0E00-\u0E7F]/;
function h(s: string) { let x = 2166136261; for (const c of s) { x ^= c.charCodeAt(0); x = Math.imul(x, 16777619) >>> 0; } return x; }
const lines = [...new Set(["pos", "neg", "neu", "q"].flatMap((f) => readFileSync(`data/chat/ws_${f}.txt`, "utf8").split("\n")).map((s) => s.trim()).filter((s) => THAI.test(s)))];
const train = lines.filter((l) => h(l) % 5 !== 0), dev = lines.filter((l) => h(l) % 5 === 0);
writeFileSync("data/chat/train.json", JSON.stringify(train)); writeFileSync("data/chat/dev.json", JSON.stringify(dev));
console.log(`chat lines: train=${train.length} dev=${dev.length}`);
