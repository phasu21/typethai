// Word-level Thai spelling suggestions: SymSpell-style delete index over a dictionary, candidates re-scored with a
// Kedmanee-aware weighted Damerau-Levenshtein distance, word frequency and the character n-gram model.
import { EN_UNSHIFTED, EN_SHIFTED, TH_UNSHIFTED, TH_SHIFTED } from "./keymap.ts";
import { meanLogProb, type NgramModel } from "./ngram.ts";
import { normalizeThai } from "./normalize.ts";

const ROWS = ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"];
type Pos = { r: number; c: number; shift: boolean };
const POS = new Map<string, Pos>();
{
  const en = [...EN_UNSHIFTED], ens = [...EN_SHIFTED], th = [...TH_UNSHIFTED], ths = [...TH_SHIFTED];
  en.forEach((k, i) => {
    const r = ROWS.findIndex((row) => row.includes(k)), c = ROWS[r].indexOf(k);
    if (!POS.has(th[i])) POS.set(th[i], { r, c, shift: false });
    if (!POS.has(ths[i])) POS.set(ths[i], { r, c, shift: true });
  });
}
// Plain QWERTY layout positions for Latin text (English typo suggestions).
const POS_EN = new Map<string, Pos>();
{
  const en = [...EN_UNSHIFTED], ens = [...EN_SHIFTED];
  en.forEach((k, i) => {
    const r = ROWS.findIndex((row) => row.includes(k)), c = ROWS[r].indexOf(k);
    if (!POS_EN.has(k)) POS_EN.set(k, { r, c, shift: false });
    const sk = ens[i];
    if (sk !== undefined && !POS_EN.has(sk)) POS_EN.set(sk, { r, c, shift: true });
  });
}

export function keyPos(ch: string): Pos | undefined { return POS.get(ch); }
/** Physical neighbours on a staggered QWERTY grid (same row +-1; row above c, c+1; row below c-1, c). */
export function adjacent(a: Pos, b: Pos): boolean {
  if (a.r === b.r) return Math.abs(a.c - b.c) === 1;
  if (b.r === a.r - 1) return b.c === a.c || b.c === a.c + 1;
  if (b.r === a.r + 1) return b.c === a.c || b.c === a.c - 1;
  return false;
}
/** All Thai chars on keys adjacent to ch's key, on the same shift layer. */
export function neighbourChars(ch: string): string[] {
  const p = POS.get(ch); if (!p) return [];
  const out: string[] = [];
  for (const [t, q] of POS) if (q.shift === p.shift && adjacent(p, q) && /[\u0E00-\u0E7F]/.test(t)) out.push(t);
  return out;
}
/** The char on the same key with the other shift state (shift slip). */
export function shiftPartner(ch: string): string | undefined {
  const p = POS.get(ch); if (!p) return undefined;
  for (const [t, q] of POS) if (q.r === p.r && q.c === p.c && q.shift !== p.shift && /[\u0E00-\u0E7F]/.test(t)) return t;
  return undefined;
}

export interface Costs { shift: number; adj: number; adjShift: number; sub: number; dup: number; insAdj: number; ins: number; del: number; trans: number }
export const DEFAULT_COSTS: Costs = { shift: 0.3, adj: 0.5, adjShift: 0.8, sub: 1, dup: 0.4, insAdj: 0.7, ins: 1, del: 0.8, trans: 0.5 };

function subCost(a: string, b: string, k: Costs): number {
  if (a === b) return 0;
  const maps = [POS, POS_EN];
  for (const m of maps) {
    const p = m.get(a), q = m.get(b);
    if (p && q) {
      if (p.r === q.r && p.c === q.c) return k.shift;
      if (adjacent(p, q)) return p.shift === q.shift ? k.adj : k.adjShift;
      return k.sub;
    }
  }
  return k.sub;
}
/** Weighted Damerau-Levenshtein: cost of turning intended word `w` into observed `t`. */
export function keyDistance(t: string, w: string, k: Costs = DEFAULT_COSTS): number {
  const a = [...t], b = [...w], n = a.length, m = b.length;
  const d: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  const insCost = (i: number, j: number) => { // extra char a[i-1] in t; context b[j-1] (prev intended) / a[i-2]
    const x = a[i - 1], prev = a[i - 2] ?? b[j - 1];
    if (prev === x) return k.dup;
    const m = POS.has(x) ? POS : (POS_EN.has(x) ? POS_EN : POS);
    const px = m.get(x);
    const nb = [b[j - 1], b[j]].filter(Boolean).map((c) => m.get(c!));
    if (px && nb.some((q) => q && adjacent(px, q))) return k.insAdj;
    return k.ins;
  };
  for (let i = 1; i <= n; i++) d[i][0] = d[i - 1][0] + insCost(i, 0);
  for (let j = 1; j <= m; j++) d[0][j] = d[0][j - 1] + k.del;
  for (let i = 1; i <= n; i++) for (let j = 1; j <= m; j++) {
    let v = Math.min(d[i - 1][j] + insCost(i, j), d[i][j - 1] + k.del, d[i - 1][j - 1] + subCost(b[j - 1], a[i - 1], k));
    if (i > 1 && j > 1 && a[i - 1] === b[j - 2] && a[i - 2] === b[j - 1]) v = Math.min(v, d[i - 2][j - 2] + k.trans);
    d[i][j] = v;
  }
  return d[n][m];
}

