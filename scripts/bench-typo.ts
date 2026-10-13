// v5 typo benchmark. Dev tuning (wiki dev + Wisesight chat dev) -> test split report; --fresh runs the one-shot
// Wongnai w_review_train held-out (refuses a second run unless --force).
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { performance } from "node:perf_hooks";
import { Corrector, DEFAULT_CORRECTOR, type CorrectorParams } from "../src/corrector.ts";
import { Detector } from "../src/detector.ts";
import { normalizeThai } from "../src/normalize.ts";
import { loadBundle, makeTok, thaiLine, evaluate, fmt, type Line } from "./evallib.ts";
import { syntheticSet, realTokens, cache, decide, OPS, type Cached } from "./typolib.ts";

const FRESH = process.argv.includes("--fresh");
const ser = (o: any) => JSON.stringify(o, null, 2);
const { bundle } = loadBundle();
const freq = JSON.parse(readFileSync("data/model/wordfreq.json", "utf8"));
let t0 = performance.now();
const corr = new Corrector(bundle.thaiWords, freq, bundle.thai);
const buildMs = performance.now() - t0;
const has = (w: string) => corr.has(w);
const pct = (a: number, b: number) => `${a}/${b} = ${((100 * a) / Math.max(1, b)).toFixed(2)}%`;

const wikiLines = (s: string) => (JSON.parse(readFileSync(`data/splits/th.${s}.json`, "utf8")) as { gold: string }[]).map((l) => l.gold);
function build(lines: string[], seed: number, nSyn = 2000, nReal = 3000) {
  const syn = syntheticSet(lines, has, nSyn, seed).map((s) => ({ ...s, c: cache(corr, bundle.thai, s.typo) }));
  const real = realTokens(lines, has, nReal);
  return { syn, real: { ...real, c: real.sample.map((t) => cache(corr, bundle.thai, t)) } };
}
function score(S: ReturnType<typeof build>, p: CorrectorParams) {
  let top1 = 0, top3 = 0, appliedOk = 0, appliedWrong = 0;
  const byOp: Record<string, { n: number; top1: number }> = {};
  for (const s of S.syn) {
    const d = decide(s.c, p, has);
    const b = (byOp[s.op] ??= { n: 0, top1: 0 }); b.n++;
    if (d.ranked[0] === s.gold) { top1++; b.top1++; }
    if (d.ranked.slice(0, 3).includes(s.gold)) top3++;
    if (d.apply !== s.c.normalized) { if (d.apply === s.gold) appliedOk++; else appliedWrong++; }
  }
  let falseCorr = 0;
  for (const x of S.real.c) if (decide(x, p, has).apply !== x.normalized) falseCorr++;
  const n = S.syn.length;
  return { n, top1: top1 / n, top3: top3 / n, autoFixed: appliedOk / n, autoWrong: appliedWrong / n, falseCorr: falseCorr / Math.max(1, S.real.c.length),
    raw: { top1, top3, appliedOk, appliedWrong, falseCorr, real: S.real.c.length, realOovOfAll: `${S.real.oovTokens}/${S.real.totalTokens}` },
    byOp: Object.fromEntries(Object.entries(byOp).map(([k, v]) => [k, `${v.top1}/${v.n} = ${(v.top1 / v.n).toFixed(3)}`])) };
}
const show = (r: ReturnType<typeof score>) => ({ syntheticTypos: r.n, top1: pct(r.raw.top1, r.n), top3: pct(r.raw.top3, r.n), top1ByOp: r.byOp,
  autoApplied: { correct: pct(r.raw.appliedOk, r.n), wrong: pct(r.raw.appliedWrong, r.n) },
  falseCorrectionOnRealTokens: pct(r.raw.falseCorr, r.raw.real), realTokensOovShare: r.raw.realOovOfAll });

