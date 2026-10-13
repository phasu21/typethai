// Synthetic Kedmanee typo generator + typo-eval helpers.
import { neighbourChars, shiftPartner, segment, Corrector, type Candidate, type CorrectorParams } from "../src/corrector.ts";
import { normalizeThai } from "../src/normalize.ts";
import { meanLogProb, type NgramModel } from "../src/ngram.ts";
import { rng } from "./evallib.ts";

export const OPS = ["adjacent", "shift", "transpose", "omit", "duplicate"] as const;
export type Op = (typeof OPS)[number];
export function makeTypo(w: string, op: Op, r: () => number): string | null {
  const c = [...w], n = c.length, pick = <T>(a: T[]) => a[Math.floor(r() * a.length)];
  if (op === "adjacent") { const ps = c.map((x, i) => i).filter((i) => neighbourChars(c[i]).length); if (!ps.length) return null; const i = pick(ps); c[i] = pick(neighbourChars(c[i])); }
  else if (op === "shift") { const ps = c.map((x, i) => i).filter((i) => shiftPartner(c[i])); if (!ps.length) return null; const i = pick(ps); c[i] = shiftPartner(c[i])!; }
  else if (op === "transpose") { const ps = c.slice(0, -1).map((x, i) => i).filter((i) => c[i] !== c[i + 1]); if (!ps.length) return null; const i = pick(ps); [c[i], c[i + 1]] = [c[i + 1], c[i]]; }
  else if (op === "omit") { if (n < 3) return null; c.splice(Math.floor(r() * n), 1); }
  else { const i = Math.floor(r() * n); c.splice(i, 0, c[i]); }
  return c.join("");
}
/** Dict words (2-10 chars) by occurrence from real text, each turned into one synthetic typo (non-word errors only). */
export function syntheticSet(lines: string[], has: (w: string) => boolean, n: number, seed: number) {
  const occ: string[] = [];
  for (const l of lines) for (const run of l.match(/[\u0E00-\u0E7F]+/g) ?? []) for (const w of segment(normalizeThai(run), has)) { const k = [...w].length; if (has(w) && k >= 2 && k <= 10) occ.push(w); }
  const r = rng(seed), out: { gold: string; typo: string; op: Op }[] = [];
  const step = Math.max(1, Math.floor(occ.length / (n * 1.5)));
  for (let i = 0; i < occ.length && out.length < n; i += step) {
    const op = OPS[out.length % OPS.length];
    const t = makeTypo(occ[i], op, r);
    if (t && t !== occ[i] && !has(t) && !has(normalizeThai(t))) out.push({ gold: occ[i], typo: t, op });
  }
  return out;
}
/** Real (assumed correct) space-delimited Thai-only tokens 2-8 chars that are NOT fully dictionary-segmentable. */
export function realTokens(lines: string[], has: (w: string) => boolean, n: number) {
  const all: string[] = [];
  for (const l of lines) for (const t of l.split(/\s+/)) if (/^[\u0E00-\u0E7F]+$/.test(t) && [...t].length >= 2 && [...t].length <= 8) all.push(t);
  const covered = (t: string) => segment(normalizeThai(t), has).every((w) => has(w));
  const ok = all.filter((t) => !covered(t));
  const step = Math.max(1, Math.floor(ok.length / n));
  return { totalTokens: all.length, oovTokens: ok.length, sample: ok.filter((_, i) => i % step === 0).slice(0, n) };
}
export type Cached = { input: string; normalized: string; cands: Candidate[]; keepLL: number };
export function cache(c: Corrector, thai: NgramModel, t: string): Cached {
  const normalized = normalizeThai(t);
  return { input: t, normalized, cands: c.has(normalized) ? [] : c.candidates(normalized), keepLL: meanLogProb(thai, normalized) };
}
export function decide(x: Cached, p: CorrectorParams, has: (w: string) => boolean) {
  if (has(x.normalized)) return { ranked: [] as string[], apply: x.normalized };
  const { suggestions } = Corrector.rank(x.normalized, x.cands, x.keepLL, p);
  const top = suggestions[0];
  return { ranked: suggestions.map((s) => s.word), apply: top && top.confidence >= p.tau ? top.word : x.normalized };
}
