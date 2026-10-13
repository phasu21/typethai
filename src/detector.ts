import { enToTh } from "./keymap.ts";
import { normalizeThai } from "./normalize.ts";
import { checkThai } from "./orthography.ts";
import { meanLogProb, type NgramModel } from "./ngram.ts";
import { Dict } from "./dict.ts";

export interface ModelBundle {
  thai: NgramModel;
  english: NgramModel;
  thaiWords: string[];
  englishWords: string[];
  params: Params;
  /** Lowercased brand/abbreviation list (chat-mined + hand list); such tokens are never boosted. */
  brands?: string[];
  /** Collapse runs of >=3 identical chars before scoring (Thai side -> 1, English side -> 2). */
  collapse?: boolean;
}
export interface Params {
  wDict: number; wEnDict: number; wShort: number; threshold: number;
  /** Context boost per flipped neighbour (0 disables). */ wCtx?: number;
  /** Neighbour window (tokens on each side). */ ctxWindow?: number;
  /** A token's own score must be above this to receive a context boost. */ ctxFloor?: number;
  /** Whole-line prior boost, and when it activates. */ wLine?: number; lineMinFrac?: number; lineMinFlips?: number;
  /** Let the line prior boost letters-only known-English tokens (e.g. "or", "st"). */ lineAllowEn?: boolean;
  /** Guard rule version (2 = v2 rules, 3 = edge-punct stripping + CamelCase, 4 = v3 + chat shapes + brand list). Default 3. */ guard?: 2 | 3 | 4;
  /** Normalise common Thai orthographic typos in the converted candidate before validity/scoring (v5). */ normalize?: boolean;
  /** Score penalty per normalisation edit (v5). */ wNorm?: number;
}

export interface TokenDecision {
  token: string; candidate: string; flip: boolean; score: number;
  valid: boolean; reason?: string; llTh: number; llEn: number; covTh: number; enHit: boolean;
  /** 0 if normalisation changed nothing, else 1 + |length change| (v5). */ normEdits?: number;
}

