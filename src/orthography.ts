// Minimal Thai orthographic validity rules (mark placement). Returns true if the string is plausible Thai.
const CONS = /[\u0E01-\u0E2E]/;           // ก..ฮ
const ABOVE_BELOW_VOWEL = /[\u0E31\u0E34-\u0E3A\u0E47\u0E4D]/; // ั ิ ี ึ ื ุ ู ฺ ็ ํ
const TONE = /[\u0E48-\u0E4B]/;           // ่ ้ ๊ ๋
const THANTHAKHAT = "\u0E4C";             // ์
const LEADING = /[\u0E40-\u0E44]/;        // เ แ โ ใ ไ
const FOLLOW = /[\u0E30\u0E32\u0E33\u0E45]/; // ะ า ำ ๅ
const THAI = /[\u0E00-\u0E7F]/;

export interface Validity { valid: boolean; reason?: string }

export function checkThai(s: string): Validity {
  const ch = [...s];
  for (let i = 0; i < ch.length; i++) {
    const c = ch[i], p = ch[i - 1] ?? "", pp = ch[i - 2] ?? "";
    if (ABOVE_BELOW_VOWEL.test(c)) {
      if (!CONS.test(p)) return { valid: false, reason: `vowel mark ${c} after non-consonant at ${i}` };
    } else if (TONE.test(c) || c === THANTHAKHAT) {
      const okBase = CONS.test(p) || (ABOVE_BELOW_VOWEL.test(p) && CONS.test(pp));
      if (!okBase) return { valid: false, reason: `tone/mark ${c} misplaced at ${i}` };
    } else if (LEADING.test(c)) {
      const n = ch[i + 1];
      if (n === undefined || !CONS.test(n)) return { valid: false, reason: `leading vowel ${c} not followed by consonant` };
    } else if (FOLLOW.test(c)) {
      if (i === 0 && c !== "\u0E45") return { valid: false, reason: `${c} at start` };
      if (i > 0 && !(CONS.test(p) || TONE.test(p) || ABOVE_BELOW_VOWEL.test(p) || p === "\u0E32" || p === "\u0E30" || p === THANTHAKHAT)) {
        return { valid: false, reason: `${c} after ${p}` };
      }
    }
  }
  if (!ch.some((c) => THAI.test(c))) return { valid: false, reason: "no thai" };
  return { valid: true };
}
