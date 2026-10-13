// Dev tuning + test report. Stage 1: token params (=v1). Stage 2: v2 neighbour context (guard 2).
// Stage 3: v3 = guard 3 + neighbour context + whole-line prior. All selection is on dev only.
// Usage: node scripts/bench.ts [--dev-only] [--max-code-lines N]
import { readFileSync, writeFileSync } from "node:fs";
import { loadBundle, makeTok, sample, thaiLine, latinLines, shortSet, evaluate, fmt, type Line } from "./evallib.ts";

const MAXCODE = Number(process.argv[process.argv.indexOf("--max-code-lines") + 1]) || 6000;
const DEV_ONLY = process.argv.includes("--dev-only");
const { bundle, det } = loadBundle();
const tok = makeTok(det);

function load(split: string, seed: number) {
  const th = JSON.parse(readFileSync(`data/splits/th.${split}.json`, "utf8")) as { gold: string }[];
  const en = JSON.parse(readFileSync(`data/splits/en.${split}.json`, "utf8")) as string[];
  const code = sample(JSON.parse(readFileSync(`data/splits/code.${split}.json`, "utf8")) as string[], MAXCODE);
  const thLines: Line[] = th.map((l) => thaiLine(tok, l.gold));
  return { thLines, enLines: latinLines(tok, en, "neg-en"), codeLines: latinLines(tok, code, "neg-code"),
    short: shortSet(tok, bundle.thaiWords, th.map((l) => l.gold).join("\n"), en, seed) };
}
type Set_ = ReturnType<typeof load>;
const all = (s: Set_) => [...s.thLines, ...s.enLines, ...s.codeLines];
function report(s: Set_, p: any, fails?: any[]) {
  return { wikiThai: fmt(evaluate(s.thLines, p, fails)), english: fmt(evaluate(s.enLines, p, fails)), code: fmt(evaluate(s.codeLines, p, fails)),
    shortDictSet: fmt(evaluate(s.short, p, fails)), pureCombinedFalseFlip: `${(100 * evaluate([...s.enLines, ...s.codeLines], p).ffPure).toFixed(3)}%` };
}

console.time("features");
const dev = load("dev", 1);
console.timeEnd("features");
const TARGET = 0.005;

// Stage 1.
const ok: { p: any; r: any }[] = [];
for (const wDict of [0, 1, 2, 3, 4, 6]) for (const wEnDict of [0, 1, 2, 3, 5]) for (const wShort of [0, 1, 2, 4, 6]) for (let th = -2; th <= 6; th += 0.25) {
  const p = { wDict, wEnDict, wShort, threshold: th, wCtx: 0 };
  const r = evaluate(all(dev), p);
  if (r.ffPure <= TARGET && r.ffMixedRate <= 0.02) ok.push({ p, r });
}
const maxF1 = Math.max(...ok.map((o) => o.r.f1));
let base = ok[0];
for (const o of ok) if (o.r.f1 >= maxF1 - 0.005 && (o.r.ffPure < base.r.ffPure || base.r.f1 < maxF1 - 0.005 || (o.r.ffPure === base.r.ffPure && o.r.f1 > base.r.f1))) base = o;
const v1 = { ...base.p, wCtx: 0, wLine: 0 };
const baseMixed = evaluate(dev.thLines, v1).ffMixed, baseShortMixed = evaluate(dev.short, v1).ffMixed;

function tune(grid: any[], label: string) {
  let best: any = null, bestObj = -1;
  for (const p of grid) {
    const r = evaluate(all(dev), p), sh = evaluate(dev.short, p);
    const okc = r.ffPure <= TARGET && r.ffMixed <= baseMixed && sh.ffMixed <= baseShortMixed;
    if (DEV_ONLY && process.argv.includes("--verbose")) console.log(label, JSON.stringify({ ...p, f1: +r.f1.toFixed(4), shR: +sh.recall.toFixed(4), ffMix: r.ffMixed, shFF: sh.ffMixed, ok: okc }));
    if (okc && r.f1 + sh.f1 > bestObj + 1e-9) { bestObj = r.f1 + sh.f1; best = p; }
  }
  return best ?? v1;
}
const floors = [-Infinity, -3, -2, -1.5, -1, -0.5];
const g2: any[] = [];
for (const wCtx of [0.25, 0.5, 0.75, 1, 1.5, 2, 2.5, 3, 4]) for (const ctxWindow of [1, 2, 3]) for (const ctxFloor of floors) g2.push({ ...v1, guard: 2, wCtx, ctxWindow, ctxFloor });
const v2 = tune(g2, "v2");
const g3: any[] = [];
for (const wCtx of [0, 0.25, 0.5, 1, 2]) for (const ctxWindow of [1, 2, 3]) for (const ctxFloor of [-Infinity, -2, -1.5, -1])
  for (const wLine of [0, 1, 2, 3, 4, 5]) for (const lineMinFrac of [0.6, 0.8, 0.9, 1.0]) for (const lineAllowEn of [false, true]) {
    if ((wCtx === 0 && (ctxWindow !== 1 || ctxFloor !== -Infinity)) || (wLine === 0 && (lineMinFrac !== 0.6 || lineAllowEn))) continue;
    g3.push({ ...v1, guard: 3, wCtx, ctxWindow, ctxFloor, wLine, lineMinFrac, lineMinFlips: 2, lineAllowEn });
  }
const v3 = tune(g3, "v3");
const configs = { v1, v2, v3 };
const devReport = { v1: report(dev, v1), v2: report(dev, v2), v3: report(dev, v3) };
console.log(JSON.stringify({ configs, dev: devReport }, (k, v) => (v === -Infinity ? "-Infinity" : v), 2));
if (DEV_ONLY) process.exit(0);

// Test split: evaluated once per config (note: this split was already looked at in v1/v2 rounds).
const test = load("test", 2);
const fails: any[] = [];
const testReport = { v1: report(test, v1), v2: report(test, v2), v3: report(test, v3, fails) };
console.log(JSON.stringify({ test: testReport }, null, 2));
const ser = (o: any) => JSON.stringify(o, (k, v) => (v === -Infinity ? "-Infinity" : v), 2);
writeFileSync("data/model/configs.json", ser(configs));
writeFileSync("bench-results.json", ser({ configs, dev: devReport, test: testReport, testFailureSamplesV3: {
  missed: fails.filter((f) => f.kind === "pos").slice(0, 60), falseFlips: fails.filter((f) => f.kind !== "pos").slice(0, 60) } }));
bundle.params = v3; writeFileSync("data/model/model.json", JSON.stringify(bundle));