const HAS_LATIN_KEY = /[A-Za-z;'\[\],.\/`\\\-=0-9~!@#$%^&*()_+{}|:"<>?]/;
const ALL_KEYS = /^[\x21-\x7E]+$/;
const URLISH = /^(https?:|www\.)|@[a-z0-9-]+\.[a-z]/i;

export class Detector {
  readonly thDict: Dict; readonly enWords: Set<string>; readonly brands: Set<string>; params: Params; private m: ModelBundle;
  constructor(m: ModelBundle) {
    this.m = m;
    this.brands = new Set(m.brands ?? []);
    this.thDict = new Dict(m.thaiWords);
    this.enWords = new Set(m.englishWords);
    this.params = { ...m.params };
  }

  scoreToken(token: string): TokenDecision {
    const raw = enToTh(token);
    const candidate = this.params.normalize && raw !== token ? normalizeThai(raw) : raw;
    const base = { token, candidate: raw, llTh: 0, llEn: 0, covTh: 0, enHit: false };
    if (!ALL_KEYS.test(token) || !HAS_LATIN_KEY.test(token) || raw === token || URLISH.test(token)) return { ...base, flip: false, score: -Infinity, valid: false, reason: "not convertible" };
    const v = checkThai(candidate);
    const thS = this.m.collapse ? collapseRuns(candidate, 1) : candidate;
    const enS = this.m.collapse ? collapseRuns(token, 2) : token;
    const llTh = meanLogProb(this.m.thai, thS);
    const llEn = meanLogProb(this.m.english, enS);
    const covTh = this.thDict.coverage(thS);
    const stripped = enS.toLowerCase().replace(/^[^a-z]+|[^a-z]+$/g, "");
    const enHit = stripped.length > 0 && this.enWords.has(stripped);
    const p = this.params;
    const normEdits = candidate === raw ? 0 : 1 + Math.abs([...raw].length - [...candidate].length);
    const score = rawScore({ llTh, llEn, covTh, enHit, token, normEdits }, p);
    return { token, candidate, llTh, llEn, covTh, enHit, normEdits, valid: v.valid, reason: v.reason, score, flip: v.valid && score > p.threshold };
  }

  /** Convert a line: whitespace-separated tokens are flipped independently. */
  convert(line: string): { output: string; decisions: TokenDecision[] } {
    const parts = line.split(/(\s+)/);
    const decisions: TokenDecision[] = [];
    for (const part of parts) if (part && !/^\s+$/.test(part)) decisions.push(this.scoreToken(part));
    const flips = contextFlips(decisions.map((d) => ({ valid: d.valid, score: d.score, eligible: ctxEligibility(d, this.params.guard ?? 3, this.brands), len: d.token.length })), this.params);
    decisions.forEach((d, i) => { d.flip = flips[i]; });
    let k = 0;
    const output = parts.map((part) => (!part || /^\s+$/.test(part)) ? part : (decisions[k++].flip ? decisions[k - 1].candidate : part)).join("");
    return { output, decisions };
  }
}

/** Combined margin: positive favours Thai. Exported so benchmarks can re-tune from cached features. */
export function rawScore(f: { llTh: number; llEn: number; covTh: number; enHit: boolean; token: string; normEdits?: number }, p: Params): number {
  return (f.llTh - f.llEn) + p.wDict * f.covTh - p.wEnDict * (f.enHit ? 1 : 0) - p.wShort / f.token.length - (p.wNorm ?? 0) * (f.normEdits ?? 0);
}

/**
 * Context-aware flip decisions for one line. Pass 1 flips tokens whose own score beats the threshold.
 * Then, iteratively (until stable), an unflipped token that passes the orthography gate gets
 * +wCtx per flipped neighbour within ctxWindow tokens. Flips are only added, never removed, and the
 * boost only comes from neighbours, so a line with no confident Thai is never changed.
 */
export type Eligibility = "ok" | "enword" | "never";

/** Strip ASCII punctuation from the token edges (used only for shape checks, never for conversion). */
export function coreOf(t: string): string {
  return t.replace(/^[()\[\]{}"'`.,;:!?<>]+|[()\[\]{}"'`.,;:!?<>]+$/g, "");
}

/**
 * Guard for context/line boosting. Shapes that look like English get "never" (they can still flip on their own score):
 * numbers ("8", "4,696"), Capitalised words ("Luciano", "(Silvio", "Adhikari)"), CamelCase ("ChoiKoon", "iPhone"),
 * ALL-CAPS and hyphenated acronyms ("PGM-FI", "TN"). Letters-only known English words ("so", "or") get "enword":
 * no neighbour boost, but the whole-line prior may boost them if `lineAllowEn` is set.
 * guard=2 reproduces the v2 rules (no edge stripping, no CamelCase) for comparison.
 */
export function ctxEligibility(d: { token: string; enHit: boolean }, guard: 2 | 3 | 4 = 3, brands?: Set<string>): Eligibility {
  const t = guard >= 3 ? coreOf(d.token) || d.token : d.token;
  if (guard >= 4 && chatShape(d.token, t, brands)) return "never";
  if (/^[\d.,%\-]+$/.test(t)) return "never";
  if (/^[A-Z][a-z]+$/.test(t)) return "never";
  if (/^[A-Z0-9][A-Za-z0-9]*(-[A-Za-z0-9]+)+$/.test(t)) return "never";
  if (t.length >= 2 && /^[A-Z0-9]+$/.test(t)) return "never";
  if (guard >= 3 && /^[A-Za-z]?[a-z]+([A-Z][a-z]+)+$/.test(t)) return "never";
  if (d.enHit && /^[A-Za-z]+$/.test(d.token)) return "enword";
  return "ok";
}
/** Back-compat boolean form: true only for "ok". */
export function ctxEligible(d: { token: string; enHit: boolean }): boolean { return ctxEligibility(d) === "ok"; }

export interface CtxToken { valid: boolean; score: number; eligible?: boolean | Eligibility; len?: number }

/**
 * Line-level flip decisions.
 * 1. Pass 1: flip tokens whose own score > threshold.
 * 2. Whole-line prior: if pass-1 flipped >= lineMinFlips tokens and that is >= lineMinFrac of the convertible
 *    tokens (finite score; unflipped tokens of <= 2 chars are left out of the denominator, since they are what
 *    the prior is meant to decide), every valid eligible token gets +wLine.
 * 3. Neighbour boost (iterative): +wCtx per flipped neighbour within ctxWindow, only for "ok" tokens with score >= ctxFloor.
 * Flips are only added; a line with no confident Thai token is never changed.
 */
export function contextFlips(toks: CtxToken[], p: Params): boolean[] {
  const flip = toks.map((t) => t.valid && t.score > p.threshold);
  const elig = toks.map((t): Eligibility => t.eligible === false ? "never" : t.eligible === true || t.eligible === undefined ? "ok" : t.eligible);
  const wl = p.wLine ?? 0;
  const bonus = toks.map(() => 0);
  if (wl > 0) {
    const nFlip = flip.filter(Boolean).length, nConv = toks.filter((t, i) => Number.isFinite(t.score) && (flip[i] || (t.len ?? 99) > 2)).length;
    if (nFlip >= (p.lineMinFlips ?? 2) && nFlip / Math.max(1, nConv) >= (p.lineMinFrac ?? 0.8)) {
      toks.forEach((t, i) => {
        const e = elig[i];
        if (e === "ok" || (e === "enword" && p.lineAllowEn)) bonus[i] = wl;
      });
      toks.forEach((t, i) => { if (!flip[i] && t.valid && t.score + bonus[i] > p.threshold) flip[i] = true; });
    }
  }
  const w = p.wCtx ?? 0, win = p.ctxWindow ?? 1;
  if (w <= 0) return flip;
  for (let iter = 0; iter < 5; iter++) {
    let changed = false;
    for (let i = 0; i < toks.length; i++) {
      if (flip[i] || !toks[i].valid || elig[i] !== "ok" || toks[i].score < (p.ctxFloor ?? -Infinity)) continue;
      let n = 0;
      for (let j = Math.max(0, i - win); j <= Math.min(toks.length - 1, i + win); j++) if (j !== i && flip[j]) n++;
      if (n > 0 && toks[i].score + bonus[i] + w * n > p.threshold) { flip[i] = true; changed = true; }
    }
    if (!changed) break;
  }
  return flip;
}

/** Collapse runs of >= 3 identical characters down to `keep` (scoring only; output text is untouched). */
export function collapseRuns(s: string, keep: number): string {
  const ch = [...s]; let o = "", run = 0;
  for (let i = 0; i < ch.length; i++) {
    run = i > 0 && ch[i] === ch[i - 1] ? run + 1 : 1;
    let j = i; while (j + 1 < ch.length && ch[j + 1] === ch[i]) j++;
    const len = j - i + run; // total run length containing i
    if (len >= 3 && run > keep) continue;
    o += ch[i];
  }
  return o;
}

const EMOTICON = /^(?:[:;=8xX][-'^o]?[()\[\]DPpOo3|\\/*$@]+|[()\[\]DPp][-']?[:;=]|T[_^.\-]?T|[;:]_+[;:]|;[-_wWoO.];|[xX]D+|[oO0][_.][oO0]|>[_.]<|\^[_.\-]?\^|-[_.]-|<3+|[:;]v|:'\(|555+)$/;
const UNIT = /^\d+(?:[.,]\d+)?(?:ml|mg|g|kg|km|cm|mm|k|gb|mb|tb|hp|cc|w|kw|mah|ghz|hz|ms|px|th|st|nd|rd|x)$/i;
const MODEL = /^[A-Za-z]{1,6}-?\d+[A-Za-z+]*$/;
/**
 * Chat-specific "never boost" shapes (guard 4): hashtags, mentions, URLs/domains, emoticons, numbers with
 * units, alphanumeric model codes (cx5, mg5, spf50+), and tokens on the brand/abbreviation list.
 */
export function chatShape(token: string, core: string, brands?: Set<string>): boolean {
  if (/^[#@]/.test(token) || /^[#@]/.test(core)) return true;
  if (/^(https?:|www\.)|\.(com|net|org|co|th|io|ly|me)(\/|$)/i.test(token)) return true;
  if (EMOTICON.test(token) || EMOTICON.test(core)) return true;
  if (UNIT.test(core) || MODEL.test(core)) return true;
  if (brands && brands.has(core.toLowerCase())) return true;
  return false;
}
