// Dictionary coverage via DP segmentation: fraction of characters covered by known words.
export class Dict {
  private words: Set<string>;
  private maxLen: number;
  constructor(words: Iterable<string>) {
    this.words = new Set();
    this.maxLen = 1;
    for (const w of words) { if (!w) continue; this.words.add(w); this.maxLen = Math.max(this.maxLen, [...w].length); }
    this.maxLen = Math.min(this.maxLen, 24);
  }
  has(w: string): boolean { return this.words.has(w); }
  /** Max chars covered by dictionary words of length >= minLen (single-char words don't count). */
  coverage(s: string, minLen = 2): number {
    const ch = [...s], n = ch.length;
    if (n === 0) return 0;
    const best = new Array<number>(n + 1).fill(0);
    for (let i = 1; i <= n; i++) {
      best[i] = best[i - 1];
      for (let L = minLen; L <= Math.min(this.maxLen, i); L++) {
        if (this.words.has(ch.slice(i - L, i).join(""))) best[i] = Math.max(best[i], best[i - L] + L);
      }
    }
    return best[n] / n;
  }
}
