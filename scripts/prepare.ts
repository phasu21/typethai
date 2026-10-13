// Build train/dev/test splits from raw corpora. Usage: node scripts/prepare.ts [codeDir ...]
import { readFileSync, writeFileSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { join, extname } from "node:path";
import { thToEn } from "../src/keymap.ts";

const RAW = "data/raw", OUT = "data/splits";
mkdirSync(OUT, { recursive: true });

// Deterministic hash-based split so reruns are stable.
function bucket(key: string): "train" | "dev" | "test" {
  let h = 2166136261;
  for (const c of key) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619) >>> 0; }
  const r = h % 10;
  return r < 8 ? "train" : r === 8 ? "dev" : "test";
}
function readJsonl(p: string): { title: string; text: string }[] {
  return readFileSync(p, "utf8").split("\n").flatMap((l) => { try { return l ? [JSON.parse(l)] : []; } catch { return []; } });
}
const THAI = /[\u0E00-\u0E7F]/;
type Tok = { gold: string; typed: string; label: "pos" | "neg" | "skip" };

// Thai lines -> tokens with labels. A Thai-only token gets mistyped (pos); Latin tokens stay (neg).
const th: Record<string, { typed: string; gold: string; toks: Tok[] }[]> = { train: [], dev: [], test: [] };
for (const d of readJsonl(join(RAW, "thwiki.jsonl"))) {
  const b = bucket("th:" + d.title);
  for (const line of d.text.split("\n")) {
    if (!THAI.test(line) || line.startsWith("==")) continue;
    const toks: Tok[] = line.split(/\s+/).filter(Boolean).map((w) => {
      if (!THAI.test(w)) return { gold: w, typed: w, label: "neg" as const };
      const e = thToEn(w);
      return e ? { gold: w, typed: e, label: "pos" as const } : { gold: w, typed: w, label: "skip" as const };
    });
    th[b].push({ gold: line, typed: toks.map((t) => t.typed).join(" "), toks });
  }
}
// English prose.
const en: Record<string, string[]> = { train: [], dev: [], test: [] };
for (const d of readJsonl(join(RAW, "enwiki.jsonl"))) {
  const b = bucket("en:" + d.title);
  for (const line of d.text.split("\n")) if (line.trim() && !line.startsWith("==") && !THAI.test(line)) en[b].push(line);
}
// Code snippets from local source trees.
const code: Record<string, string[]> = { train: [], dev: [], test: [] };
const EXT = new Set([".ts", ".tsx", ".js", ".py", ".rs", ".go", ".sh", ".json", ".css"]);
function walk(dir: string) {
  for (const n of readdirSync(dir)) {
    if (n === "node_modules" || n === "dist" || n === "target" || n.startsWith(".")) continue;
    const p = join(dir, n), s = statSync(p);
    if (s.isDirectory()) walk(p);
    else if (EXT.has(extname(n)) && s.size < 200_000 && !n.endsWith(".min.js")) {
      const b = bucket("code:" + p);
      for (const line of readFileSync(p, "utf8").split("\n")) if (line.trim() && !THAI.test(line)) code[b].push(line);
    }
  }
}
for (const d of process.argv.slice(2)) walk(d);

for (const s of ["train", "dev", "test"]) {
  writeFileSync(join(OUT, `th.${s}.json`), JSON.stringify(th[s]));
  writeFileSync(join(OUT, `en.${s}.json`), JSON.stringify(en[s]));
  writeFileSync(join(OUT, `code.${s}.json`), JSON.stringify(code[s]));
  const pos = th[s].flatMap((l) => l.toks).filter((t) => t.label === "pos").length;
  console.log(`${s}: th lines=${th[s].length} thai pos tokens=${pos} en lines=${en[s].length} code lines=${code[s].length}`);
}