function deletes(w: string[], maxD: number): Set<string> {
  const out = new Set<string>([w.join("")]);
  let frontier = [w];
  for (let d = 0; d < maxD; d++) {
    const next: string[][] = [];
    for (const x of frontier) for (let i = 0; i < x.length; i++) {
      const y = [...x.slice(0, i), ...x.slice(i + 1)], s = y.join("");
      if (!out.has(s)) { out.add(s); next.push(y); }
    }
    frontier = next;
  }
  return out;
}

export interface CorrectorParams { alpha: number; beta: number; gamma: number; keepBias: number; tau: number; maxDist: number }
export const DEFAULT_CORRECTOR: CorrectorParams = { alpha: 3, beta: 1, gamma: 0.5, keepBias: 0, tau: 0.9, maxDist: 2 };
export interface Candidate { word: string; dist: number; logFreq: number; ll: number }
export interface Suggestion { word: string; score: number; confidence: number; dist: number }
export interface Correction { input: string; normalized: string; inDict: boolean; suggestions: Suggestion[]; keepConfidence: number; apply: string | null }

export class Corrector {
  private index = new Map<string, number[]>();
  private words: string[];
  private wordSet: Set<string>;
  private freq: Map<string, number>;
  params: CorrectorParams;
  private thai: NgramModel; private maxDel: number;
  constructor(words: string[], freq: Record<string, number>, thai: NgramModel, params: Partial<CorrectorParams> = {}, maxDel = 2) {
    this.thai = thai; this.maxDel = maxDel;
    this.words = [...new Set(words.filter((w) => !/\s/.test(w) && [...w].length >= 2 && [...w].length <= 16))];
    this.wordSet = new Set(this.words);
    this.freq = new Map(Object.entries(freq));
    this.params = { ...DEFAULT_CORRECTOR, ...params };
    this.words.forEach((w, id) => {
      for (const d of deletes([...w], [...w].length <= 3 ? 1 : this.maxDel)) {
        const l = this.index.get(d); if (l) l.push(id); else this.index.set(d, [id]);
      }
    });
  }
  has(w: string): boolean { return this.wordSet.has(w); }
  /** Raw candidates (param-independent features), for caching in tuning. */
  candidates(t: string): Candidate[] {
    const ids = new Set<number>();
    for (const d of deletes([...t], this.maxDel)) for (const id of this.index.get(d) ?? []) ids.add(id);
    const out: Candidate[] = [];
    for (const id of ids) {
      const w = this.words[id];
      if (w === t) continue;
      const dist = keyDistance(t, w);
      if (dist <= this.params.maxDist) out.push({ word: w, dist, logFreq: Math.log1p(this.freq.get(w) ?? 0), ll: meanLogProb(this.thai, w) });
    }
    return out;
  }
  static rank(t: string, cands: Candidate[], keepLL: number, p: CorrectorParams): { suggestions: Suggestion[]; keepConfidence: number } {
    const sc = cands.map((c) => ({ c, s: -p.alpha * c.dist + p.gamma * c.logFreq + p.beta * c.ll }));
    const keep = p.keepBias + p.beta * keepLL;
    const mx = Math.max(keep, ...sc.map((x) => x.s));
    const z = Math.exp(keep - mx) + sc.reduce((a, x) => a + Math.exp(x.s - mx), 0);
    sc.sort((a, b) => b.s - a.s);
    return { keepConfidence: Math.exp(keep - mx) / z,
      suggestions: sc.map((x) => ({ word: x.c.word, score: x.s, dist: x.c.dist, confidence: Math.exp(x.s - mx) / z })) };
  }
  /** Ranked suggestions for one Thai word/token. `apply` is set only when confidence >= tau. */
  suggest(token: string, k = 5): Correction {
    const normalized = normalizeThai(token);
    if (this.wordSet.has(normalized)) return { input: token, normalized, inDict: true, suggestions: [], keepConfidence: 1, apply: normalized !== token ? normalized : null };
    const cands = this.candidates(normalized);
    const { suggestions, keepConfidence } = Corrector.rank(normalized, cands, meanLogProb(this.thai, normalized), this.params);
    const top = suggestions[0];
    const apply = top && top.confidence >= this.params.tau ? top.word : normalized !== token ? normalized : null;
    return { input: token, normalized, inDict: false, suggestions: suggestions.slice(0, k), keepConfidence, apply };
  }
}

/** Dictionary max-coverage segmentation (DP): returns pieces; unknown chars are grouped into runs. */
export function segment(s: string, has: (w: string) => boolean, maxLen = 16): string[] {
  const ch = [...s], n = ch.length;
  const best = new Array<number>(n + 1).fill(-Infinity), back = new Array<number>(n + 1).fill(-1);
  best[0] = 0;
  for (let i = 1; i <= n; i++) {
    best[i] = best[i - 1] - 1; back[i] = i - 1; // unknown char
    for (let L = 1; L <= Math.min(maxLen, i); L++) {
      const w = ch.slice(i - L, i).join("");
      if (has(w)) { const v = best[i - L] + L * L; if (v > best[i]) { best[i] = v; back[i] = i - L; } }
    }
  }
  const out: string[] = []; let i = n, unk = "";
  while (i > 0) {
    const j = back[i], w = ch.slice(j, i).join("");
    if (i - j === 1 && !has(w)) unk = w + unk;
    else { if (unk) { out.push(unk); unk = ""; } out.push(w); }
    i = j;
  }
  if (unk) out.push(unk);
  return out.reverse();
}
