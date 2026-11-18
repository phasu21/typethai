# LATTE-WordSeg-Lite vs typeThai's dictionary segmentation (offline spike)

Date: 2026-11-18
Scope: does [iapp/OpenThai2.0-LATTE-WordSeg-Lite](https://huggingface.co/iapp/OpenThai2.0-LATTE-WordSeg-Lite)
(revision `d5f39bc`, wheel 0.1.2, ONNX int8) improve on `Dict::segment` (`typethai-core/src/dict.rs`), the segmenter
behind `corpus::word_runs` (the pair counts that `tt-bigrams` builds), `predict::words`, `thai_tail` and
`Engine::screen_words`? Spike only: no app code changed. The tools are in `latte-wordseg-spike/`.

## Summary

LATTE segments much better than the dictionary: word F1 is 84–93 against 67–72 on the public benchmark. That only
partly helps typeThai. **Pair counts built from LATTE segmentation** are better when scored against human-segmented
text (next word in the top 3: 13.9% → 18.5%; completion in the top 3: 81.2% → 85.0%). They are slightly worse when
scored against dictionary-segmented text, the units the app ranks today (completion in the top 3: 80.2% → 79.5% for
the union table, and down to 73.6% for LATTE alone). **Running LATTE in the app** adds at most about 0.5 points on
top of that. It costs about 19 MB of ONNX Runtime per architecture (+56% on the 68 MB universal binary), raises the
minimum macOS to 13.4, and takes about 0.7 ms per keystroke against 8 µs now. **Partial words:** LATTE isolates a
3-character tail well (96%), but splits nearly finished words (62% exact). The real `thai_tail` failure is a head
word the dictionary lacks, and a dictionary-only fallback fixes it better than LATTE does (39% → 91%).
**Verdict: adopt at build time only, as an experiment (a union of the dictionary and LATTE-short pair counts)
checked against real picks in the rank log; do not run it in the app.** The `thai_tail` fallback is a separate,
LATTE-free change worth testing.

## Setup

- Model: `model.int8.onnx` (2.98 MB, SHA-256 matches `manifest.json`), `lexicon.txt.gz` (177,385 words), `charclass.npz`.
  The wheel was unpacked into its own empty directory and read before it was installed. It uses only NumPy and
  onnxruntime, with no network access, `exec` or pickle (`np.load` keeps `allow_pickle` at False). It ran in a
  Python 3.14 venv with `python -I`.
- typeThai: the shipped `model-v4` `typethai.ttm` (SHA-256 `0f1fda05…`, which matches `release.yml`). The Thai corpora
  are those of `scripts/bigrams/fetch.sh`, with the same pinned files (7 of 8 checksums match `SHA256SUMS`; the
  weekly Tatoeba export differs). They were split with `extract.py`'s crc32 rule, Thai only.
  **Sanity check:** counting dictionary pairs with `--vocab`, min count 3 (the `3:all` level), reproduces the shipped Thai
  pair table exactly: 466,754 pairs, with the same scores as the `.ttm`.
- LATTE was run with `tokenize_batch` (the default) over all 63M training and held-out characters, at about
  65–110K characters/s on one thread, about 20 min per unit. LATTE tokens are joined with U+200B, which
  `word_runs`' class table already treats as a space. Then the same `word_runs` rules apply: a token the
  dictionary lacks ends the run (`pre`), or is re-split with `dict.segment` (`hyb`).

## 1. Segmentation quality

Public benchmark (OpenThai2.0-WordSeg-Benchmark `08fb0d9`, its own `score.py`). The LATTE numbers reproduce the
published ones.

| Set (gold convention) | dict.segment word F1 / boundary F1 | LATTE short | LATTE long |
|---|---|---|---|
| TUD (short) | 68.5 / 85.4 | **90.3** / 95.9 | 83.7 / 92.9 |
| PUD (long) | 66.7 / 84.2 | 84.7 / 93.6 | **86.2** / 94.2 |
| WS160 (short, Wisesight) | 70.6 / 83.5 | **88.3** / 93.9 | 85.5 / 92.1 |
| VISTEC test (long, tweets) | 72.4 / 85.7 | 87.9 / 94.5 | **92.8** / 96.6 |

`dict.segment` scores at about the level of PyThaiNLP newmm (68–77), as expected for maximal matching.

typeThai's own held-out corpora have no gold segmentation, except VISTEC, which is the row above. Between the
dictionary and LATTE-short, the cuts inside Thai text agree at F1 81–91 (wiki 80.6, reviews 90.5). LATTE's tokens are
more often missing from typeThai's dictionary: 86–98% of Thai characters fall in dictionary words, against 97–99% for
`dict.segment` (wiki 86.4% against 96.7%). These are names and compounds the dictionary does not list, and each one
ends a pair run. This is why the `hyb` and union tables below recover part of the loss.

## 2. Downstream: next word and completion

All tables are pruned like `model-v4` (`--vocab`, min count 3, all followers kept). Ranking is the shipped
`Completer::complete` with the previous word and the pairs, without word memory (`tt-bigram-eval`'s "shipped"
column). Next word: the gold word is the first, or among the first 3, followers of the previous word. Completion:
targets of 3+ characters, completed from their first 2 characters.

Three held-out views, since "the right word" depends on the segmentation:

- **A, dictionary units**: all typeThai held-out text (98K lines, 1.25M pairs), segmented by `dict.segment`, as
  the app reads it today.
- **G, gold units**: the benchmark lines that are not in typeThai's training text (978 VISTEC, 363 TUD, 1,000 PUD
  and 23 WS160 lines; 75K pairs). These are human segmentations, partly in conventions LATTE was trained on, which
  favours LATTE.
