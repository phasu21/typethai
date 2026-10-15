// On-device personal model: base flip classifier + per-user stage-2 weight delta trained by online SGD from implicit
// feedback, plus a frequency-decayed personal token memory. Pure TS; persistence via toJSON/fromJSON (see personal-store.ts).
import { tokenFeatures, contextFeatures, lrPredict, type FlipModel } from "./classifier.ts";
import type { TokenDecision } from "./detector.ts";

export interface PersonalConfig {
  lr: number;          // SGD step on standardized features
  l2: number;          // pull of delta toward 0 (= toward base weights)
  maxDrift: number;    // per-weight |delta| cap, and cap on the total personal logit shift
  memWeight: number;   // logit weight of the token-memory term
  memDecay: number;    // per-event multiplicative decay of memory counts
  memOverride: number; // |decayed net count| at which memory overrides the model outright
  trust: number;       // 0..1: scale SGD steps down when the BASE model is confident (robustness to wrong signals)
  memPurity: number;   // override only if the memory is at least this one-sided (conv or kept share)
}
export const DEFAULT_PERSONAL: PersonalConfig = { lr: 0.05, l2: 0.01, maxDrift: 1.0, memWeight: 2.0, memDecay: 0.999, memOverride: 3, trust: 0, memPurity: 0 };

export type FeedbackKind = "undo" | "retype"; // undo/backspace after a flip => should NOT flip; manual switch+retype => SHOULD flip
interface Mem { kept: number; conv: number; t: number }
export interface PersonalState { version: 1; dw: number[]; db: number; mem: Record<string, Mem>; events: number; config: PersonalConfig }

const sig = (z: number) => 1 / (1 + Math.exp(-z));
const logit = (p: number) => Math.log(Math.max(1e-9, p) / Math.max(1e-9, 1 - p));

export class PersonalModel {
  readonly base: FlipModel; state: PersonalState;
  constructor(base: FlipModel, state?: PersonalState, config: Partial<PersonalConfig> = {}) {
    this.base = base;
    if (state && state.config.memOverride === null) state.config.memOverride = Infinity; // JSON has no Infinity
    this.state = state ?? PersonalModel.fresh(base, { ...DEFAULT_PERSONAL, ...config });
  }
  static fresh(base: FlipModel, config: PersonalConfig): PersonalState {
    return { version: 1, dw: new Array(base.stage2.w.length).fill(0), db: 0, mem: {}, events: 0, config };
  }
  reset(): void { this.state = PersonalModel.fresh(this.base, this.state.config); }

  private key(d: TokenDecision) { return d.token.toLowerCase(); }
  private memNet(k: string): number {
    const m = this.state.mem[k]; if (!m) return 0;
    const f = this.state.config.memDecay ** (this.state.events - m.t);
    return (m.conv - m.kept) * f;
  }
  private memPure(k: string): boolean {
    const m = this.state.mem[k]; if (!m) return false;
    return Math.max(m.conv, m.kept) / Math.max(1e-9, m.conv + m.kept) >= this.state.config.memPurity;
  }
  private x2(ds: TokenDecision[], p1: number[], i: number) { return [...tokenFeatures(ds[i]), ...contextFeatures(p1, i)]; }
  private z2(x: number[]): number[] { const s = this.base.stage2; return x.map((v, k) => (v - s.mu[k]) / s.sd[k]); }

  private baseLogit(z: number[]): number { const s2 = this.base.stage2; let a = s2.b; for (let k = 0; k < z.length; k++) a += s2.w[k] * z[k]; return a; }
  /**
   * The personal logit shift, capped at maxDrift like each weight. A rare feature has a large standardized value
   * (one-word line: z ~ 6.5), so a few same-shaped events would otherwise move every such token by several logits.
   */
  private shift(z: number[]): number {
    const { dw, db, config } = this.state; let d = db; for (let k = 0; k < z.length; k++) d += dw[k] * z[k];
    return clamp(d, config.maxDrift);
  }
  /** Personal flip probabilities for one line (0 for tokens that can't flip). */
  lineProbs(ds: TokenDecision[]): number[] {
    const { config } = this.state;
    const p1 = ds.map((d) => lrPredict(this.base.stage1, tokenFeatures(d)));
    return ds.map((d, i) => {
      if (!d.valid || d.reason === "not convertible") return 0;
      const z = this.z2(this.x2(ds, p1, i));
      const a = this.baseLogit(z) + this.shift(z);
      const net = this.memNet(this.key(d));
      if (Math.abs(net) >= (config.memOverride ?? Infinity) && this.memPure(this.key(d))) return net > 0 ? 1 : 0;
      return sig(a + config.memWeight * Math.tanh(net / 2));
    });
  }
  lineFlips(ds: TokenDecision[]): boolean[] { return this.lineProbs(ds).map((p) => p > this.base.threshold); }

  /**
   * One feedback event on token i of a line. One SGD step on the stage-2 delta (log-loss toward the implied label,
   * with the decision threshold folded in so the target is "cross the threshold"), L2 pull toward base, drift cap;
   * then update the token memory.
   */
  feedback(ds: TokenDecision[], i: number, kind: FeedbackKind): void {
    const y = kind === "retype" ? 1 : 0, { config } = this.state;
    const p1 = ds.map((d) => lrPredict(this.base.stage1, tokenFeatures(d)));
    if (ds[i].valid && ds[i].reason !== "not convertible") {
      const z = this.z2(this.x2(ds, p1, i)), dw = this.state.dw;
      const ab = this.baseLogit(z), a = ab + this.shift(z);
      const p = sig(a - logit(this.base.threshold)); // probability relative to the decision boundary
      const pb = sig(ab - logit(this.base.threshold)), conf = Math.abs(2 * pb - 1); // base confidence 0..1
      const g = (p - y) * (1 - config.trust * conf);
      for (let k = 0; k < z.length; k++) dw[k] = clamp(dw[k] - config.lr * (g * z[k] + config.l2 * dw[k]), config.maxDrift);
      this.state.db = clamp(this.state.db - config.lr * (g + config.l2 * this.state.db), config.maxDrift);
    }
    this.state.events++;
    const k = this.key(ds[i]), m = this.state.mem[k] ?? { kept: 0, conv: 0, t: this.state.events };
    const f = config.memDecay ** (this.state.events - m.t);
    m.kept *= f; m.conv *= f; m.t = this.state.events;
    if (y) m.conv += 1; else m.kept += 1;
    this.state.mem[k] = m;
  }
  /** Drop near-zero memory entries (keeps the state small). */
  compact(min = 0.05): void {
    for (const [k, m] of Object.entries(this.state.mem)) { const f = this.state.config.memDecay ** (this.state.events - m.t); if ((m.kept + m.conv) * f < min) delete this.state.mem[k]; }
  }
  toJSON(): PersonalState { return this.state; }
}
const clamp = (v: number, c: number) => Math.max(-c, Math.min(c, v));
