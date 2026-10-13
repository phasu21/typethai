import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { enToTh, thToEn, EN_TO_TH, TH_TO_EN, checkThai, trainNgram, meanLogProb, Dict, Detector } from "../src/index.ts";

test("keymap: canonical example", () => {
  assert.equal(enToTh("l;ylfu"), "สวัสดี");
  assert.equal(thToEn("สวัสดี"), "l;ylfu");
});
test("keymap: covers 94 printable keys incl. shifted", () => {
  assert.equal(EN_TO_TH.size, 94);

  assert.equal(enToTh("{"), "ฐ"); assert.equal(enToTh("<"), "ฒ"); assert.equal(enToTh("H"), "็");
});
test("keymap: Thai->EN round-trips for every Thai output", () => {
  for (const [th, en] of TH_TO_EN) assert.equal(enToTh(en), th);
  assert.equal(thToEn("abc"), null);
});
test("orthography: valid and invalid", () => {
  for (const w of ["สวัสดี", "ภาษาไทย", "กำแพง", "น้ำ", "เมือง", "ที่", "ครับ"]) assert.ok(checkThai(w).valid, w);
  for (const w of ["ัก", "่ก", "เ", "าก", "กเ"]) assert.equal(checkThai(w).valid, false, w);
});
test("ngram: in-domain text scores higher", () => {
  const m = trainNgram(["hello world", "hello there", "world peace"]);
  assert.ok(meanLogProb(m, "hello") > meanLogProb(m, "zqxjv"));
});
test("dict coverage", () => {
  const d = new Dict(["สวัส", "ดี", "สวัสดี"]);
  assert.equal(d.coverage("สวัสดี"), 1);
  assert.ok(d.coverage("สวัสดีกก") < 1);
});
test("detector with toy model flips Thai, keeps English", () => {
  const th = ["สวัสดี", "ครับ", "ภาษาไทย", "ขอบคุณ", "สวัสดีครับ"];
  const en = ["hello", "world", "the", "quick", "brown", "fox", "thanks"];
  const det = new Detector({ thai: trainNgram(th), english: trainNgram(en), thaiWords: th, englishWords: en,
    params: { wDict: 2, wEnDict: 2, wShort: 0, threshold: 0 } });
  assert.equal(det.convert("l;ylfu hello").output, "สวัสดี hello");
});
const MODEL = "data/model/model.json";
test("trained model: real sentences", { skip: !existsSync(MODEL) && "run npm run build-model first" }, () => {
  const det = new Detector(JSON.parse(readFileSync(MODEL, "utf8")));
  assert.equal(det.convert("l;ylfu").output, "สวัสดี");
  assert.equal(det.convert("The quick brown fox jumps over the lazy dog.").output, "The quick brown fox jumps over the lazy dog.");
  assert.equal(det.convert("const x = foo(bar);").output, "const x = foo(bar);");
});