- **R, realistic runtime**: in the same gold lines, a gold word of 4+ characters typed after up to 3 unspaced
  words. The previous word is what the app's segmenter (dict) or LATTE makes of the head (29,204 cases). This is
  the closest view to the bubble.

| Pair table (pairs) | A next@1 / @3 | A compl@1 / @3 | G next@1 / @3 | G compl@1 / @3 | R compl@1 / @3 (prev by dict) |
|---|---|---|---|---|---|
| **dict (shipped, 466,754)** | **13.0 / 23.2** | **65.5 / 80.2** | 6.8 / 13.9 | 61.1 / 81.2 | 45.4 / 68.6 |
| LATTE short (343,382) | 7.8 / 15.0 | 58.4 / 73.6 | 9.3 / 18.4 | **69.8** / 84.7 | 51.5 / 72.8 |
| LATTE short + dict for unknown tokens (416,495) | 9.0 / 16.9 | 60.6 / 75.4 | 9.2 / 18.2 | 69.7 / 84.7 | 51.4 / 72.9 |
| LATTE long (338,305) | 8.4 / 15.9 | 58.9 / 74.2 | **9.4** / **18.5** | 68.8 / 84.2 | 51.1 / 72.6 |
| LATTE long + dict for unknown tokens (429,705) | 10.1 / 18.6 | 62.0 / 76.7 | 9.2 / 18.3 | 68.8 / 84.4 | 51.3 / 72.9 |
| **union: dict + LATTE short (550,385)** | 11.8 / 21.7 | 63.5 / 79.5 | 9.2 / **18.5** | 69.7 / **85.0** | **52.7 / 74.2** |
| union: dict + LATTE long (524,488) | 12.0 / 22.0 | 64.0 / 79.7 | 9.0 / 18.3 | 68.4 / 84.2 | 51.8 / 73.4 |

Unigram completion alone: A 65.8, G 76.6 (top 3). In view R, taking the previous word from LATTE (in-app) instead of
the dictionary changes completion top-3 by +0.5 for LATTE-short pairs and by -0.3 for the union table. If the whole
app read LATTE-short units (A re-segmented, a different set of targets), the scores are 15.0 / 27.0 next and
77.1 / 89.6 completion. They are not comparable to A: shorter units are easier to complete.

