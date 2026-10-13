// One-shot evaluation on the FRESH held-out set (never used for tuning). Refuses to run twice unless --force.
// Inputs (data/fresh/): ws_{pos,neg,neu,q}.txt (PyThaiNLP wisesight-sentiment, chat-style Thai with mixed English),
// thwiki_fresh.jsonl / enwiki_fresh.jsonl (random articles not in the train/dev/test pool).
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { loadBundle, makeTok, thaiLine, latinLines, shortSet, evaluate, fmt, type Line } from "./evallib.ts";

const OUT = "fresh-results.json";
if (existsSync(OUT) && !process.argv.includes("--force")) { console.error(`${OUT} exists: the fresh set has already been evaluated once.`); process.exit(1); }
const { bundle, det } = loadBundle();
const tok = makeTok(det);
const configs = JSON.parse(readFileSync("data/model/configs.json", "utf8"), (k, v) => (v === "-Infinity" ? -Infinity : v));
const jl = (p: string) => readFileSync(p, "utf8").split("\n").flatMap((l) => { try { return l ? [JSON.parse(l)] : []; } catch { return []; } });
const THAI = /[\u0E00-\u0E7F]/;

const ws = ["pos", "neg", "neu", "q"].flatMap((f) => readFileSync(`data/fresh/ws_${f}.txt`, "utf8").split("\n")).map((s) => s.trim()).filter((s) => THAI.test(s));
const thw = jl("data/fresh/thwiki_fresh.jsonl").flatMap((d: any) => d.text.split("\n")).filter((l: string) => THAI.test(l) && !l.startsWith("=="));
const enw = jl("data/fresh/enwiki_fresh.jsonl").flatMap((d: any) => d.text.split("\n")).filter((l: string) => l.trim() && !l.startsWith("==") && !THAI.test(l));

const sets: Record<string, Line[]> = {
  wisesightChat: ws.map((l) => thaiLine(tok, l)),
  thaiWikiFresh: thw.map((l: string) => thaiLine(tok, l)),
  englishWikiFresh: latinLines(tok, enw, "neg-en"),
  shortDictFresh: shortSet(tok, bundle.thaiWords, [...ws, ...thw].join("\n"), enw, 3),
};
const sizes = Object.fromEntries(Object.entries(sets).map(([k, v]) => [k, v.length]));
const results: any = { note: "fresh held-out set; evaluated once", lines: sizes };
const fails: any[] = [];
for (const v of ["v1", "v2", "v3"]) {
  results[v] = Object.fromEntries(Object.entries(sets).map(([k, ls]) => [k, fmt(evaluate(ls, configs[v], v === "v3" ? fails : undefined))]));
  const embedded = evaluate([...sets.wisesightChat, ...sets.thaiWikiFresh, ...sets.shortDictFresh], configs[v]);
  results[v].embeddedEnglishAll = `${embedded.ffMixed}/${embedded.negMixed} = ${(100 * embedded.ffMixedRate).toFixed(3)}%`;
}
results.failureSamplesV3 = { missed: fails.filter((f) => f.kind === "pos").slice(0, 80), falseFlips: fails.filter((f) => f.kind !== "pos").slice(0, 80) };
writeFileSync(OUT, JSON.stringify(results, null, 2));
const { failureSamplesV3, ...summary } = results;
console.log(JSON.stringify(summary, null, 2));
