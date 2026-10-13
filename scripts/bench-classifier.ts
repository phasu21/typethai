// v5 rules vs learned classifier. Dev: calibrate classifier threshold (embedded-English <= 0.3% on chat dev, wiki dev
// and short-word dev; pure EN+code <= 0.5%), report both on dev and the old test split.
// --fresh: one-shot on the Thai Toxicity Tweet corpus + a new English wiki sample (refuses a second run).
import { readFileSync, writeFileSync, existsSync, statSync } from "node:fs";
import { performance } from "node:perf_hooks";
import { Detector } from "../src/detector.ts";
import { lineFlips, lineProbs, type FlipModel } from "../src/classifier.ts";
import { loadBundle, makeTok, sample, thaiLine, latinLines, shortSet, evaluate, fmt, type Line } from "./evallib.ts";

const FRESH = process.argv.includes("--fresh");
const ser = (o: any) => JSON.stringify(o, (k, v) => (v === -Infinity ? "-Infinity" : v), 2);
const { bundle } = loadBundle();
const wNorm = JSON.parse(readFileSync("data/model/corrector.json", "utf8")).wNorm;
const v5 = { ...bundle.params, normalize: true, wNorm };              // rules: v4 params + guard 4 + brands + normalisation
const detRules = new Detector({ ...bundle, params: v5 });
const detLearn = new Detector({ ...bundle, brands: [], params: { ...bundle.params, normalize: true } });
const model = JSON.parse(readFileSync("data/model/flip-classifier.json", "utf8")) as FlipModel;
const tokR = makeTok(detRules), tokL = makeTok(detLearn);
const jl = (p: string) => readFileSync(p, "utf8").split("\n").flatMap((l) => { try { return l ? [JSON.parse(l)] : []; } catch { return []; } });
const THAI = /[\u0E00-\u0E7F]/;

type Src = { thai: Record<string, string[]>; english?: string[]; code?: string[]; shortFrom?: string[]; enForShort: string[]; seed: number };
function build(src: Src, tok: ReturnType<typeof makeTok>, thaiWords: string[]) {
  const S: Record<string, Line[]> = {};
  for (const [k, ls] of Object.entries(src.thai)) S[k] = ls.map((l) => thaiLine(tok, l));
  if (src.english) S.english = latinLines(tok, src.english, "neg-en");
  if (src.code) S.code = latinLines(tok, src.code, "neg-code");
  if (src.shortFrom) S.shortDictSet = shortSet(tok, thaiWords, src.shortFrom.join("\n"), src.enForShort, src.seed);
  return S;
}
const learnP = (thr: number) => ({ flipFn: (line: Line) => lineFlips(model, line.map((it) => it.d), thr) });
const report = (S: Record<string, Line[]>, p: any) => Object.fromEntries(Object.entries(S).map(([k, v]) => [k, fmt(evaluate(v, p))]));
function both(src: Src, thr: number, fails?: any[]) {
  const R = build(src, tokR, bundle.thaiWords), L = build(src, tokL, bundle.thaiWords);
  return { R, L, v5rules: report(R, v5), classifier: report(L, learnP(thr)) };
}
const wiki = (s: string) => (JSON.parse(readFileSync(`data/splits/th.${s}.json`, "utf8")) as { gold: string }[]).map((l) => l.gold);
const en = (s: string) => JSON.parse(readFileSync(`data/splits/en.${s}.json`, "utf8")) as string[];
const code = (s: string) => sample(JSON.parse(readFileSync(`data/splits/code.${s}.json`, "utf8")) as string[], 6000);

function latency(L: Record<string, Line[]>) {
  const lines = Object.values(L).flat().slice(0, 3000);
  const toks = lines.reduce((a, l) => a + l.length, 0);
  const a = performance.now(); for (const l of lines) lineFlips(model, l.map((it) => detLearn.scoreToken(it.d.token))); const learnMs = performance.now() - a;
  const b = performance.now(); for (const l of lines) detRules.convert(l.map((it) => it.d.token).join(" ")); const ruleMs = performance.now() - b;
  return { tokens: toks, classifierMsPerToken: +(learnMs / toks).toFixed(4), rulesMsPerToken: +(ruleMs / toks).toFixed(4), note: "includes scoreToken (n-gram features); single thread on the box" };
}
const sizes = () => ({ classifierWeightsBytes: statSync("data/model/flip-classifier.json").size, classifierParams: model.stage1.w.length + model.stage2.w.length + 2,
  sharedNgramModelBytes: statSync("data/model/model.json").size, brandListEntriesUsedByRulesOnly: (bundle.brands ?? []).length });

