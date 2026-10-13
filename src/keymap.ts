// Full EN (US QWERTY) <-> Thai Kedmanee (TIS 820-2538) keymap, unshifted + shifted.
export const EN_UNSHIFTED = "`1234567890-=qwertyuiop[]\\asdfghjkl;'zxcvbnm,./";
export const TH_UNSHIFTED = "_ๅ/-ภถุึคตจขชๆไำพะัีรนยบลฃฟหกดเ้่าสวงผปแอิืทมใฝ";
export const EN_SHIFTED = "~!@#$%^&*()_+QWERTYUIOP{}|ASDFGHJKL:\"ZXCVBNM<>?";
export const TH_SHIFTED = "%+๑๒๓๔ู฿๕๖๗๘๙๐\"ฎฑธํ๊ณฯญฐ,ฅฤฆฏโฌ็๋ษศซ.()ฉฮฺ์?ฒฬฦ";

if ([...EN_UNSHIFTED].length !== [...TH_UNSHIFTED].length || [...EN_SHIFTED].length !== [...TH_SHIFTED].length) {
  throw new Error("keymap length mismatch");
}

export const EN_TO_TH: ReadonlyMap<string, string> = (() => {
  const m = new Map<string, string>();
  const e = [...EN_UNSHIFTED, ...EN_SHIFTED], t = [...TH_UNSHIFTED, ...TH_SHIFTED];
  e.forEach((c, i) => m.set(c, t[i]));
  return m;
})();

/** Thai char -> EN key. Only Thai-script outputs are inverted (ASCII outputs like "/" are ambiguous). */
export const TH_TO_EN: ReadonlyMap<string, string> = (() => {
  const m = new Map<string, string>();
  for (const [e, t] of EN_TO_TH) if (/[\u0E00-\u0E7F]/.test(t)) m.set(t, e);
  return m;
})();

export function enToTh(s: string): string {
  let o = "";
  for (const c of s) o += EN_TO_TH.get(c) ?? c;
  return o;
}

/** Returns null if some char cannot be typed on Kedmanee. */
export function thToEn(s: string): string | null {
  let o = "";
  for (const c of s) {
    const e = TH_TO_EN.get(c);
    if (e !== undefined) o += e;
    else if (c === " ") o += c;
    else return null;
  }
  return o;
}
