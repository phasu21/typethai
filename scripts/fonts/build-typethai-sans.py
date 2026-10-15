"""Build typeThai Sans: Noto Sans Thai with the Latin letters of Noto Sans, one file per weight.

gpui only loads a font that has a Latin 'm', so Noto Sans Thai (Thai-only) cannot be used on its own, and a fallback
font list is resolved by system font name, which a bundled font does not have. Merging the two gives one family that
gpui loads directly on macOS and Windows. Both sources are under the SIL Open Font License 1.1; the result is renamed
because it is a modified version.

usage: python build-typethai-sans.py <NotoSans hinted/ttf dir> <NotoSansThai hinted/ttf dir> <out dir>
Sources: notofonts/latin-greek-cyrillic release NotoSans-v2.015, notofonts/thai release NotoSansThai-v2.002.
"""
import sys
import tempfile
from pathlib import Path

from fontTools import subset
from fontTools.merge import Merger
from fontTools.ttLib import TTFont

FAMILY = "typeThai Sans"
WEIGHTS = ["Regular", "Medium", "SemiBold", "Bold"]
# Basic Latin, Latin-1, Latin Extended-A, general punctuation, currency, arrows (the Settings text uses →), math minus
LATIN = "U+0020-007E,U+00A0-017F,U+2000-206F,U+20A0-20CF,U+2122,U+2190-21FF,U+2212"
# Thai, plus the dotted circle shaping engines draw under a stray mark
THAI = "U+0E00-0E7F,U+25CC"


def subset_to(src: Path, unicodes: str, out: Path) -> None:
    opts = subset.Options()
    opts.layout_features = ["*"]
    opts.name_IDs = ["*"]
    opts.name_languages = ["*"]
    opts.notdef_outline = True
    opts.glyph_names = False
    font = subset.load_font(str(src), opts)
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=subset.parse_unicodes(unicodes))
    sub.subset(font)
    font.save(str(out))


def rename(font: TTFont, weight: str) -> None:
    ribbi = weight in ("Regular", "Bold")
    names = {
        1: FAMILY if ribbi else f"{FAMILY} {weight}",
        2: weight if ribbi else "Regular",
        4: f"{FAMILY} {weight}",
        6: f"typeThaiSans-{weight}",
        16: FAMILY,
        17: weight,
    }
    table = font["name"]
    table.names = [n for n in table.names if n.nameID not in (1, 2, 3, 4, 6, 7, 16, 17, 21, 22)]
    table.setName(f"typeThai Sans {weight}; built from Noto Sans 2.015 and Noto Sans Thai 2.002", 3, 3, 1, 0x409)
    for nid, value in names.items():
        table.setName(value, nid, 3, 1, 0x409)
        table.setName(value, nid, 1, 0, 0)


def widen_vertical_metrics(merged: TTFont, sources: list[TTFont]) -> None:
    """Thai stacks marks above and below the line: keep the tallest ascent and deepest descent of the sources."""
    asc = max(s["hhea"].ascent for s in sources)
    desc = min(s["hhea"].descent for s in sources)
    merged["hhea"].ascent, merged["hhea"].descent = asc, desc
    os2 = merged["OS/2"]
    os2.usWinAscent = max(s["OS/2"].usWinAscent for s in sources)
    os2.usWinDescent = max(s["OS/2"].usWinDescent for s in sources)
    os2.sTypoAscender = max(s["OS/2"].sTypoAscender for s in sources)
    os2.sTypoDescender = min(s["OS/2"].sTypoDescender for s in sources)


def main() -> None:
    latin_dir, thai_dir, out_dir = (Path(a) for a in sys.argv[1:4])
    out_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        for w in WEIGHTS:
            lat, th = Path(tmp, f"lat-{w}.ttf"), Path(tmp, f"th-{w}.ttf")
            subset_to(latin_dir / f"NotoSans-{w}.ttf", LATIN, lat)
            subset_to(thai_dir / f"NotoSansThai-{w}.ttf", THAI, th)
            sources = [TTFont(str(lat)), TTFont(str(th))]
            merged = Merger().merge([str(lat), str(th)])
            widen_vertical_metrics(merged, sources)
            merged["OS/2"].usWeightClass = sources[0]["OS/2"].usWeightClass
            rename(merged, w)
            out = out_dir / f"typeThaiSans-{w}.ttf"
            merged.save(str(out))
            cmap = merged.getBestCmap()
            print(f"{out.name}: {out.stat().st_size} bytes, {len(cmap)} chars, has m {ord('m') in cmap}, has ก {0x0E01 in cmap}")


if __name__ == "__main__":
    main()
