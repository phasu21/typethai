// Learned per-token flip classifier: two-stage logistic regression (stage 2 sees stage-1 probabilities of neighbours
// and the line). Pure TS, weights are plain JSON. Replaces the hand brand list, guards and tuned thresholds.
import type { TokenDecision } from "./detector.ts";

const EMO = /^(?:[:;=8xX][-'^o]?[()\[\]DPpOo3|\\/*$@]+|T[_^.\-]?T|[xX]D+|\^[_.\-]?\^|<3+|5{3,}\+*)$/;
const LETTERKEYS = /[;',.\[\]\/`\\\-=]/g;

/** Token-level features (stage 1). Order is fixed; see FEATURE_NAMES. */
export function tokenFeatures(d: TokenDecision): number[] {
  const t = d.token, n = t.length || 1, conv = Number.isFinite(d.llTh) && d.reason !== "not convertible";
  const letters = (t.match(/[A-Za-z]/g) ?? []).length, upper = (t.match(/[A-Z]/g) ?? []).length, digits = (t.match(/\d/g) ?? []).length;
  const keyp = (t.match(LETTERKEYS) ?? []).length, other = n - letters - digits - keyp;
  const core = t.replace(/^[()\[\]{}"'`.,;:!?<>]+|[()\[\]{}"'`.,;:!?<>]+$/g, "") || t;
  const margin = conv ? d.llTh - d.llEn : 0;
  return [
    conv ? 1 : 0, d.valid ? 1 : 0, margin, conv ? d.llTh : 0, conv ? d.llEn : 0, d.covTh ?? 0, d.enHit ? 1 : 0, d.normEdits ?? 0,
    Math.min(n, 20) / 20, 1 / n, n <= 2 ? 1 : 0, n <= 4 ? 1 : 0,
    letters / n, upper / n, digits / n, keyp / n, other / n,
    /^[A-Z][a-z]+$/.test(core) ? 1 : 0, /^[A-Za-z]?[a-z]+([A-Z][a-z]+)+$/.test(core) ? 1 : 0, core.length >= 2 && /^[A-Z0-9]+$/.test(core) ? 1 : 0,
    /^[A-Za-z]+$/.test(t) ? 1 : 0, /^[\d.,%+\-]+$/.test(t) ? 1 : 0, /^[#@]/.test(t) ? 1 : 0,
    /^(https?:|www\.)|\.(com|net|org|co|th|io)(\/|$)/i.test(t) ? 1 : 0, EMO.test(core) ? 1 : 0,
    /^[A-Za-z]{1,6}-?\d+[A-Za-z+]*$/.test(core) ? 1 : 0, /(.)\1\1/.test(t) ? 1 : 0, core !== t ? 1 : 0,
    d.valid ? margin : 0,
  ];
}
export const FEATURE_NAMES = ["convertible", "valid", "margin", "llTh", "llEn", "dictCovTh", "enDictHit", "normEdits", "len/20", "1/len", "len<=2", "len<=4",
  "letterFrac", "upperFrac", "digitFrac", "thaiKeyPunctFrac", "otherFrac", "Capitalised", "CamelCase", "ALLCAPS", "lettersOnly", "numberish", "#@", "url", "emoticon",
  "modelCode", "triple-run", "edgePunct", "valid*margin"];

/** Stage-2 context features from stage-1 probabilities of the line. */
export function contextFeatures(p1: number[], i: number, win = 2): number[] {
  const nb: number[] = [];
  for (let j = Math.max(0, i - win); j <= Math.min(p1.length - 1, i + win); j++) if (j !== i) nb.push(p1[j]);
  const others = p1.filter((_, j) => j !== i);
  const mean = (a: number[]) => (a.length ? a.reduce((x, y) => x + y, 0) / a.length : 0);
  return [p1[i], p1[i - 1] ?? 0, p1[i + 1] ?? 0, nb.length ? Math.max(...nb) : 0, mean(nb), mean(others), others.filter((x) => x > 0.5).length / Math.max(1, others.length), Math.min(p1.length, 30) / 30, p1.length === 1 ? 1 : 0];
}
export const CONTEXT_NAMES = ["p1", "p1Left", "p1Right", "p1MaxWin2", "p1MeanWin2", "p1LineMean", "lineFracThai", "lineLen/30", "singleToken"];

export interface LR { w: number[]; b: number; mu: number[]; sd: number[] }
export interface FlipModel { stage1: LR; stage2: LR; threshold: number; names: string[] }
const sig = (z: number) => 1 / (1 + Math.exp(-z));
export function lrPredict(m: LR, x: number[]): number {
  let z = m.b; for (let k = 0; k < x.length; k++) z += m.w[k] * ((x[k] - m.mu[k]) / m.sd[k]); return sig(z);
}
/** Full-batch Adam on weighted log-loss with L2. X rows are raw features. */
export function trainLR(X: number[][], y: number[], opts: { l2?: number; epochs?: number; lr?: number; posWeight?: number } = {}): LR {
  const { l2 = 1e-4, epochs = 300, lr = 0.05 } = opts, n = X.length, d = X[0].length;
  const mu = new Array(d).fill(0), sd = new Array(d).fill(0);
  for (const x of X) x.forEach((v, k) => (mu[k] += v / n));
  for (const x of X) x.forEach((v, k) => (sd[k] += (v - mu[k]) ** 2 / n));
  for (let k = 0; k < d; k++) sd[k] = Math.sqrt(sd[k]) || 1;
  const Z = X.map((x) => x.map((v, k) => (v - mu[k]) / sd[k]));
  const npos = y.reduce((a, b) => a + b, 0), pw = opts.posWeight ?? (n - npos) / Math.max(1, npos);
  const w = new Array(d).fill(0); let b = 0;
  const m = new Array(d + 1).fill(0), v = new Array(d + 1).fill(0);
  for (let e = 1; e <= epochs; e++) {
    const g = new Array(d + 1).fill(0); let W = 0;
    for (let i = 0; i < n; i++) {
      let z = b; const zi = Z[i]; for (let k = 0; k < d; k++) z += w[k] * zi[k];
      const wt = y[i] ? pw : 1, r = (sig(z) - y[i]) * wt; W += wt;
      for (let k = 0; k < d; k++) g[k] += r * zi[k]; g[d] += r;
    }
    for (let k = 0; k <= d; k++) {
      const gk = g[k] / W + (k < d ? l2 * w[k] : 0);
      m[k] = 0.9 * m[k] + 0.1 * gk; v[k] = 0.999 * v[k] + 0.001 * gk * gk;
      const step = (lr * (m[k] / (1 - 0.9 ** e))) / (Math.sqrt(v[k] / (1 - 0.999 ** e)) + 1e-8);
      if (k < d) w[k] -= step; else b -= step;
    }
  }
  return { w, b, mu, sd };
}
/** Per-line flip probabilities. Non-convertible / invalid tokens get 0. */
export function lineProbs(m: FlipModel, ds: TokenDecision[]): number[] {
  const p1 = ds.map((d) => lrPredict(m.stage1, tokenFeatures(d)));
  return ds.map((d, i) => (d.valid && d.reason !== "not convertible" ? lrPredict(m.stage2, [...tokenFeatures(d), ...contextFeatures(p1, i)]) : 0));
}
export function lineFlips(m: FlipModel, ds: TokenDecision[], threshold = m.threshold): boolean[] {
  return lineProbs(m, ds).map((p) => p > threshold);
}