import { contextFlips, ctxEligible } from "../src/index.ts";
const P = { wDict: 0, wEnDict: 0, wShort: 0, threshold: 0 };
test("context: disabled when wCtx=0", () => {
  assert.deepEqual(contextFlips([{ valid: true, score: 1 }, { valid: true, score: -0.3 }], { ...P, wCtx: 0 }), [true, false]);
});
test("context: neighbour boost flips a weak token, propagates iteratively", () => {
  const toks = [{ valid: true, score: 2 }, { valid: true, score: -0.3 }, { valid: true, score: -0.3 }];
  assert.deepEqual(contextFlips(toks, { ...P, wCtx: 0.5, ctxWindow: 1 }), [true, true, true]);
});
test("context: never boosts without a confident neighbour, invalid, ineligible, or below floor", () => {
  assert.deepEqual(contextFlips([{ valid: true, score: -0.1 }, { valid: true, score: -0.1 }], { ...P, wCtx: 5 }), [false, false]);
  const toks = [{ valid: true, score: 2 }, { valid: false, score: -0.1 }, { valid: true, score: -0.1, eligible: false }, { valid: true, score: -3 }];
  assert.deepEqual(contextFlips(toks, { ...P, wCtx: 5, ctxWindow: 3, ctxFloor: -1.5 }), [true, false, false, false]);
});
test("context guard: English-looking shapes are ineligible", () => {
  for (const t of ["so", "8", "4,696", "Luciano", "PGM-FI", "TN"]) assert.equal(ctxEligible({ token: t, enHit: t === "so" }), false, t);
  for (const t of ["]e", "di,", "ohe", "go"]) assert.equal(ctxEligible({ token: t, enHit: false }), true, t);
});
test("trained model: context fixes short word next to Thai", { skip: !existsSync(MODEL) && "no model" }, () => {
  const det = new Detector(JSON.parse(readFileSync(MODEL, "utf8")));
  assert.ok((det.params.wCtx ?? 0) > 0);
  assert.equal(det.convert("The quick brown fox jumps over the lazy dog.").output, "The quick brown fox jumps over the lazy dog.");
});

import { ctxEligibility, coreOf } from "../src/index.ts";
test("guard v3: edge punctuation stripped, CamelCase caught; v2 misses them", () => {
  assert.equal(coreOf("(Silvio"), "Silvio"); assert.equal(coreOf("Adhikari)"), "Adhikari");
  for (const t of ["(Silvio", "Adhikari)", "ChoiKoon", "iPhone", "\"Hello"]) {
    assert.equal(ctxEligibility({ token: t, enHit: false }, 3), "never", t);
    assert.equal(ctxEligibility({ token: t, enHit: false }, 2), "ok", t);
  }
  assert.equal(ctxEligibility({ token: "or", enHit: true }), "enword");
  assert.equal(ctxEligibility({ token: "]e", enHit: false }), "ok");
});
test("line prior: boosts weak tokens only when the rest of the line is Thai", () => {
  const p = { ...P, wCtx: 0, wLine: 2, lineMinFrac: 0.8, lineMinFlips: 2 };
  const thaiLine = [{ valid: true, score: 3, len: 6 }, { valid: true, score: 2, len: 5 }, { valid: true, score: -1.5, len: 2 }];
  assert.deepEqual(contextFlips(thaiLine, p), [true, true, true]);
  // only one confident Thai token -> below lineMinFlips
  assert.deepEqual(contextFlips([{ valid: true, score: 3, len: 6 }, { valid: true, score: -1.5, len: 2 }], p), [true, false]);
  // mostly-English line (long unflipped tokens count in the denominator) -> no prior
  const mixed = [{ valid: true, score: 3, len: 6 }, { valid: true, score: 2, len: 5 }, ...Array.from({ length: 4 }, () => ({ valid: true, score: -2, len: 6 }))];
  assert.deepEqual(contextFlips(mixed, p), [true, true, false, false, false, false]);
  // "enword" only boosted when lineAllowEn
  const en = [...thaiLine.slice(0, 2), { valid: true, score: -1.5, len: 2, eligible: "enword" as const }];
  assert.equal(contextFlips(en, p)[2], false);
  assert.equal(contextFlips(en, { ...p, lineAllowEn: true })[2], true);
});