// Detection recall before/after normalisation (v4 params vs v4 + normalize), plus natural orthographic-typo slice.
const normDet = new Detector({ ...bundle, params: { ...bundle.params, normalize: true } });
const tokPlain = makeTok(new Detector(bundle)), tokNorm = makeTok(normDet);
function linesFor(lines: string[], norm: boolean) {
  const L: Line[] = lines.map((l) => thaiLine(norm ? tokNorm : tokPlain, l));
  return { L, natural: L.map((l) => l.map((it) => it.kind === "pos" && normalizeThai(it.gold) !== it.gold ? it : { ...it, kind: "skip" as const })) };
}
function detection(lines: string[], wNorm: number) {
  const out: any = {};
  for (const [name, norm] of [["v4", false], ["v4+normalize", true]] as const) {
    const p = { ...bundle.params, normalize: norm, wNorm: norm ? wNorm : 0 };
    const { L, natural } = linesFor(lines, norm);
    const all = evaluate(L, p), nat = evaluate(natural, p);
    out[name] = { ...fmt(all), naturalOrthoTypoTokens: `${nat.tp}/${nat.pos} = ${nat.recall.toFixed(4)}` };
  }
  return out;
}
// Natural orthographic typos whose normalised form is a dictionary word: does the corrector produce it?
function naturalOrtho(lines: string[], p: CorrectorParams) {
  const toks = new Set<string>();
  for (const l of lines) for (const t of l.split(/\s+/)) for (const run of t.match(/[\u0E00-\u0E7F]+/g) ?? []) if (normalizeThai(run) !== run && has(normalizeThai(run))) toks.add(run);
  let ok = 0; for (const t of toks) if (decide(cache(corr, bundle.thai, t), p, has).apply === normalizeThai(t)) ok++;
  return { uniqueTokens: toks.size, fixed: pct(ok, toks.size), examples: [...toks].slice(0, 12).map((t) => `${t}->${normalizeThai(t)}`) };
}
function latency(lines: string[]) {
  const det = new Detector({ ...bundle, params: { ...bundle.params, normalize: true } });
  const toks = lines.flatMap((l) => l.split(/\s+/)).filter((t) => /[\u0E00-\u0E7F]/.test(t)).slice(0, 5000);
  const typed = lines.flatMap((l) => thaiLine(tokPlain, l)).filter((it) => it.kind !== "skip").map((it) => it.d.token).slice(0, 5000); // mistyped forms + embedded English
  const ms = (f: () => void) => { const a = performance.now(); f(); return performance.now() - a; };
  const detT = typed.map((t) => ms(() => det.scoreToken(t)));
  const oov = toks.filter((t) => /^[\u0E00-\u0E7F]+$/.test(t) && [...t].length <= 12 && !has(normalizeThai(t))).slice(0, 2000);
  const corT = oov.map((t) => ms(() => corr.suggest(t)));
  const q = (a: number[], f: number) => +[...a].sort((x, y) => x - y)[Math.floor(f * (a.length - 1))].toFixed(4);
  const mean = (a: number[]) => +(a.reduce((x, y) => x + y, 0) / a.length).toFixed(4);
  return { note: "ms per token, single thread, Node 24 on the box", detectScoreToken: { n: detT.length, mean: mean(detT), p50: q(detT, 0.5), p95: q(detT, 0.95), p99: q(detT, 0.99) },
    correctorSuggestOov: { n: corT.length, mean: mean(corT), p50: q(corT, 0.5), p95: q(corT, 0.95), p99: q(corT, 0.99) },
    correctorIndexBuildMs: Math.round(buildMs), heapUsedMB: Math.round(process.memoryUsage().heapUsed / 1e6) };
}

