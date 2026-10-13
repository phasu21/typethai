// v4 tuning on dev (wiki dev + Wisesight chat dev), then one report on the old test split.
// v1 = old model (data/model/model.v3.json) with v1 params; v4 = new model (chat-trained, collapse, brands) + guard 4.
// Usage: node scripts/bench-chat.ts [--dev-only]
import { readFileSync, writeFileSync } from "node:fs";
import { loadBundle, makeTok, sample, thaiLine, latinLines, shortSet, evaluate, fmt, type Line } from "./evallib.ts";

const DEV_ONLY = process.argv.includes("--dev-only");
const ser = (o: any) => JSON.stringify(o, (k, v) => (v === -Infinity ? "-Infinity" : v), 2);
const old = loadBundle("data/model/model.v3.json"), neu = loadBundle("data/model/model.json");
const v1 = JSON.parse(readFileSync("data/model/configs.v3.json", "utf8"), (k, v) => (v === "-Infinity" ? -Infinity : v)).v1;

function load(m: ReturnType<typeof loadBundle>, split: "dev" | "test", seed: number) {
  const tok = makeTok(m.det);
  const th = JSON.parse(readFileSync(`data/splits/th.${split}.json`, "utf8")) as { gold: string }[];
  const en = JSON.parse(readFileSync(`data/splits/en.${split}.json`, "utf8")) as string[];
  const code = sample(JSON.parse(readFileSync(`data/splits/code.${split}.json`, "utf8")) as string[], 6000);
  const out: Record<string, Line[]> = {
    wikiThai: th.map((l) => thaiLine(tok, l.gold)), english: latinLines(tok, en, "neg-en"), code: latinLines(tok, code, "neg-code"),
    shortDictSet: shortSet(tok, m.bundle.thaiWords, th.map((l) => l.gold).join("\n"), en, seed),
  };
  if (split === "dev") out.chatDev = (JSON.parse(readFileSync("data/chat/dev.json", "utf8")) as string[]).map((l) => thaiLine(tok, l));
  return out;
}
const report = (S: Record<string, Line[]>, p: any, fails?: any[]) => Object.fromEntries(Object.entries(S).map(([k, v]) => [k, fmt(evaluate(v, p, fails))]));

console.time("features");
const devOld = load(old, "dev", 1), devNew = load(neu, "dev", 1);
console.timeEnd("features");
const TARGET = 0.005, EMB = 0.0025; // embedded-English <= 0.25% on dev: v3 showed dev->fresh inflation, so leave a 2x margin under 0.5%
const pureOf = (S: Record<string, Line[]>, p: any) => evaluate([...S.english, ...S.code], p).ffPure;
const embOf = (S: Record<string, Line[]>, p: any) => {
  const a = evaluate(S.chatDev, p), b = evaluate(S.wikiThai, p), c = evaluate(S.shortDictSet, p);
  return { chat: a.ffMixedRate, wiki: b.ffMixedRate, short: c.ffMixedRate, a, b, c };
};

// Stage A: token params on new model, no context.
let A: any = null, Aobj = -1;
for (const wDict of [0, 1, 2, 4]) for (const wEnDict of [0, 1, 2, 4]) for (const wShort of [0, 1, 2, 4]) for (let th = -1; th <= 3; th += 0.5) {
  const p = { wDict, wEnDict, wShort, threshold: th, wCtx: 0, wLine: 0, guard: 4 };
  if (pureOf(devNew, p) > TARGET) continue;
  const e = embOf(devNew, p);
  if (e.chat > EMB || e.wiki > EMB || e.short > EMB) continue;
  const obj = e.a.f1 + e.b.f1;
  if (obj > Aobj + 1e-9) { Aobj = obj; A = p; }
}
if (!A) throw new Error("stage A: nothing meets constraints");
// Stage B: context + line prior with guard 4.
let B = A, Bobj = -1;
{ const e = embOf(devNew, A); Bobj = e.a.f1 + e.b.f1 + e.c.f1; }
for (const wCtx of [0, 0.25, 0.5, 1, 2]) for (const ctxWindow of [1, 2, 3]) for (const ctxFloor of [-Infinity, -2, -1.5, -1, -0.5])
  for (const wLine of [0, 0.5, 1, 2, 3]) for (const lineMinFrac of [0.6, 0.8, 1.0]) for (const lineAllowEn of [false, true]) {
    if ((wCtx === 0 && (ctxWindow !== 1 || ctxFloor !== -Infinity)) || (wLine === 0 && (lineMinFrac !== 0.6 || lineAllowEn))) continue;
    const p = { ...A, wCtx, ctxWindow, ctxFloor, wLine, lineMinFrac, lineMinFlips: 2, lineAllowEn };
    if (pureOf(devNew, p) > TARGET) continue;
    const e = embOf(devNew, p);
    if (e.chat > EMB || e.wiki > EMB || e.short > EMB) continue;
    const obj = e.a.f1 + e.b.f1 + e.c.f1;
    if (obj > Bobj + 1e-9) { Bobj = obj; B = p; }
  }
const v4 = B;
const dev = { v1: report(devOld, v1), v4: report(devNew, v4) };
console.log(ser({ v4, dev }));
writeFileSync("data/model/configs.json", ser({ v1, v4 }));
const bundle = neu.bundle; bundle.params = v4; writeFileSync("data/model/model.json", JSON.stringify(bundle));
if (DEV_ONLY) process.exit(0);

const testOld = load(old, "test", 2), testNew = load(neu, "test", 2);
const fails: any[] = [];
const test = { v1: report(testOld, v1), v4: report(testNew, v4, fails) };
console.log(ser({ test }));
writeFileSync("bench-chat-results.json", ser({ v1, v4, dev, test, testFailureSamplesV4: {
  missed: fails.filter((f) => f.kind === "pos").slice(0, 60), falseFlips: fails.filter((f) => f.kind !== "pos").slice(0, 60) } }));
