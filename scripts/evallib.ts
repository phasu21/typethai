// Shared evaluation helpers for bench.ts (dev/test tuning) and eval-fresh.ts (one-shot fresh held-out).
import { readFileSync } from "node:fs";
import { Detector, rawScore, contextFlips, ctxEligibility, type ModelBundle, type TokenDecision, type Eligibility } from "../src/detector.ts";
import { thToEn } from "../src/keymap.ts";
import { normalizeThai } from "../src/normalize.ts";

export type Kind = "pos" | "neg-mixed" | "neg-en" | "neg-code" | "skip";
export type Item = { d: TokenDecision; kind: Kind; gold: string; e2: Eligibility; e3: Eligibility; e4: Eligibility };
export type Line = Item[];
const THAI = /[\u0E00-\u0E7F]/;

export function makeTok(det: Detector) {
  return (typed: string, kind: Kind, gold: string): Item => {
    const d = det.scoreToken(typed);
    return { d, kind, gold, e2: ctxEligibility(d, 2), e3: ctxEligibility(d, 3), e4: ctxEligibility(d, 4, det.brands) };
  };
}
export function sample<T>(a: T[], n: number): T[] {
  if (a.length <= n) return a;
  const step = a.length / n; return Array.from({ length: n }, (_, i) => a[Math.floor(i * step)]);
}
export function rng(seed: number) { return () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 2 ** 32; }; }

/** Thai-script line -> labelled tokens: Thai-only typeable tokens become mistyped positives, ASCII tokens are embedded-English negatives. */
export function thaiLine(tok: ReturnType<typeof makeTok>, line: string): Line {
  return line.split(/\s+/).filter(Boolean).map((w) => {
    if (!THAI.test(w)) return /^[\x21-\x7E]+$/.test(w) ? tok(w, "neg-mixed", w) : tok(w, "skip", w);
    const e = thToEn(w);
    return e ? tok(e, "pos", w) : tok(w, "skip", w);
  });
}
export function latinLines(tok: ReturnType<typeof makeTok>, lines: string[], kind: Kind): Line[] {
  return lines.map((l) => l.split(/\s+/).filter(Boolean).map((w) => tok(w, kind, w)));
}
/** Synthetic short dictionary-word lines (3-6 words, ~1/3 with one English word inserted). */
export function shortSet(tok: ReturnType<typeof makeTok>, thaiWords: string[], thaiText: string, enLines: string[], seed: number, n = 800): Line[] {
  const words = thaiWords.filter((w) => { const k = [...w].length; return k >= 2 && k <= 4 && thToEn(w) && !w.includes(" "); }).filter((w) => thaiText.includes(w));
  const enVocab = [...new Set(enLines.flatMap((l) => l.split(/\s+/)).filter((w) => /^[A-Za-z]{1,8}$/.test(w)))];
  const r = rng(seed), out: Line[] = [];
  for (let i = 0; i < n; i++) {
    const k = 3 + Math.floor(r() * 4), line: Line = [];
    for (let j = 0; j < k; j++) { const w = words[Math.floor(r() * words.length)]; line.push(tok(thToEn(w)!, "pos", w)); }
    if (r() < 1 / 3) { const e = enVocab[Math.floor(r() * enVocab.length)]; line.splice(Math.floor(r() * (k + 1)), 0, tok(e, "neg-mixed", e)); }
    out.push(line);
  }
  return out;
}

export function evaluate(lines: Line[], p: any, fails?: any[]) {
  const c = { tp: 0, flips: 0, pos: 0, posShort: 0, tpShort: 0, posTypo: 0, tpTypo: 0, negMixed: 0, ffMixed: 0, negEn: 0, ffEn: 0, negCode: 0, ffCode: 0 };
  const g = p.guard ?? 3;
  for (const line of lines) {
    const scored = line.map((it) => ({ valid: it.kind !== "skip" && it.d.valid, score: it.kind === "skip" ? -Infinity : rawScore(it.d, p), eligible: g === 2 ? it.e2 : g === 4 ? it.e4 : it.e3, len: it.d.token.length }));
    const flips: boolean[] = p.flipFn ? p.flipFn(line) : contextFlips(scored, p);
    line.forEach((it, i) => {
      if (it.kind === "skip") return;
      const f = flips[i];
      const rec = () => fails?.push({ kind: it.kind, token: it.d.token, candidate: it.d.candidate, gold: it.gold, score: +scored[i].score.toFixed(2), why: it.d.reason,
        line: line.map((x, j) => (flips[j] ? x.d.candidate : x.d.token)).join(" ").slice(0, 120) });
      if (it.kind === "pos") {
        c.pos++; const short = [...it.gold].length <= 4; if (short) c.posShort++;
        const typo = normalizeThai(it.gold) !== it.gold; if (typo) c.posTypo++;
        if (f && typo && normalizeThai(it.d.candidate) === normalizeThai(it.gold)) c.tpTypo++;
        if (f) { c.flips++; if (normalizeThai(it.d.candidate) === normalizeThai(it.gold)) { c.tp++; if (short) c.tpShort++; } else rec(); } else rec();
      } else {
        if (f) { if (it.kind === "neg-mixed") c.flips++; rec(); }
        if (it.kind === "neg-mixed") { c.negMixed++; if (f) c.ffMixed++; }
        else if (it.kind === "neg-en") { c.negEn++; if (f) c.ffEn++; }
        else { c.negCode++; if (f) c.ffCode++; }
      }
    });
  }
  const precision = c.tp / Math.max(1, c.flips), recall = c.tp / Math.max(1, c.pos);
  return { ...c, precision, recall, f1: (2 * precision * recall) / Math.max(1e-9, precision + recall),
    ffPure: (c.ffEn + c.ffCode) / Math.max(1, c.negEn + c.negCode), ffMixedRate: c.ffMixed / Math.max(1, c.negMixed) };
}
export const pct = (a: number, b: number) => `${a}/${b} = ${((100 * a) / Math.max(1, b)).toFixed(3)}%`;
export function fmt(r: ReturnType<typeof evaluate>) {
  const o: Record<string, unknown> = {};
  if (r.pos) Object.assign(o, { precision: +r.precision.toFixed(4), recall: `${r.tp}/${r.pos} = ${r.recall.toFixed(4)}`, f1: +r.f1.toFixed(4),
    recallShortLe4: `${r.tpShort}/${r.posShort} = ${(r.tpShort / Math.max(1, r.posShort)).toFixed(4)}`,
    recallTypoTokens: `${r.tpTypo}/${r.posTypo} = ${(r.tpTypo / Math.max(1, r.posTypo)).toFixed(4)}` });
  if (r.negEn) o.falseFlipEnglish = pct(r.ffEn, r.negEn);
  if (r.negCode) o.falseFlipCode = pct(r.ffCode, r.negCode);
  if (r.negMixed) o.falseFlipEmbeddedEnglish = pct(r.ffMixed, r.negMixed);
  return o;
}
export function loadBundle(path = "data/model/model.json"): { bundle: ModelBundle; det: Detector } {
  const bundle = JSON.parse(readFileSync(path, "utf8")) as ModelBundle;
  return { bundle, det: new Detector(bundle) };
}