How to read it: LATTE's pairs know the words people write (G, R: +4 to +7 points), but they lose the dictionary's
compounds that the app's own segmentation produces as context and as targets (A: -0.7 to -7). The union keeps most of
both. Which view matches users is a question for real picks (`--rank-report`), not for this offline eval.

## 3. Partial words (the tail of an unspaced run)

Cases come from gold lines in TUD, WS160 and VISTEC (about 27K sampled): 1–3 unspaced words, then the first 3
characters, or all but the last character, of a 4+ character completion word. "Exact" means the tail chosen
is exactly the partial word.

| Case (share) | `thai_tail` today | LATTE short alone | LATTE long alone | LATTE cuts + vocab, longest tail | `thai_tail`, else dict-only fallback |
|---|---|---|---|---|---|
| 3 chars typed, head in dict (88%) | 87.9 | **96.3** | 95.8 | 87.7 | 87.9 |
| 3 chars typed, head has an unknown word (12%) | 38.8 (56% no tail at all) | **93.5** | 92.1 | 88.5 | 90.7 |
| word minus 1, head in dict (89%) | **91.9** | 62.1 | 69.2 | 91.6 | **91.9** |
| word minus 1, head has an unknown word (11%) | 40.5 (55% no tail) | 61.6 | 67.9 | 91.4 | **93.4** |

- LATTE does still isolate prefix + incomplete tail for short tails. It cuts at the head boundary 97–99.6% of the
  time, even when the head is out of the dictionary. As the partial grows, it reads it as a word plus a fragment
  and cuts inside it 30–38% of the time (สถานกา → สถาน|กา).
- `thai_tail`'s misses are mostly "tail too long" (8–12%): the last head word plus the partial is the prefix of a
  longer word. Many of these may still be acceptable offers.
- Its real gap is a head with a word the dictionary lacks: `head_ok` then fails and no completion is offered at all.
  If no tail passes, falling back to the longest tail that the completer extends, without `head_ok`, takes these
  cases from 39–41% to 91–93% with no model. That is better than using LATTE's cuts (88–91%). This is an engine change
  (it may offer rows on junk or wrong-layout runs, which `head_ok` was presumably guarding against), so it needs the
  engine tests and the rank log before it ships.

## 4. Cost if embedded in the app

| | Today (`dict.segment` + `thai_tail`) | LATTE via `ort` 2.0.0-rc.15 (static, measured) |
|---|---|---|
| Latency, 13-char run (keystroke) | p50 8 µs, p99 26 µs | p50 0.65–0.74 ms, p99 1.1–1.4 ms (Apple M5, 1 thread) |
| Latency, 60-char run | p50 80 µs | p50 1.35 ms, p99 1.9 ms |
| Binary | 68 MB universal app | +19.3 MB per arch (stripped test binary, includes the CoreML EP): about +38 MB universal |
| Assets | none | model 2.98 MB, lexicon 0.84 MB gz (4.3 MB raw), char table 1.1 MB raw (4 KB as npz) |
| Memory | | about 78 MB RSS in the test binary (ORT plus a 177K-word `HashSet`), load 0.2 s |
| Platform | macOS 12.0 target | the prebuilt ORT objects target macOS 13.4. The link also needed `libclang_rt.osx.a` (`___isPlatformVersionAtLeast`) |

The Rust port (`latte-wordseg-spike/latte-rs.rs`, about 70 lines of logic) matches the Python package on 2,749 of
2,750 runs (short) and 2,747 (long), the int8 rounding noise the model card describes. Decoding logic a port needs:

1. **charclass**: code point to id. Use the 944-character vocabulary from the ONNX metadata (`13 + index`), else
   the Unicode-class bucket table (`charclass.npz`, 0–12).
