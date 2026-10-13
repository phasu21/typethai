// Train the two-stage LR flip classifier on TRAIN splits only (wiki train, Wisesight chat train, EN train, code train,
// synthetic short-word lines from train text). Writes data/model/flip-classifier.json (threshold calibrated later on dev).
import { readFileSync, writeFileSync } from "node:fs";
import { Detector } from "../src/detector.ts";
import { tokenFeatures, contextFeatures, trainLR, lrPredict, FEATURE_NAMES, CONTEXT_NAMES, type FlipModel, type LR } from "../src/classifier.ts";
import { loadBundle, makeTok, sample, thaiLine, latinLines, shortSet, type Line } from "./evallib.ts";

const { bundle } = loadBundle();
const det = new Detector({ ...bundle, brands: [], params: { ...bundle.params, normalize: true } });
const tok = makeTok(det);
const th = (JSON.parse(readFileSync("data/splits/th.train.json", "utf8")) as { gold: string }[]).map((l) => l.gold);
const chat = JSON.parse(readFileSync("data/chat/train.json", "utf8")) as string[];
const en = JSON.parse(readFileSync("data/splits/en.train.json", "utf8")) as string[];
const code = JSON.parse(readFileSync("data/splits/code.train.json", "utf8")) as string[];
const lines: Line[] = [
  ...sample(th, 4000).map((l) => thaiLine(tok, l)), ...sample(chat, 10000).map((l) => thaiLine(tok, l)),
  ...latinLines(tok, sample(en, 3000), "neg-en"), ...latinLines(tok, sample(code, 6000), "neg-code"),
  ...shortSet(tok, bundle.thaiWords, th.join("\n"), en, 5, 1500),
];
const usable = (it: Line[number]) => it.kind !== "skip" && it.d.valid && it.d.reason !== "not convertible";
function rows1(L: Line[]) { const X: number[][] = [], y: number[] = []; for (const l of L) for (const it of l) if (usable(it)) { X.push(tokenFeatures(it.d)); y.push(it.kind === "pos" ? 1 : 0); } return { X, y }; }
function rows2(L: Line[], s1: LR) {
  const X: number[][] = [], y: number[] = [];
  for (const l of L) { const p1 = l.map((it) => lrPredict(s1, tokenFeatures(it.d)));
    l.forEach((it, i) => { if (usable(it)) { X.push([...tokenFeatures(it.d), ...contextFeatures(p1, i)]); y.push(it.kind === "pos" ? 1 : 0); } }); }
  return { X, y };
}
console.time("train");
// 2-fold cross-fitting so stage 2 sees out-of-fold stage-1 probabilities.
const A = lines.filter((_, i) => i % 2 === 0), B = lines.filter((_, i) => i % 2 === 1);
const rA = rows1(A), rB = rows1(B);
const s1A = trainLR(rA.X, rA.y), s1B = trainLR(rB.X, rB.y);
const r2a = rows2(A, s1B), r2b = rows2(B, s1A);
const stage2 = trainLR([...r2a.X, ...r2b.X], [...r2a.y, ...r2b.y]);
const all1 = rows1(lines);
const stage1 = trainLR(all1.X, all1.y);
console.timeEnd("train");
const model: FlipModel = { stage1, stage2, threshold: 0.5, names: [...FEATURE_NAMES, ...CONTEXT_NAMES.map((n) => "ctx:" + n)] };
writeFileSync("data/model/flip-classifier.json", JSON.stringify(model));
const pos = all1.y.reduce((a, b) => a + b, 0);
console.log(`lines=${lines.length} rows=${all1.y.length} pos=${pos} neg=${all1.y.length - pos} params=${stage1.w.length + 1 + stage2.w.length + 1}`);
const top = stage2.w.map((w, k) => [model.names[k], +w.toFixed(3)] as const).sort((a, b) => Math.abs(b[1]) - Math.abs(a[1])).slice(0, 12);
console.log("top stage-2 weights:", JSON.stringify(top));