import { collapseRuns, chatShape } from "../src/index.ts";
test("v4: collapseRuns (scoring only)", () => {
  assert.equal(collapseRuns("แดกกก", 1), "แดก");
  assert.equal(collapseRuns("sooooo", 2), "soo");
  assert.equal(collapseRuns("กก", 1), "กก"); // runs < 3 untouched
  assert.equal(collapseRuns("good", 2), "good");
});
test("v4: chat shapes are never boosted", () => {
  for (const t of ["#iflix", "@user", "https://x.co", "google.com", "T^T", "xD", ";-;", ":)", "5555", "cx5", "spf50+", "90ml"])
    assert.ok(chatShape(t, coreOf(t) || t), t);
  for (const t of ["]e", "8ht", "di,", "l;ylfu"]) assert.equal(chatShape(t, coreOf(t) || t), false, t);
  assert.ok(chatShape("mk", "mk", new Set(["mk"])));
  assert.equal(ctxEligibility({ token: "mk", enHit: false }, 4, new Set(["mk"])), "never");
  assert.equal(ctxEligibility({ token: "mk", enHit: false }, 3, new Set(["mk"])), "ok");
});
test("v4 trained model: chat line keeps brand next to Thai", { skip: !existsSync(MODEL) && "no model" }, () => {
  const det = new Detector(JSON.parse(readFileSync(MODEL, "utf8")));
  if (det.params.guard !== 4) return;
  const out = det.convert("vdkd'd;kdkd mk ;yogp^").output; // mistyped Thai around "mk"
  assert.ok(out.includes(" mk "), out);
});

import { normalizeThai, Corrector, keyDistance, neighbourChars, shiftPartner, segment } from "../src/index.ts";
test("v5 normalize: common orthographic typos", () => {
  const cases: [string, string][] = [["เเล้ว", "แล้ว"], ["น้ํา", "น้ำ"], ["นำ้", "น้ำ"], ["ร้่าน", "ร้าน"], ["ที่่", "ที่"], ["คะ่", "ค่ะ"], ["ชื่ิอ", "ชื่อ"], ["ั้ก", "ก"]];
  for (const [a, b] of cases) assert.equal(normalizeThai(a), b, a);
  for (const w of ["สวัสดี", "ตั้ง", "น้ำ", "ค่ะ", "กำแพง", "เมือง", "แล้ว"]) assert.equal(normalizeThai(w), w, w);
});
test("v5 keyboard model: neighbours, shift partner, weighted distance", () => {
  assert.ok(neighbourChars("ก").includes("ด")); assert.equal(shiftPartner("ก"), "ฏ");
  assert.equal(keyDistance("สวัสดี", "สวัสดี"), 0);
  assert.ok(keyDistance("สวัสกี", "สวัสดี") < keyDistance("สวัสมี", "สวัสดี")); // adjacent < non-adjacent
  assert.ok(keyDistance("สวัสดีี", "สวัสดี") < 1); // duplication cheap
});
test("v5 corrector: ranked suggestions + confidence, auto-apply only above tau", () => {
  const words = ["สวัสดี", "สวัส", "ดี", "ครับ", "ขอบคุณ"];
  const thai = trainNgram(words);
  const c = new Corrector(words, { สวัสดี: 50, ครับ: 40, ขอบคุณ: 30 }, thai, { tau: 0.5 });
  const r = c.suggest("สวัสกี");
  assert.equal(r.suggestions[0].word, "สวัสดี");
  assert.ok(r.suggestions[0].confidence > 0 && r.suggestions[0].confidence <= 1);
  assert.equal(c.suggest("ครับ").inDict, true);
  const strict = new Corrector(words, {}, thai, { tau: 1.01 });
  assert.equal(strict.suggest("สวัสกี").apply, null);
});
test("v5 segment", () => {
  const has = (w: string) => ["สวัสดี", "ครับ"].includes(w);
  assert.deepEqual(segment("สวัสดีครับ", has), ["สวัสดี", "ครับ"]);
});
test("v5 detector: normalisation rescues an orthographic typo", () => {
  const th = ["แล้ว", "ไป", "แล้วไป"], en = ["hello", "world"];
  const mk = (normalize: boolean) => new Detector({ thai: trainNgram(th), english: trainNgram(en), thaiWords: th, englishWords: en,
    params: { wDict: 2, wEnDict: 0, wShort: 0, threshold: 0, normalize, wNorm: 0 } });
  const typed = thToEn("เเล้ว")!; // แล้ว typed with a double sara e
  assert.equal(enToTh(typed), "เเล้ว");
  assert.equal(mk(false).scoreToken(typed).valid, false);
  assert.equal(mk(true).scoreToken(typed).candidate, "แล้ว");
  assert.equal(mk(true).scoreToken(typed).valid, true);
});