if (!FRESH) {
  const devSrc: Src = { thai: { wikiThai: wiki("dev"), chatDev: JSON.parse(readFileSync("data/chat/dev.json", "utf8")) }, english: en("dev"), code: code("dev"), shortFrom: wiki("dev"), enForShort: en("dev"), seed: 1 };
  const L = build(devSrc, tokL, bundle.thaiWords);
  // Cache probabilities once, then sweep thresholds.
  const probs = new Map<Line, number[]>(); for (const ls of Object.values(L)) for (const l of ls) probs.set(l, lineProbs(model, l.map((it) => it.d)));
  const atThr = (t: number) => ({ flipFn: (line: Line) => probs.get(line)!.map((p) => p > t) });
  let thr = 0.999, bestR = -1; const sweep: any[] = [];
  for (let t = 0.3; t <= 0.995; t += 0.005) {
    const a = evaluate(L.chatDev, atThr(t)), b = evaluate(L.wikiThai, atThr(t)), c = evaluate(L.shortDictSet, atThr(t)), pure = evaluate([...L.english, ...L.code], atThr(t));
    const ok = a.ffMixedRate <= 0.003 && b.ffMixedRate <= 0.003 && c.ffMixedRate <= 0.003 && pure.ffPure <= 0.005;
    if (Math.abs((t * 1000) % 50) < 1e-6 || ok && a.recall + b.recall + c.recall > bestR) sweep.push({ t: +t.toFixed(3), chatEmb: +(100 * a.ffMixedRate).toFixed(3), wikiEmb: +(100 * b.ffMixedRate).toFixed(3), shortEmb: +(100 * c.ffMixedRate).toFixed(3), chatRecall: +a.recall.toFixed(4), ok });
    if (ok && a.recall + b.recall + c.recall > bestR) { bestR = a.recall + b.recall + c.recall; thr = +t.toFixed(3); }
  }
  model.threshold = thr; writeFileSync("data/model/flip-classifier.json", JSON.stringify(model));
  const devRes = { v5rules: report(build(devSrc, tokR, bundle.thaiWords), v5), classifier: report(L, learnP(thr)) };
  const testSrc: Src = { thai: { wikiThai: wiki("test") }, english: en("test"), code: code("test"), shortFrom: wiki("test"), enForShort: en("test"), seed: 2 };
  const t = both(testSrc, thr);
  const res = { calibratedThreshold: thr, sweep: sweep.slice(-12), dev: devRes, oldTest: { v5rules: t.v5rules, classifier: t.classifier }, size: sizes(), latency: latency(t.L) };
  console.log(ser(res)); writeFileSync("classifier-results.json", ser(res));
} else {
  const OUT = "classifier-fresh-results.json";
  if (existsSync(OUT) && !process.argv.includes("--force")) { console.error(`${OUT} exists: already evaluated once.`); process.exit(1); }
  const tweets = JSON.parse(readFileSync("data/fresh/tweets_lines.json", "utf8")) as string[];
  const enw = jl("data/fresh/enwiki_fresh2.jsonl").flatMap((d: any) => d.text.split("\n")).filter((l: string) => l.trim() && !l.startsWith("==") && !THAI.test(l));
  const src: Src = { thai: { tweets }, english: enw, shortFrom: tweets, enForShort: enw, seed: 7 };
  const t = both(src, model.threshold);
  const fails: any[] = []; evaluate(t.L.tweets, { flipFn: (line: Line) => lineFlips(model, line.map((it) => it.d)) }, fails);
  const failsR: any[] = []; evaluate(t.R.tweets, v5, failsR);
  const pick = (f: any[], pos: boolean) => f.filter((x) => (x.kind === "pos") === pos).slice(0, 30).map((x) => `${x.token} -> ${x.candidate}${pos ? ` (gold ${x.gold})` : ""}`);
  const res = { note: "Thai Toxicity Tweet Corpus (tmu-nlp, HF parquet) + 250 new random English Wikipedia articles; one-shot", lines: { tweets: tweets.length, englishWiki: enw.length },
    threshold: model.threshold, v5rules: t.v5rules, classifier: t.classifier, size: sizes(), latency: latency(t.L),
    failures: { classifier: { falseFlips: pick(fails, false), missed: pick(fails, true) }, v5rules: { falseFlips: pick(failsR, false), missed: pick(failsR, true) } } };
  console.log(ser(res)); writeFileSync(OUT, ser(res));
}
