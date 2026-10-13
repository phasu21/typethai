// Character trigram model with interpolated backoff. Serializable to plain JSON.
export interface NgramModel { order: 3; counts: Record<string, number>; vocab: number; lambdas: [number, number, number, number] }

const BOS = "\u0002", EOS = "\u0003";

export function trainNgram(texts: Iterable<string>): NgramModel {
  const counts: Record<string, number> = {};
  const chars = new Set<string>();
  const add = (k: string) => { counts[k] = (counts[k] ?? 0) + 1; };
  for (const t of texts) {
    const s = [BOS, BOS, ...t, EOS];
    for (let i = 2; i < s.length; i++) {
      chars.add(s[i]);
      add("1|" + s[i]); add("1|");
      add("2|" + s[i - 1] + s[i]); add("2c|" + s[i - 1]);
      add("3|" + s[i - 2] + s[i - 1] + s[i]); add("3c|" + s[i - 2] + s[i - 1]);
    }
  }
  return { order: 3, counts, vocab: chars.size + 1, lambdas: [0.6, 0.3, 0.09, 0.01] };
}

/** Mean natural-log probability per character (incl. end marker). */
export function meanLogProb(m: NgramModel, t: string): number {
  const c = m.counts, [l3, l2, l1, l0] = m.lambdas;
  const s = [BOS, BOS, ...t, EOS];
  const total = c["1|"] ?? 1;
  let lp = 0;
  for (let i = 2; i < s.length; i++) {
    const a = s[i - 2], b = s[i - 1], x = s[i];
    const c3d = c["3c|" + a + b], c2d = c["2c|" + b];
    const p3 = c3d ? (c["3|" + a + b + x] ?? 0) / c3d : 0;
    const p2 = c2d ? (c["2|" + b + x] ?? 0) / c2d : 0;
    const p1 = (c["1|" + x] ?? 0) / total;
    lp += Math.log(l3 * p3 + l2 * p2 + l1 * p1 + l0 / m.vocab);
  }
  return lp / (s.length - 2);
}

/** Drop rare n-grams to shrink the model. */
export function prune(m: NgramModel, minCount: number): NgramModel {
  const counts: Record<string, number> = {};
  for (const [k, v] of Object.entries(m.counts)) if (v >= minCount || k.startsWith("1|") || k.includes("c|")) counts[k] = v;
  return { ...m, counts };
}
