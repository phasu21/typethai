// Dump TS reference outputs for the Rust parity harness (rust/crates/typethai-core/src/bin/tt-parity.rs).
// Inputs: typed forms of dev/test lines (Thai gold -> EN keys per token), English, code and chat dev lines; corrector words.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { Detector } from "../src/detector.ts";
import { thToEn } from "../src/keymap.ts";
import { normalizeThai } from "../src/normalize.ts";
import { Corrector } from "../src/corrector.ts";
import { lineProbs, type FlipModel } from "../src/classifier.ts";
import { loadBundle, sample, rng } from "./evallib.ts";

const { bundle } = loadBundle();
const wNorm = JSON.parse(readFileSync("data/model/corrector.json", "utf8")).wNorm;
const detRules = new Detector({ ...bundle, params: { ...bundle.params, normalize: true, wNorm } });
const detLearn = new Detector({ ...bundle, brands: [], params: { ...bundle.params, normalize: true } });
const model = JSON.parse(readFileSync("data/model/flip-classifier.json", "utf8")) as FlipModel;
const freq = JSON.parse(readFileSync("data/model/wordfreq.json", "utf8"));
const corr = new Corrector(bundle.thaiWords, freq, bundle.thai, JSON.parse(readFileSync("data/model/corrector.json", "utf8")));
const THAI = /[\u0E00-\u0E7F]/;
const typed = (l: string) => l.split(/(\s+)/).map((w) => (THAI.test(w) ? thToEn(w) ?? w : w)).join("");
const J = (p: string) => JSON.parse(readFileSync(p, "utf8"));
const N = Number(process.argv[2] ?? 1500);
const sets: Record<string, string[]> = {
  "th.dev": sample(J("data/splits/th.dev.json").map((x: any) => typed(x.gold)), N),
  "th.test": sample(J("data/splits/th.test.json").map((x: any) => typed(x.gold)), N),
  "en.dev": sample(J("data/splits/en.dev.json"), N), "en.test": sample(J("data/splits/en.test.json"), N),
  "code.dev": sample(J("data/splits/code.dev.json"), N),
  "chat.dev": sample((J("data/chat/dev.json") as any[]).map((x) => typeof x === "string" ? x : x.text ?? x.gold).filter(Boolean).map(typed), N),
};
const fin = (x: number) => (Number.isFinite(x) ? x : null);
mkdirSync("data/parity", { recursive: true });
const out: string[] = [];
for (const [set, lines] of Object.entries(sets)) for (const line of lines) {
  const r = detRules.convert(line);
  const learn = detLearn.convert(line).decisions;
  out.push(JSON.stringify({ set, line, output: r.output, flips: r.decisions.map((d) => d.flip), scores: r.decisions.map((d) => fin(d.score)), probs: lineProbs(model, learn) }));
}
writeFileSync("data/parity/lines.jsonl", out.join("\n") + "\n");
// corrector: dev Thai words, half with one deterministic char deletion/duplication
const R = rng(7), words = sample([...new Set(J("data/splits/th.dev.json").flatMap((x: any) => x.gold.split(/\s+/)).filter((w: string) => THAI.test(w) && [...w].length >= 3))] as string[], 2000);
const cw = words.map((w) => { const c = [...w], i = Math.floor(R() * c.length); return R() < 0.5 ? w : R() < 0.5 ? [...c.slice(0, i), ...c.slice(i + 1)].join("") : [...c.slice(0, i), c[i], ...c.slice(i)].join(""); });
writeFileSync("data/parity/corrector.jsonl", cw.map((w) => { const c = corr.suggest(w, 3); return JSON.stringify({ w, norm: normalizeThai(w), apply: c.apply, top: c.suggestions.map((s) => s.word), conf: c.suggestions.map((s) => s.confidence), keep: c.keepConfidence }); }).join("\n") + "\n");
console.log(`lines ${out.length}, corrector ${cw.length}`);
// personal: deterministic feedback replay over chat.dev lines; record probs after each event + final state
import { PersonalModel, DEFAULT_PERSONAL } from "../src/personal.ts";
const pm = new PersonalModel(model, undefined, { ...DEFAULT_PERSONAL, trust: 0.5, memPurity: 0.8 });
const pl: string[] = [];
sets["chat.dev"].slice(0, 400).forEach((line, n) => {
  const ds = detLearn.convert(line).decisions; if (!ds.length) return;
  const i = n % ds.length, kind = n % 3 === 0 ? "retype" : "undo";
  pm.feedback(ds, i, kind); pl.push(JSON.stringify({ line, i, kind, probs: pm.lineProbs(ds) }));
});
pm.compact();
writeFileSync("data/parity/personal.jsonl", pl.join("\n") + "\n");
writeFileSync("data/parity/personal-state.json", JSON.stringify(pm.toJSON()));