const tunedPath = "data/model/corrector.json";
if (!FRESH) {
  const devLines = [...wikiLines("dev"), ...(JSON.parse(readFileSync("data/chat/dev.json", "utf8")) as string[])];
  console.time("dev features"); const dev = build(devLines, 11); console.timeEnd("dev features");
  // Stage 1: ranking weights by dev top-1.
  let best: CorrectorParams = DEFAULT_CORRECTOR, bTop = -1;
  for (const alpha of [1, 2, 3, 4, 6, 8]) for (const beta of [0, 0.5, 1, 2]) for (const gamma of [0, 0.25, 0.5, 1]) {
    const p = { ...DEFAULT_CORRECTOR, alpha, beta, gamma };
    const r = score(dev, p); if (r.top1 > bTop + 1e-9) { bTop = r.top1; best = p; }
  }
  // Stage 2: keepBias/tau maximise correct auto-fixes s.t. dev false-correction <= 0.5% and wrong auto-fixes <= 2%.
  let best2 = { ...best, tau: 1.01 }, bFix = -1;
  for (const keepBias of [-4, -2, -1, 0, 1, 2, 3, 4, 6, 8]) for (const tau of [0.5, 0.6, 0.7, 0.8, 0.85, 0.9, 0.95, 0.98, 0.99]) {
    const p = { ...best, keepBias, tau }, r = score(dev, p);
    if (r.falseCorr <= 0.005 && r.autoWrong <= 0.02 && r.autoFixed > bFix) { bFix = r.autoFixed; best2 = p; }
  }
  
  const devRep = show(score(dev, best2));
  console.log(ser({ params: best2, dev: devRep }));
  // Detection: tune the normalisation penalty wNorm on dev (chat dev + wiki dev): maximise natural-typo recall + F1
  // subject to embedded-English false flips <= 0.25% on each dev source (same margin as v4).
  const chatDevL = linesFor(JSON.parse(readFileSync("data/chat/dev.json", "utf8")), true), wikiDevL = linesFor(wikiLines("dev"), true);
  let wNorm = 1e9, bObj = -1; const sweep: any[] = [];
  for (const w of [0, 0.25, 0.5, 1, 1.5, 2, 3, 5, 10]) {
    const p = { ...bundle.params, normalize: true, wNorm: w };
    const a = evaluate(chatDevL.L, p), b = evaluate(wikiDevL.L, p), na = evaluate(chatDevL.natural, p);
    sweep.push({ wNorm: w, chatEmb: a.ffMixedRate, wikiEmb: b.ffMixedRate, natRecall: na.recall, f1: a.f1 });
    if (a.ffMixedRate <= 0.0025 && b.ffMixedRate <= 0.0025 && na.recall + a.f1 > bObj) { bObj = na.recall + a.f1; wNorm = w; }
  }
  console.log(ser({ wNormSweepDev: sweep, wNorm }));
  const testLines = wikiLines("test");
  const test = build(testLines, 12);
  const chatDev = JSON.parse(readFileSync("data/chat/dev.json", "utf8")) as string[];
  const res = { params: best2, dev: devRep, testWiki: show(score(test, best2)),
    wNorm, detectionRecall: { chatDev: detection(chatDev, wNorm), testWiki: detection(testLines, wNorm) },
    naturalOrtho: { chatDev: naturalOrtho(chatDev, best2) }, latency: latency(testLines) };
  console.log(ser(res)); writeFileSync("typo-results.json", ser(res)); writeFileSync(tunedPath, ser({ ...best2, wNorm }));
} else {
  const OUT = "typo-fresh-results.json";
  if (existsSync(OUT) && !process.argv.includes("--force")) { console.error(`${OUT} exists: fresh set already evaluated once.`); process.exit(1); }
  const { wNorm, ...p } = JSON.parse(readFileSync(tunedPath, "utf8")) as CorrectorParams & { wNorm: number };
  const lines = JSON.parse(readFileSync("data/fresh/wongnai2_lines.json", "utf8")) as string[];
  const S = build(lines, 13);
  const sc = score(S, p);
  const fails = S.syn.filter((s) => decide(s.c, p, has).ranked[0] !== s.gold).slice(0, 25).map((s) => `${s.op}: ${s.typo} (gold ${s.gold}) -> ${decide(s.c, p, has).ranked[0] ?? "∅"}`);
  const fc = S.real.c.filter((x) => decide(x, p, has).apply !== x.normalized).slice(0, 25).map((x) => `${x.input} -> ${decide(x, p, has).apply}`);
  const res = { note: "Wongnai w_review_train.csv, every 20th review; one-shot", lines: lines.length, params: p, correction: show(sc),
    wNorm, detectionRecall: detection(lines, wNorm), naturalOrtho: naturalOrtho(lines, p), latency: latency(lines), failures: { synthetic: fails, falseCorrections: fc } };
  console.log(ser(res)); writeFileSync(OUT, ser(res));
}
