// Generic Thai orthographic normalisation (no per-word rules).
const CONS = /[\u0E01-\u0E2E]/;
const VOWEL_MARK = /[\u0E31\u0E34-\u0E3A\u0E47\u0E4D]/; // ั ิ ี ึ ื ุ ู ฺ ็ ํ
const TONE = /[\u0E48-\u0E4B]/;
const KARAN = "\u0E4C";
const isMark = (c: string) => VOWEL_MARK.test(c) || TONE.test(c) || c === KARAN;

/**
 * - เเ -> แ ; ํา (nikhahit + sara aa) -> ำ
 * - per consonant cluster: drop duplicate marks, keep at most one vowel mark and one tone mark (the first),
 *   and order them vowel mark -> tone -> thanthakhat
 * - tone written after ำ moves before it (น ำ ้ -> น ้ ำ)
 * - a tone typed after a following ะ/า moves into the consonant cluster (คะ่ -> ค่ะ)
 * - other combining marks with no base consonant are dropped
 */
export function normalizeThai(s: string): string {
  let t = s.replace(/\u0E40\u0E40/g, "\u0E41").replace(/\u0E4D([\u0E48-\u0E4B]?)\u0E32/g, "$1\u0E33");
  const ch = [...t], out: string[] = [];
  let i = 0;
  while (i < ch.length) {
    const c = ch[i];
    if (isMark(c)) {
      // marks run: find base
      const run: string[] = [];
      while (i < ch.length && isMark(ch[i])) run.push(ch[i++]);
      const base = out[out.length - 1];
      if (base === undefined || !(CONS.test(base))) {
        // misordered: tone typed after a following vowel (คะ่ -> ค่ะ, กา้ -> ก้า): move it into the consonant cluster
        const tn = run.find((m) => TONE.test(m));
        if (tn && (base === "\u0E30" || base === "\u0E32")) {
          let j = out.length - 2;
          while (j >= 0 && isMark(out[j])) j--;
          if (j >= 0 && CONS.test(out[j]) && !out.slice(j + 1, out.length - 1).some((m) => TONE.test(m))) {
            let ins = j + 1; while (ins < out.length - 1 && VOWEL_MARK.test(out[ins])) ins++;
            out.splice(ins, 0, tn);
          }
        }
        continue; // other stray marks are dropped
      }
      const v = run.find((m) => VOWEL_MARK.test(m)), tn = run.find((m) => TONE.test(m)), k = run.includes(KARAN);
      if (v) out.push(v); if (tn) out.push(tn); if (k) out.push(KARAN);
      continue;
    }
    if (c === "\u0E33" && i + 1 < ch.length && TONE.test(ch[i + 1]) && !TONE.test(out[out.length - 1] ?? "")) {
      out.push(ch[i + 1], c); i += 2;
      while (i < ch.length && TONE.test(ch[i])) i++; // drop further tones
      continue;
    }
    out.push(c); i++;
  }
  return out.join("");
}
