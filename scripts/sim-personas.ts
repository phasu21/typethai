// Simulated users replaying typing streams with implicit feedback -> learning curves for the personal model.
import { readFileSync, writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { performance } from "node:perf_hooks";
import { Detector } from "../src/detector.ts";
import { lineFlips, type FlipModel } from "../src/classifier.ts";
import { PersonalModel, DEFAULT_PERSONAL, type PersonalConfig } from "../src/personal.ts";
import { savePersonal, loadPersonal, resetPersonal } from "../src/personal-store.ts";
import { loadBundle, makeTok, sample, thaiLine, latinLines, shortSet, evaluate, fmt, rng, type Line } from "./evallib.ts";

const { bundle } = loadBundle();
const det = new Detector({ ...bundle, brands: [], params: { ...bundle.params, normalize: true } });
const tok = makeTok(det);
const base = JSON.parse(readFileSync("data/model/flip-classifier.json", "utf8")) as FlipModel;
const ser = (o: any) => JSON.stringify(o, null, 2);

// ---- personas (held-out sources only; injected vocab makes each user distinct) ----
function inject(lines: string[], seed: number, adds: { p: number; words: string[] }[], maxLines: number): string[] {
  const r = rng(seed), out: string[] = [];
  for (const l of lines.slice(0, maxLines)) {
    const t = l.split(/\s+/).filter(Boolean);
    for (const a of adds) if (r() < a.p) t.splice(Math.floor(r() * (t.length + 1)), 0, a.words[Math.floor(r() * a.words.length)]);
    out.push(t.join(" "));
  }
  return out;
}
const J = (p: string) => JSON.parse(readFileSync(p, "utf8")) as string[];
const PERSONAS: Record<string, { desc: string; lines: string[] }> = {
  foodie: { desc: "Wongnai reviews (v4 held-out) + English food/brand words (30% of lines) + emoticons (25%)",
    lines: inject(J("data/fresh/wongnai_lines.json"), 1, [{ p: 0.3, words: ["mk", "latte", "matcha", "cafe", "wifi", "omakase", "brunch", "starbucks", "sushi", "ok", "set", "menu"] },
      { p: 0.25, words: [";-;", "T^T", "xD", ":)", "-_-", ";w;", "^^"] }], 1400) },
  slang: { desc: "Thai Toxicity tweets (v6 held-out) + Thai slang/abbreviations typed in Thai (40% of lines)",
    lines: inject(J("data/fresh/tweets_lines.json"), 2, [{ p: 0.4, words: ["สัส", "คสช", "กทม", "ปชช", "แรด", "ทำ", "นะ", "จ้า", "ปะ", "ดิ"] }], 2160) },
  names: { desc: "Wisesight chat dev + English-letter nicknames (40%) + Thai names typed in Thai (20%)",
    lines: inject(J("data/chat/dev.json"), 3, [{ p: 0.4, words: ["bank", "mint", "ploy", "fern", "beam", "june", "nut", "pond", "Gun", "Toey"] },
      { p: 0.2, words: ["พลอย", "มิ้นท์", "แบงค์", "เฟิร์น", "ต้อย", "ปอนด์"] }], 1500) },
};

type Scenario = { name: string; miss: number; wrong: number };
const SCEN: Scenario[] = [
  { name: "clean", miss: 0, wrong: 0 }, { name: "default (10% missing, 1% wrong)", miss: 0.1, wrong: 0.01 },
  { name: "noisy (30% missing, 5% wrong)", miss: 0.3, wrong: 0.05 }, { name: "adversarial (10% missing, 20% wrong)", miss: 0.1, wrong: 0.2 },
];

function run(L: Line[], sc: Scenario, cfg: PersonalConfig, seed: number, pm = new PersonalModel(base, undefined, cfg)) {
  const r = rng(seed), W = 500;
  const winA: number[] = [], winS: number[] = []; let n = 0, eA = 0, eS = 0, totA = 0, totS = 0, upd = 0, updMs = 0, predMs = 0, predTok = 0;
  for (const line of L) {
    const ds = line.map((it) => it.d);
    let t0 = performance.now(); const fa = pm.lineFlips(ds); predMs += performance.now() - t0; predTok += ds.length;
    const fs = lineFlips(base, ds);
    const events: [number, "undo" | "retype"][] = [];
    line.forEach((it, i) => {
      if (it.kind === "skip") return;
      const y = it.kind === "pos";
      const errA = fa[i] !== y, errS = fs[i] !== y;
      n++; eA += +errA; eS += +errS; totA += +errA; totS += +errS;
      if (errA) { if (r() >= sc.miss) events.push([i, y ? "retype" : "undo"]); }
      else if (r() < sc.wrong) events.push([i, y ? "undo" : "retype"]); // wrong signal on a correct decision
      if (n % W === 0) { winA.push(eA / W); winS.push(eS / W); eA = 0; eS = 0; }
    });
    for (const [i, k] of events) { const t1 = performance.now(); pm.feedback(ds, i, k); updMs += performance.now() - t1; upd++; }
  }
  const staticRate = totS / n;
  // sustained: first window from which the mean of all remaining adaptive windows is <= half the static error rate
  const halveIdx = winA.findIndex((_, i) => { const rest = winA.slice(i); return rest.reduce((a, b) => a + b, 0) / rest.length <= staticRate / 2; });
  return { pm, tokens: n, staticErrRate: +staticRate.toFixed(4), adaptiveErrRate: +(totA / n).toFixed(4), staticErrors: totS, adaptiveErrors: totA,
    windowsStatic: winS.map((x) => +x.toFixed(4)), windowsAdaptive: winA.map((x) => +x.toFixed(4)),
    lastWindowStatic: winS.at(-1), lastWindowAdaptive: winA.at(-1),
    tokensToHalve: halveIdx >= 0 ? (halveIdx + 1) * W : "never", feedbackEvents: upd,
    updateMsMean: +(updMs / Math.max(1, upd)).toFixed(4), predictMsPerToken: +(predMs / predTok).toFixed(4) };
}

// Standard dev sets for the degradation check.
const wiki = (JSON.parse(readFileSync("data/splits/th.dev.json", "utf8")) as { gold: string }[]).map((l) => l.gold);
const enDev = J("data/splits/en.dev.json");
const devSets: Record<string, Line[]> = {
  wikiThai: wiki.map((l) => thaiLine(tok, l)), chatDev: J("data/chat/dev.json").map((l) => thaiLine(tok, l)),
  english: latinLines(tok, enDev, "neg-en"), code: latinLines(tok, sample(J("data/splits/code.dev.json"), 6000), "neg-code"),
  shortDictSet: shortSet(tok, bundle.thaiWords, wiki.join("\n"), enDev, 1),
};
const devReport = (flip: (ds: any[]) => boolean[]) => Object.fromEntries(Object.entries(devSets).map(([k, v]) => [k, fmt(evaluate(v, { flipFn: (line: Line) => flip(line.map((it) => it.d)) }))]));

// ---- hyper-parameter tuning on a DEV persona (Wisesight chat-TRAIN lines, different injected vocab) ----
const devPersona = inject(J("data/chat/train.json"), 9, [{ p: 0.3, words: ["pizza", "boba", "grab", "line", "cake", "kfc", "promo", "dm"] }, { p: 0.2, words: [":D", "TT", "^_^", ":("] },
  { p: 0.3, words: ["555", "จ้ะ", "เด้อ", "ป่าว", "มั้ย", "อ่ะ"] }, { p: 0.2, words: ["Max", "Ice", "fah", "nam"] }], 1800).map((l) => thaiLine(tok, l));
let CFG: PersonalConfig = DEFAULT_PERSONAL, bestObj = Infinity; const grid: any[] = [];
for (const lr of [0.01, 0.03, 0.1]) for (const l2 of [0.01, 0.1]) for (const maxDrift of [0.3, 1]) for (const memWeight of [0.5, 2]) for (const memOverride of [3, 6, Infinity])
  for (const trust of [0, 0.9]) for (const memPurity of [0, 0.85]) {
    const c = { ...DEFAULT_PERSONAL, lr, l2, maxDrift, memWeight, memOverride, trust, memPurity };
    const rs = [SCEN[1], SCEN[2], SCEN[3]].map((sc, j) => run(devPersona, sc, c, 50 + j));
    const ratios = rs.map((r) => r.adaptiveErrRate / Math.max(1e-9, r.staticErrRate));
    // objective: mean error ratio over default/noisy/adversarial, with a hard penalty if default is worse than static
    const obj = (ratios[0] + ratios[1] + ratios[2]) / 3 + (ratios[0] > 1 ? 10 : 0);
    grid.push({ ...c, ratios: ratios.map((x) => +x.toFixed(3)) });
    if (obj < bestObj) { bestObj = obj; CFG = c; }
  }
console.error("tuned config on dev persona:", JSON.stringify(CFG), "obj", bestObj.toFixed(3));

const out: any = { config: CFG, tuning: { devPersona: "Wisesight chat train + injected vocab", gridSize: grid.length, best: grid.find((g) => JSON.stringify({ ...g, ratios: undefined }) === JSON.stringify({ ...CFG, ratios: undefined })) }, personas: {}, devBase: devReport((ds) => lineFlips(base, ds)) };
const dir = mkdtempSync(join(tmpdir(), "typethai-"));
for (const [name, P] of Object.entries(PERSONAS)) {
  const L = P.lines.map((l) => thaiLine(tok, l));
  const res: any = { desc: P.desc, lines: L.length };
  for (const sc of SCEN) {
    const r = run(L, sc, CFG, 100 + name.length);
    const { pm, ...rest } = r;
    const path = join(dir, `${name}.json`), bytes = savePersonal(pm, path);
    const re = loadPersonal(base, path); // persistence round trip
    res[sc.name] = { ...rest, stateBytes: bytes, memEntries: Object.keys(pm.state.mem).length, maxAbsDelta: +Math.max(...pm.state.dw.map(Math.abs), Math.abs(pm.state.db)).toFixed(3),
      roundTripSame: JSON.stringify(re.state) === JSON.stringify(pm.state) && JSON.stringify(re.lineProbs(L[5].map((it) => it.d))) === JSON.stringify(pm.lineProbs(L[5].map((it) => it.d))) };
    if (sc.name.startsWith("default") || sc.name.startsWith("adversarial")) res[sc.name].devAfterAdaptation = devReport((ds) => pm.lineFlips(ds));
    resetPersonal(pm, path);
  }
  out.personas[name] = res;
}
// Learning-rate sensitivity on one persona (default scenario).
const Lf = PERSONAS.foodie.lines.map((l) => thaiLine(tok, l));
out.lrSensitivityFoodie = Object.fromEntries([0.01, 0.05, 0.2].map((lr) => { const r = run(Lf, SCEN[1], { ...CFG, lr }, 7); return [lr, { adaptiveErrRate: r.adaptiveErrRate, staticErrRate: r.staticErrRate, tokensToHalve: r.tokensToHalve }]; }));
out.memoryAblationFoodie = (() => { const r = run(Lf, SCEN[1], { ...CFG, memWeight: 0, memOverride: Infinity }, 7); return { note: "delta-only, no token memory", adaptiveErrRate: r.adaptiveErrRate, staticErrRate: r.staticErrRate, tokensToHalve: r.tokensToHalve }; })();
writeFileSync("personal-sim-results.json", ser(out));
const brief = (x: any) => Object.fromEntries(Object.entries(x).map(([k, v]: any) => [k, typeof v === "object" && v && "tokens" in v ? { tokens: v.tokens, static: v.staticErrRate, adaptive: v.adaptiveErrRate, lastWinS: v.lastWindowStatic, lastWinA: v.lastWindowAdaptive, halve: v.tokensToHalve, events: v.feedbackEvents, updMs: v.updateMsMean, predMs: v.predictMsPerToken, bytes: v.stateBytes, mem: v.memEntries, maxDelta: v.maxAbsDelta, rt: v.roundTripSame } : v]));
console.log(ser({ config: CFG, personas: Object.fromEntries(Object.entries(out.personas).map(([k, v]) => [k, brief(v)])), lr: out.lrSensitivityFoodie, memAblation: out.memoryAblationFoodie }));