import { trainLR, lrPredict, tokenFeatures, contextFeatures, lineFlips, FEATURE_NAMES, CONTEXT_NAMES } from "../src/index.ts";
test("v6 LR: learns a separable toy problem", () => {
  const X = Array.from({ length: 200 }, (_, i) => [i % 2 ? 1 + (i % 7) / 10 : -1 - (i % 5) / 10, (i % 3) / 3]);
  const y = X.map((x) => (x[0] > 0 ? 1 : 0));
  const m = trainLR(X, y, { epochs: 200 });
  assert.ok(lrPredict(m, [1.5, 0]) > 0.9); assert.ok(lrPredict(m, [-1.5, 0]) < 0.1);
});
test("v6 features: fixed lengths matching names", () => {
  const d = { token: "l;ylfu", candidate: "สวัสดี", flip: false, score: 1, valid: true, llTh: -2, llEn: -5, covTh: 1, enHit: false, normEdits: 0 };
  assert.equal(tokenFeatures(d).length, FEATURE_NAMES.length);
  assert.equal(contextFeatures([0.9, 0.1, 0.8], 1).length, CONTEXT_NAMES.length);
});
const CLF = "data/model/flip-classifier.json";
test("v6 trained classifier: flips Thai, keeps English", { skip: !(existsSync(CLF) && existsSync(MODEL)) && "no model" }, () => {
  const b = JSON.parse(readFileSync(MODEL, "utf8"));
  const det = new Detector({ ...b, brands: [], params: { ...b.params, normalize: true } });
  const m = JSON.parse(readFileSync(CLF, "utf8"));
  const line = (s: string) => lineFlips(m, s.split(" ").map((t) => det.scoreToken(t)));
  assert.deepEqual(line("l;ylfu 8iy[ the"), [true, true, false]);
  assert.ok(line("The quick brown fox jumps over the lazy dog").every((f) => !f));
});

import { PersonalModel, savePersonal, loadPersonal, resetPersonal } from "../src/index.ts";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
test("v7 personal model: feedback, drift cap, memory, persistence, reset", { skip: !(existsSync(CLF) && existsSync(MODEL)) && "no model" }, () => {
  const b = JSON.parse(readFileSync(MODEL, "utf8"));
  const det = new Detector({ ...b, brands: [], params: { ...b.params, normalize: true } });
  const base = JSON.parse(readFileSync(CLF, "utf8"));
  const ds = "l;ylfu 8iy[ omakase".split(" ").map((t) => det.scoreToken(t));
  const pm = new PersonalModel(base, undefined, { lr: 0.5, maxDrift: 0.2, memOverride: 2, memPurity: 0.8 });
  const p0 = pm.lineProbs(ds)[0];
  for (let i = 0; i < 50; i++) pm.feedback(ds, 0, "undo");
  assert.ok(pm.state.dw.every((w) => Math.abs(w) <= 0.2 + 1e-12));     // drift cap
  assert.equal(pm.lineProbs(ds)[0], 0);                                  // memory override after repeated undo
  assert.ok(pm.lineProbs(ds)[0] < p0);
  const dir = mkdtempSync(join(tmpdir(), "tt-")), path = join(dir, "p.json");
  pm.state.config.memOverride = Infinity;                                // Infinity must survive JSON
  savePersonal(pm, path);
  const re = loadPersonal(base, path);
  assert.equal(re.state.config.memOverride, Infinity);
  assert.deepEqual(re.lineProbs(ds), pm.lineProbs(ds));
  resetPersonal(re, path);
  assert.equal(existsSync(path), false);
  assert.deepEqual(re.lineProbs(ds), new PersonalModel(base).lineProbs(ds)); // back to base
});
