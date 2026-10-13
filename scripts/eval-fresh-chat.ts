// One-shot v4 held-out evaluation on Wongnai reviews (new chat-like source, never used for tuning).
// Also reports the v3 fresh wiki samples (thwiki_fresh / enwiki_fresh) - note those were already evaluated once in v3.
// Refuses to run twice unless --force.
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { loadBundle, makeTok, thaiLine, latinLines, shortSet, evaluate, fmt, type Line } from "./evallib.ts";

const OUT = "fresh-chat-results.json";
if (existsSync(OUT) && !process.argv.includes("--force")) { console.error(`${OUT} exists: already evaluated once.`); process.exit(1); }
const cfg = JSON.parse(readFileSync("data/model/configs.json", "utf8"), (k, v) => (v === "-Infinity" ? -Infinity : v));
const jl = (p: string) => readFileSync(p, "utf8").split("\n").flatMap((l) => { try { return l ? [JSON.parse(l)] : []; } catch { return []; } });
const THAI = /[\u0E00-\u0E7F]/;
const wn: string[] = JSON.parse(readFileSync("data/fresh/wongnai_lines.json", "utf8"));
const thw = jl("data/fresh/thwiki_fresh.jsonl").flatMap((d: any) => d.text.split("\n")).filter((l: string) => THAI.test(l) && !l.startsWith("=="));
const enw = jl("data/fresh/enwiki_fresh.jsonl").flatMap((d: any) => d.text.split("\n")).filter((l: string) => l.trim() && !l.startsWith("==") && !THAI.test(l));

const results: any = { note: "Wongnai = new one-shot held-out; thaiWikiFresh/englishWikiFresh were already used once in v3", lines: { wongnai: wn.length, thaiWikiFresh: thw.length, englishWikiFresh: enw.length } };
const fails: any[] = [];
for (const [name, model] of [["v1", "data/model/model.v3.json"], ["v4", "data/model/model.json"]] as const) {
  const m = loadBundle(model), tok = makeTok(m.det);
  const sets: Record<string, Line[]> = {
    wongnai: wn.map((l) => thaiLine(tok, l)),
    wongnaiShortDict: shortSet(tok, m.bundle.thaiWords, wn.join("\n"), enw, 4),
    thaiWikiFresh: thw.map((l: string) => thaiLine(tok, l)),
    englishWikiFresh: latinLines(tok, enw, "neg-en"),
  };
  results[name] = Object.fromEntries(Object.entries(sets).map(([k, v]) => [k, fmt(evaluate(v, cfg[name], name === "v4" && k === "wongnai" ? fails : undefined))]));
}
results.failureSamplesV4Wongnai = { missed: fails.filter((f) => f.kind === "pos").slice(0, 80), falseFlips: fails.filter((f) => f.kind !== "pos").slice(0, 120) };
writeFileSync(OUT, JSON.stringify(results, null, 2));
const { failureSamplesV4Wongnai, ...summary } = results;
console.log(JSON.stringify(summary, null, 2));