2. **lexicon features**: for every dictionary word of 2–16 characters found at each position, 13 bits (6 length buckets ×
   starts/ends, plus "inside a word"). The model looks them up in an 8192 × 32 table, so the lexicon is model input,
   not post-processing. typeThai's dictionary cannot replace it without retraining.
3. **decode**: no lattice search in `tokenize`. A word starts where the unit's logit is ≥ 0 (nested units: the maximum
   over coarser units), except before a combining mark or following vowel, and after a leading vowel (เ–ไ). Whitespace
   always cuts, and ASCII-letter and digit runs are never split. The lattice is only in `tokenize_nbest` (dictionary
   spans plus each unit's words, n-best by summed log-probabilities), which typeThai would not need.
4. **network**, if ONNX Runtime is to be avoided: char embedding 957×64 plus lex embedding, then 3 Conv1d (widths
   3/5/7, 128 channels), 2 bidirectional LSTM layers (hidden 128, `DynamicQuantizeLSTM`, an ORT contrib op, so
   tract and other runtimes cannot load the graph as is), then a 256×3 int8 linear layer. That is 1.17M parameters. A
   hand-written Rust forward pass is about 300 lines, adds almost nothing to the binary, and should be well under 1 ms
   for a keystroke. It would need its own parity tests.

## 5. Licence

- Model, lexicon and char table: CC BY-SA 4.0. The ICU-derived lexicon entries also carry the Unicode-DFS-2016 notice.
  The runtime code is Apache-2.0.
- **Build-time only**: LATTE runs offline on the corpora, and only word pair counts ship. Neither the model nor the
  lexicon is distributed, and the counts are facts about our corpora, not an adaptation of the model. Even on the
  strictest reading, `typethai.ttm` is already CC BY-SA 4.0, so ShareAlike is satisfied. Credit it as a build tool
  in `DATA_LICENSES.md` (name, iApp Technology and the LATTE authors, licence, link). Its training data (VISTEC,
  UD-TUD, Wisesight, khammoon CC BY-SA rows, Wikipedia) adds no obligation on the counts.
- **Bundled in the app**: ship the model and lexicon files with attribution (title, copyright holders, the NOTICE),
  the CC BY-SA 4.0 licence or its URI, the Unicode notice, and an indication of changes. Any converted or merged form,
  such as the lexicon packed into the TTM or the char table as `.bin`, is Adapted Material and must stay CC BY-SA 4.0
  (the TTM already is). The files must not be encrypted or otherwise locked (no effective technological measures). The
  app's MIT code is a separate work and is not affected. Also add ONNX Runtime's MIT licence and its
  ThirdPartyNotices, and keep Apache-2.0 attribution in any source ported from their runtime.

## Verdict

- **Adopt at build time only, as an experiment.** Build the Thai pairs as the union of the dictionary and LATTE-short
  counts. It is near-neutral on dictionary units (completion top 3 -0.7) and best on gold and realistic units
  (+3.8 and +5.6), with 18% more pairs (550K, about +1 MB in the TTM). Ship it only if the rank log's real picks
  agree. `scripts/bigrams/build.sh` would need a pre-segmentation step (LATTE writing U+200B between tokens, as here).
  The Python venv is a build dependency, not an app dependency.
- **Do not run it in the app.** It adds +38 MB, a macOS 13.4 floor and 90× the segmentation time for at most +0.5
  points downstream. The partial-word gap it would close is closed better by a dictionary-only `thai_tail` fallback.
  If in-app segmentation ever matters (for example `screen_words` for learning), a pure-Rust forward pass (section 4,
  item 4) avoids ONNX Runtime.

Not done: English is unaffected, as LATTE is Thai only. Word memory and the personal model are not in the eval. The
benchmark's BEST, ORCHID and khammoon sets (non-commercial, or built from sources) were skipped. Latency was measured on
an M5 only.
