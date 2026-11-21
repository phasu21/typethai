# Thai typo and spelling fixes: which small on-device model? (research)

Date: 2026-11-21
Scope: research only. Which approach should typeThai use to fix Thai typing mistakes at the caret: first single
words, later whole sentences? No app code was changed. One small analysis was run on a public dataset (section 1);
its scripts are described there. Earlier work this builds on: `latte-wordseg-spike.md` (the cost of ONNX Runtime),
`confusion-pairs-spike.md` (branch `spike/confusion-pairs`, the English staged plan), `llm-rewrite-selection.md`,
`llm-sentence-completion*.md` and `embedded-llm-next-word-prediction.md`.

Labels used below: **verified** means read on the primary page (model card, paper, repo) during this research.
**Measured** means counted here from the data. **Inferred** means reasoned or estimated, not read or run.

## Summary

- **What typeThai already has.** A Thai non-word speller (`typethai-core/src/corrector.rs`): a SymSpell-style delete
  index over the 62K PyThaiNLP words, a Kedmanee-aware weighted Damerau-Levenshtein distance, word frequency and the
  character trigram model. On synthetic slips in held-out reviews it puts the right word first 81.7% of the time
  (README). It only fires when the typed word is **not** in the dictionary.
- **The biggest gap is real-word errors.** In 163,600 human-annotated misspellings in Thai tweets
  (VISTEC-TP-TH-2021), **52% of the typed forms are themselves dictionary words** (`นี่`/`นี้`, `ล่าน`/`ล้าน`,
  `คะ`/`ค่ะ`, `ของ`/`ขอ`). A dictionary check cannot see them; they need context.
- **Most of the annotated "misspellings" are not typos.** About 69% are on-purpose informal writing (spacing of ๆ,
  repeated letters, shortenings, slang respellings such as `เค้า` for `เขา`). typeThai must **not** "fix" these. Of the
  31% that are one edit away, tone marks, vowel marks and consonants dominate, and about a fifth of them are a Kedmanee
  neighbour or Shift slip.
- **Recommendation.** Stage 1 (cheap, now): extend the speller with a **Thai confusion list** (about 1,000 attested
  pairs from Thai Wiktionary, plus pairs mined from VISTEC and Kedmanee one-key slips that make a real word) and
  score them with the Thai word pair counts the model already ships, exactly like the English confusion spike. Stage 2:
  a **small character-level GRU** (about 1–3 MB, pure Rust, no ONNX Runtime) trained on Thai text with injected
  Kedmanee and spelling errors, as a better context scorer for the same candidates. Stage 3: **sentence rewrite on
  request only**, through the existing ⌃⌥R path with a Thai-tuned 4B model and a diff preview.
- **No ready-made small Thai spell model is worth adopting.** The Thai spelling models on Hugging Face are either
  experiments with no licence or metrics (a ByT5 "not good", 1.2 GB), an 815 MB embedding lookup, or 4B LLM fine-tunes
  with no licence or results. The best published Thai benchmark (ThaiEditBench, 2026) tests only 8B+ LLMs, and the
  Thai-tuned 8B–30B models there over-edit badly on long text.

## 1. Which Thai typing errors matter most (with data)

### 1.1 Real social-media misspellings: VISTEC-TP-TH-2021 (measured here)

[VISTEC-TP-TH-2021](https://github.com/mrpeerat/OSKut/tree/main/VISTEC-TP-TH-2021) (verified): 49,997 tweets
(2017–2019), 3.39M words, annotated by Chiang Mai University linguists. Misspellings are tagged
`<msp value="intended">typed</msp>`. Licence CC BY-SA 3.0 (the folder's `LICENSE.md`). typeThai already uses its raw
tweets for word pair counts (`DATA_LICENSES.md`).

Method: both preprocessed files (train 39.4 MB, test 9.9 MB) were downloaded, and every `<msp>` pair was extracted
(163,600 tokens, 24,603 unique typed→intended pairs). Each pair was classified with an optimal-string-alignment edit
distance. One-edit pairs were labelled by the character class involved, and as a **Kedmanee slip** when the two
characters are neighbouring keys, the two levels of one key (Shift), a doubled key or a swap of two adjacent letters.
"Dictionary word" means the typed form is in PyThaiNLP `words_th.txt` (62,107 words, the list the model ships). The
scripts were throwaway Python in a scratch directory; nothing was added to the repo.

| Bucket | Tokens | Share | Unique pairs | Typed form is a dictionary word | Kedmanee slip |
|---|---|---|---|---|---|
| Informal on purpose: ๆ spacing, repeated letters, shortenings, slang (3+ edits) | 78,328 | 47.9% | 10,085 | 45% | 0% |
| Two edits (mostly informal respellings: `เค้า→เขา`, `นึง→หนึ่ง`, `เมื่อไหร่→เมื่อไร`) | 34,583 | 21.1% | 4,084 | 72% | 0% |
| **One edit: consonant** (wrong, extra or missing) | 18,455 | 11.3% | 4,972 | 45% | 16% |
| **One edit: tone mark** ่ ้ ๊ ๋ (wrong, extra, missing, or tone↔็ on one key) | 16,250 | 9.9% | 2,582 | 56% | 30% |
| **One edit: vowel mark** (above/below, leading เ แ โ, trailing ะ า ำ) | 11,955 | 7.3% | 1,722 | 55% | 22% |
| One edit: punctuation, digits, ฯ | 2,987 | 1.8% | 869 | 41% | 0% |
| One edit: letter order (`โครต→โคตร`, `แสกน→สแกน`) | 535 | 0.3% | 120 | 9% | 100% |
| One edit: ไ/ใ (`ไกล้→ใกล้`, `หลงไหล→หลงใหล`) | 311 | 0.2% | 60 | 20% | 0% |
| One edit: karan ์ | 196 | 0.1% | 109 | 31% | 7% |

All one-edit pairs together: 50,689 tokens (31.0%). Of these, 25,480 (50%) are real words and 11,162 (22%) are
Kedmanee slips. Similar-sound consonant swaps (ศ/ษ/ส, ท/ธ/ฑ, ค/ฅ/ฆ, น/ณ, ซ/ส and others) are only 1,615 tokens (1%):
`คุน→คุณ`, `ซัก→สัก`, `ออฟฟิส→ออฟฟิศ`, `เทอ→เธอ`.

Typical one-edit pairs, most frequent first:
- Tone: `อ่ะ→อะ`, `ทันที่→ทันที`, `แต่ล่ะ→แต่ละ` (extra); `ล่าน→ล้าน`, `นี่↔นี้`, `นั้ง→นั่ง`, `แม่→แม้` (่/้ are
  neighbouring keys); `ก้→ก็`, `เต๊นท์→เต็นท์` (้ and ็ share a key); `ชัน→ชั้น`, `คะ→ค่ะ`, `ที→ที่` (missing).
- Vowel: `กุ→กู`, `ผุ้ชาย→ผู้ชาย` (ุ/ู Shift pair); `เปน→เป็น`, `เหน→เห็น` (missing ็); `ถืง→ถึง`; `ป็น→เป็น`
  (missing leading vowel); `นะง→นาง`, `ค่า→ค่ะ`.
- Consonant: `ตอน→นอน`, `ฟมด→หมด` (neighbours); `โปรโมท→โปรโมต`, `ซีรีย์→ซีรีส์` (loanword endings); `จิงจัง→จริงจัง`
  (missing ร in a cluster).

Caveats: the annotators tagged every non-standard form, not only mistakes, so this is an upper bound on what looks
wrong, and the intentional share is large. Tweets are one register. Frequencies are dominated by a few very common
words (`กุ`, `มากๆ`). The classifier is heuristic (one alignment per pair). It is still the largest real Thai
error sample available, and it was typed on real keyboards (mostly phones, so Kedmanee desktop slips are likely
**under**-represented).

### 1.2 Other evidence (verified)

- **Nakwijit & Purver, "Misspelling Semantics in Thai", LREC 2022** ([arXiv 2206.09680](https://arxiv.org/abs/2206.09680)):
  3,000 Wisesight sentences, 1,484 misspelt tokens, 728 unique types; **572 intentional vs 156 unintentional** types.
  In their Table 2 (one annotator, Wisesight test) the patterns are vowel substitutions 428, tone modification 402,
  others 371, character repetition 300, simplifying 217, **tone confusion 211**, **typos 106**, consonant deviation 51,
  **consonant confusion 15**. The unintentional ones are tone confusion, consonant confusion and typos. Same picture
  as above: chat Thai is mostly deliberate, and real mistakes cluster on tone marks.
- **ThaiEditBench, AACL 2026** ([paper](https://aclanthology.org/2026.aacl-short.17/),
  [repo](https://github.com/yongsa-nut/ThaiEditBench)): a benchmark for formal Thai **spelling** (knowledge) errors,
  not finger slips. Its inventory of 1,044 attested wrong→correct pairs, from the Thai Wiktionary misspelling appendix
  and a Wikipedia cleanup bot, splits by class as: tone 43, consonant pair (ศ/ษ/ส, ท/ธ/ฑ, ฎ/ฏ, น/ณ) 168, karan ์ 177,
  vowel form 189, cluster/letter order (incl. รร/ัน) 338, loanword transliteration 129. So in formal writing the
  common errors are karan (`ขาดดุลย์→ขาดดุล`), cluster order (`ศรีษะ→ศีรษะ`, `บรรได→บันได`), vowel length
  (`คลีนิก→คลินิก`), Indic consonants (`กฏ→กฎ`) and loanword endings (`นิวยอร์ค→นิวยอร์ก`, `ซอฟท์แวร์→ซอฟต์แวร์`).
  Loanwords are the class models miss most (missed 0.186 of gold edits, pooled).

### 1.3 What this means for typeThai

| Error type | How common | Already handled? | What it needs |
|---|---|---|---|
| Tone mark slips and confusions (่/้, ้/็, missing ่) | High: ~10% of all tags, half of them real words | Non-word only | Context: real-word candidates + n-gram or model |
| Vowel marks (ุ/ู, missing ็, ั/ะ/า) | ~7%, half real words | Non-word; `normalize_thai` fixes mark order (เเ→แ, ํา→ำ, tone after vowel) | Same as tone |
| Consonant neighbours and clusters | ~11% | Non-word, Kedmanee-weighted | Same |
| ไ/ใ | Small (0.2%) but well known; few real words | Non-word, mostly | A fixed list of the 20 ใ words is enough |
| Similar-sound consonants (ศ ษ ส, ท ธ ฑ, ข ฃ, ค ฅ) | ~1% in chat; big in formal text | Non-word, but costs are flat (no "sounds alike" discount) | Lower substitution cost within a sound group |
| Karan ์ and loanword spellings | Rare in chat, common in formal text | Partly (non-word) | Attested-pair list (Wiktionary, Royal Institute rules) |
| Letter order (transposition) | 0.3% | Yes (transposition cost) | Nothing new |
| Informal on purpose (มากๆ, มากกกก, เค้า, ป่ะ) | ~69% | Must stay **unflagged** | A "leave alone" list and a false-flag test on chat text |

## 2. Datasets and licences

| Dataset | What it gives | Size | Licence | Commercial use | Status |
|---|---|---|---|---|---|
| [VISTEC-TP-TH-2021](https://github.com/mrpeerat/OSKut/tree/main/VISTEC-TP-TH-2021) | Real typed→intended pairs in tweets (`<msp>`) | 50K tweets; 163,600 tags (measured) | CC BY-SA 3.0 | Yes, with attribution and share-alike | Verified |
| [pythainlp/thai-misspell-correction](https://huggingface.co/datasets/pythainlp/thai-misspell-correction) | Sentence pairs (`inputs`, `targets`) derived from VISTEC-TP-TH-21 | 40K train / 10K test | CC BY-SA 3.0 | Yes (as above) | Verified. Targets sometimes reword (`ดูคอนฯ→ดูคอนเสิร์ต`). Contains profanity and names |
| [Thai Wiktionary: รายชื่อคำในภาษาไทยที่มักเขียนผิด](https://th.wiktionary.org/wiki/ภาคผนวก:รายชื่อคำในภาษาไทยที่มักเขียนผิด) | Correct word → common misspellings (`กงศุล→กงสุล`, `เกมส์→เกม`) | Roughly 1,000 entries (inferred; page not fully read) | Wiktionary text is CC BY-SA (version on the page not confirmed) | Yes, with attribution and share-alike | Page verified, count and licence version not |
| [ThaiEditBench](https://github.com/yongsa-nut/ThaiEditBench) | Formal-text test sets with exact gold, plus 86 real Wikipedia errors | T1 774 sentences, T2 224, T3 122, natural 86; 1,044 attested pairs | Data CC BY-SA 4.0, code MIT (repo); the paper says "for research use" | Licence allows it; use as **evaluation only** to keep it clean | Verified (repo + paper) |
| Nakwijit & Purver 2022 misspelling annotations | 3,000 Wisesight sentences with intent labels | 1,484 tokens | Not found in the paper | Unknown | Release location not found |
| Lertpiya et al. 2020/2023 data ([IEEE Access 2020](https://doi.org/10.1109/ACCESS.2020.3010828), [2023](https://doi.org/10.1109/ACCESS.2023.3295004)) | Social-text spelling correction sets | Not read | Papers CC BY 4.0; data availability unknown | Unknown | Abstracts only (full text blocked) |
| Thai Wikipedia, Wisesight, Tatoeba Thai, scb-mt-en-th-2020 (already used) | Clean text for synthetic error injection and false-flag tests | Already in the build | CC BY-SA 4.0 / CC0 / CC BY 2.0 FR | Yes | In `DATA_LICENSES.md` |

The shipped model is already CC BY-SA 4.0 (it derives from Wikipedia), so adding pairs mined from VISTEC and
Wiktionary does not change its licence; it adds two attribution lines. The Thai Toxicity Tweet Corpus stays
eval-only (CC BY-NC). ThaiEditBench should stay a test set; training on it would make its numbers meaningless.

Synthetic errors: the clean text above, corrupted with (a) Kedmanee slips from `keymap.rs` (neighbour, Shift level,
doubled key, dropped mark, swapped letters) at rates fitted to the one-edit shares in section 1.1, and (b) attested
spelling pairs from Wiktionary and VISTEC. This is the tinyfix recipe, with Thai keys.

## 3. Candidate approaches

### a) Dictionary + edit distance + n-gram ranking (what typeThai does, extended)

- **Today:** non-word errors only. SymSpell-style deletes (distance 2), Kedmanee costs, frequency, character
  trigrams. 81.7% top-1 / 89.0% top-3 on synthetic slips (README). Runs in microseconds; already shipped.
- **Extension (real-word):** a Thai confusion list, scored with the shipped Thai word pair counts (TTM section 12).
  Same scorer as `typethai_core::confusion` on `spike/confusion-pairs`: `ln P(w | prev) + ln P(next | w)` with a
  margin and an evidence guard. Candidate sources: Wiktionary pairs, frequent one-edit pairs from VISTEC train (only
  those that are not intentional: tone, vowel and consonant slips, not shortenings), and Kedmanee one-key variants of
  common words that are themselves words (`นี่/นี้`, `แม่/แม้`, `ค่ะ/คะ`).
- **Thai catch:** no spaces. Swapping a word can change how the run splits (`ทันที่` segments as `ทัน|ที่`, both
  words). The check must compare **segmentations**: score the run with the typed form and with the candidate, both
  through `Dict::segment`, and compare total pair log-probability. Slightly more work than English, still µs.
- **Similar-sound consonants:** add a cheaper substitution cost inside sound groups (ศ ษ ส; ท ธ ฑ ฒ; ค ฅ ฆ; ข ฃ;
  น ณ; ด ฎ; ต ฏ; ถ ฐ; ล ฬ; ย ญ; พ ภ) in `corrector.rs`. One table, no new data.
- Related tools (inferred unless stated): PyThaiNLP's `spell`/`correct` engines `pn` (Norvig, default), `phunspell`
  (Hunspell port), `symspellpy`, and `wanchanberta_thai_grammarly` (verified on the
  [PyThaiNLP docs](https://pythainlp.org/dev-docs/api/spell.html); the last has no model id on the page). They are
  Python and word-level; typeThai's Rust version already covers what `pn` and `symspellpy` do.
- **Size:** the list is a few tens of KB. **Latency:** 5–10 µs per checked word in the English spike; Thai adds a
  second segmentation of the run (inferred: tens of µs). **No new dependency.**

### b) Small character-level GRU/LSTM scorer (tinyfix-style, trained for Thai)

- **Why character-level:** Thai has no spaces and a 20K-word vocabulary misses many words; a character model also sees
  the mark order inside a syllable. It would score "how natural is this text" for each candidate (a bidirectional
  character language model, or a classifier over candidates like tinyfix).
- **tinyfix** ([paperweight/tinyfix](https://huggingface.co/paperweight/tinyfix), MIT, verified in the English spike):
  1.9M parameters, 2 GRUs (hidden 256) over 16 words left and 4 right, 2.1 MB at int8, about 3 ms in browser WASM,
  about 0.5 false changes per 1,000 words, 1 h 45 min of training on one A10G. **English only.** A Thai version means
  training our own.
- **Thai sketch (inferred):** about 90 Thai characters + 30 symbols as input; embeddings 64; one GRU layer each way,
  hidden 256–384; about 0.6–1.5M parameters, **1–3 MB at int8**. Train as a character language model on the clean
  training text in `DATA_LICENSES.md` (tens of MB), then score candidates from stage 1 by the change in log-probability
  over the run and a few neighbouring words. Training: a few GPU hours, or overnight on Apple Silicon with MLX or
  PyTorch MPS (one job at a time).
- **Runtime:** a hand-written forward pass in Rust, as the LATTE spike's section 4 sketches (about 300 lines, nothing
  added to the binary, macOS 12 floor kept). Weights stored in the TTM. **Cost per check (inferred):** one GRU step at
  hidden 384 is 3·(64+384)·384 ≈ 0.5M multiply-adds, so re-reading a whole 40-character window both ways is about 40M
  (roughly 5–20 ms scalar). Carrying the left state across keystrokes and re-reading only a short right window
  (about 10 characters) per candidate cuts that to a few million, **well under 1 ms**, and hidden 256 halves it again.
  It runs once per finished word, only when stage 1 has candidates.
- **Licence:** our own weights, trained on the existing sources, so CC BY-SA 4.0 like the model.
- **Risk:** unknown quality until trained; it must beat stage 1 at the same false-flag rate or it is not worth it.

### c) Small seq2seq (ByT5-small, mT5-small)

- [google/byt5-small](https://huggingface.co/google/byt5-small) and [google/mt5-small](https://huggingface.co/google/mt5-small):
  Apache-2.0, about 300M parameters, **1.2 GB** in fp32 (verified file sizes). ByT5 works on UTF-8 bytes, and each
  Thai character is 3 bytes, so a 30-character sentence is about 90 decoder steps. mT5-small is mostly a 250K-token
  embedding table.
- Thai fine-tunes on Hugging Face (verified via the HF API): `thanaphatt1/byt5-thai-misspelling-correction-experiment`
  (300M, 1.2 GB, no licence, README says "not good"), `kennyxaxa/fix_thai-eng_typo` (ByT5-small base, a 1.1 GB zip, no
  licence or card). Neither is usable.
- Published Thai seq2seq: Lertpiya et al. 2020 (IEEE Access, two stages: a neural detector, then a seq2seq corrector
  with contextual attention) report on social text WER 2.51% → 2.07% and Thai GLEU 0.9409 → 0.9502 against a
  copy-augmented transformer (from the abstract, verified; the full text was not reachable). Their 2023 follow-up
  (XNCC) separates the dictionary from the neural model and needs only 2% hand-annotated data (abstract).
- **Fit (inferred):** even at int8, ByT5-small is about 300 MB and needs ONNX Runtime or a full transformer in Rust;
  decoding a sentence on CPU is likely hundreds of ms to over a second. It rewrites, so it needs the same over-edit
  guards as an LLM. It is too big for a menu-bar helper and not better targeted than (a)+(b). **Not recommended.**

### d) Small LLMs with real Thai ability (on-request sentence rewrite)

| Model | Size | Licence | Thai evidence | Fit |
|---|---|---|---|---|
| [Typhoon2.5-Qwen3-4B](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b) (+ [GGUF](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf)) | 4B; Q4_K_M 2.5 GB | Apache-2.0, plus the OpenTyphoon terms the card links | Thai-first; already measured in-app: 1.3 s p50 warm (`llm-sentence-completion-app-contract.md`) | **Best small choice for stage 3** |
| [Typhoon2.1-Gemma3-4B](https://huggingface.co/typhoon-ai/typhoon2.1-gemma3-4b) | 4B | Gemma terms (inferred from the Gemma 3 base) | Thai-tuned; Gemma tokenizer uses ~40% fewer tokens for Thai (`llm-rewrite-selection.md`) | Alternative |
| [Gemma 4 E2B / E4B](https://huggingface.co/google/gemma-4-E4B-it) | 2.3B / 4.5B effective (5.1B / 8B with embeddings) | Apache-2.0 (card) | No Thai-specific score on the card | Worth one test; `gemma4` is already the local Ollama default |
| [Typhoon2-Llama3.2-1B](https://huggingface.co/typhoon-ai/llama3.2-typhoon2-1b-instruct) | 1B | Llama 3.2 Community License | IFEval-TH 52.5%, MT-Bench TH 3.97 (card) | Too weak at following "change nothing else" |
| [Qwen2.5-0.5B / 1.5B-Instruct](https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct) | 0.5B / 1.5B | Apache-2.0 | Thai named among 29 languages; IFEval-TH 44.4% for 1.5B (Typhoon card) | Too weak |
| [Qwen3-0.6B / 1.7B](https://huggingface.co/Qwen/Qwen3-1.7B) | 0.6B / 1.7B | Apache-2.0 | 119 languages; Thai reasoning below 4B weak (Qwen3 report, see `llm-rewrite-selection.md`) | Too weak |
| [Gemma 3 1B](https://huggingface.co/google/gemma-3-1b-it) | 1B | Gemma terms (gated) | 140+ languages, no Thai score | Too weak |
| [SeaLLMs-v3-1.5B-Chat](https://huggingface.co/SeaLLMs/SeaLLMs-v3-1.5B-Chat) | 1.5B | Custom "seallms" terms | M3Exam Thai 42.4 (card) | Licence unclear, weak |
| OpenThaiGPT ([1.5-7B](https://huggingface.co/openthaigpt/openthaigpt1.5-7b-instruct), ThaiLLM 8B) | 7–8B | `other` / Apache-2.0 tags | ThaiEditBench: over-edits (below) | Too big and over-edits |
| SEA-LION v5 E2B/E4B ([aisingapore](https://huggingface.co/aisingapore)) | 2–4B | MIT tag; base-model terms may apply | Not checked | Possible later test |
| `Mularstyle/typhoon2.5-qwen3-4b-thaispell-*` | 4B; Q4_K_M 2.5 GB | None stated (most) | No card or metrics | Not usable as-is |

**What ThaiEditBench says about LLM spelling fixes** (verified, Table 1, correction F1; T3 = page-length text with
1–3 errors): GPT-5.5 high 0.981 (T1) / 0.855 (T3); Gemma-4-31B 0.961 / 0.898; Typhoon-S 8B 0.792 / 0.617; THaLLE 8B
0.760 / 0.507; Typhoon 2.5 30B 0.754 / 0.476; OpenThaiGPT v7.2 0.753 / 0.463; Qwen3.6-35B-A3B 0.709 / 0.310.
Spurious edits per 100 items grow with length: Typhoon 2.5 30.9 (T1) → 191.8 (T3), OpenThaiGPT 19.1 → 196.7
(repo `ERROR-ANALYSIS.md`). Two lessons: models rarely pick the wrong spelling once they find an error, so **finding**
the error is the hard part, and **small and Thai-tuned models over-edit**. A 4B model will be worse than these. So an
LLM must stay an explicit request, on a selected sentence (short input), with a word-level diff the user accepts.

### e) Existing Thai spell-check models and papers (2020–2026)

| Work | What | Result | Usable? |
|---|---|---|---|
| ThaiEditBench (Yongsatianchot et al., AACL 2026), [repo](https://github.com/yongsa-nut/ThaiEditBench) | Benchmark, 16 LLMs | Above | **Yes, as a test set** (T1, natural set) |
| Lertpiya, Chalothorn, Chuangsuwanich 2020, [IEEE Access](https://doi.org/10.1109/ACCESS.2020.3010828) | Detector + seq2seq corrector on social text | WER 2.07%, GLEU 0.9502 (abstract) | Method only; no released model found |
| Lertpiya, Chalothorn, Buabthong 2023, [IEEE Access](https://doi.org/10.1109/ACCESS.2023.3295004) | XNCC, dictionary-decoupled neural corrector, progressive annotation | No numbers in abstract | Method only |
| Nakwijit & Purver 2022, [arXiv](https://arxiv.org/abs/2206.09680) | Intent taxonomy of Thai misspellings | Section 1.2 | Taxonomy; shows what **not** to fix |
| [pythainlp/word-spelling-correction-char2vec](https://huggingface.co/pythainlp/word-spelling-correction-char2vec) | Char2Vec nearest-neighbour word lookup (ONNX) over the Royal Institute word list | No metrics; 815 MB (800 MB embeddings) | No: too big, no context, no numbers |
| `bookpanda/wangchanberta-base-att-spm-uncased-tagging` | WangchanBERTa token tagger, likely behind PyThaiNLP's `wanchanberta_thai_grammarly` (inferred) | "Unknown dataset", no metrics, no licence | No |

No 2024–2026 paper with a small (under 10M parameter) Thai typo model and published numbers was found.

## 4. Comparison

| Approach | Fixes | Size added | Latency | Rust, no ORT | Licence | Training cost |
|---|---|---|---|---|---|---|
| a) speller + Thai confusion list + pair counts | Non-word slips (today) + listed real-word mix-ups | ~50 KB list | ~10–50 µs per word (inferred from English 5–10 µs) | Yes, already | CC BY-SA (data) | None; a mining script |
| b) char GRU scorer | Same candidates, better context, more real-word slips | 1–3 MB | under 1 ms per word with carried state (inferred) | Yes, ~300-line forward pass | Own weights, CC BY-SA | Hours of GPU, or overnight on a Mac |
| c) ByT5/mT5-small seq2seq | Any local rewrite | ~300 MB int8, 1.2 GB fp32 | ~0.3–2 s per sentence on CPU (inferred) | No (needs ORT or a transformer port) | Apache-2.0 base | Fine-tune on GPU; no good Thai checkpoint exists |
| d) Thai 4B LLM on request | Anything, incl. grammar and missing words | 2.5 GB download, opt-in | 1.3 s p50 warm (measured earlier) | Ollama now; `llama-cpp-2` later | Apache-2.0 (Typhoon2.5) | None |

## 5. Recommended staged plan

1. **Stage 1, now (cheap).** Keep the non-word speller. Add (i) sound-group substitution costs, (ii) a Thai confusion
   list built at model-build time from Wiktionary, VISTEC train one-edit slips and Kedmanee one-key real-word variants,
   with an explicit **leave-alone** list for intentional forms (ๆ spacing, repeated letters, common slang and particles
   such as `ค่ะ/คะ` only with strong evidence), (iii) the confusion scorer from the English spike, run over Thai
   segmentations of the run, in the "one word late" mode, behind the existing typo switch at a strict margin.
2. **Stage 2, if stage 1 sticks.** Train the Thai character GRU (section 3b) and use it to re-score stage 1's candidates
   (and Kedmanee edit-1 real-word candidates that the list does not cover). Ship it only if it beats stage 1 at an equal
   false-flag rate and stays under 3 MB and 1 ms.
3. **Stage 3, sentence fix on request.** ⌃⌥R *Fix typos* with Typhoon2.5-Qwen3-4B (or Gemma 4 E4B if it tests better),
   on the selected sentence only, with a prompt that forbids rewording and a word-level diff (on Thai segments) the
   user accepts. Use stages 1–2 to highlight which sentence looks worth sending. Never automatic.

## 6. What a later spike should measure

Test sets (none used for training or tuning):
- **Real slips:** VISTEC-TP-TH-2021 **test** split, one-edit pairs that are not intentional (tone, vowel, consonant,
  ไ/ใ, karan, order), with their tweet as context. Mine stage 1 pairs from the train split only.
- **Formal spelling:** ThaiEditBench T1 (774) and the natural Wikipedia set (86).
- **Synthetic:** held-out Wikipedia/Wisesight/Tatoeba Thai lines with one injected Kedmanee slip or attested pair
  each (the existing `bench-typo.ts` style, plus real-word cases).
- **Clean text for false flags:** the held-out Thai splits the model build already keeps (wiki test, Wisesight test,
  Tatoeba Thai held-out, Wongnai fresh), plus VISTEC test tweets with the `<msp>` spans removed or skipped, so chat
  informalities count as false flags if flagged.

Metrics:
- **False flags per 1,000 Thai words** (words counted by `Dict::segment`) on clean text. **Target ≤ 0.25 / 1K**, the
  same as the English spike, reported per source; chat text separately. Also count flags on intentional forms
  (VISTEC tags in the informal buckets): target near 0.
- **Recall** (top-1 and top-3) on real slips and on injected errors, overall and per bucket (tone, vowel, consonant,
  sound group, ไ/ใ, karan, loanword), and **macro recall** averaged over pairs.
- **Precision of offered rows** (target ≥ 99%) and the "all" precision that includes false flags elsewhere on the line.
- **Latency** per checked word (µs, release build), memory and asset size.
- **Stage 3:** ThaiEditBench correction F1 on T1 and T3, T3 clean-sentence fidelity (target ≥ 0.95), spurious edits
  per 100 items, and latency p50/p95 through the app contract.

## 7. Unknowns

- How common Kedmanee **desktop** slips are. VISTEC tweets are mostly phone typing, and no study of Kedmanee
  error rates was found. typeThai's own rank log (accepted and dismissed typo rows) is the best future source.
- Whether Thai real-word checks can reach 0.25 false flags per 1K: Thai pair counts are sparser than English, and the
  segmentation step adds noise. This is the main thing the stage 1 spike must answer.
- Stage 2 quality: no Thai char-GRU typo model has been published; size and latency here are estimates.
- The exact entry count and licence version of the Wiktionary appendix (not fully read).
- The Lertpiya 2020/2023 full papers and data (IEEE pages blocked automated access); Nakwijit & Purver's data release.
- Gemma 4 E2B/E4B and SEA-LION v5 small models on Thai spelling: not tested.
- Mark-order problems that `normalize_thai` already fixes (เเ, ํา, tone before vowel) are invisible in VISTEC's
  preprocessed text, so their real frequency is unknown.

## Sources

- VISTEC-TP-TH-2021: https://github.com/mrpeerat/OSKut/tree/main/VISTEC-TP-TH-2021
- pythainlp/thai-misspell-correction: https://huggingface.co/datasets/pythainlp/thai-misspell-correction
- Thai Wiktionary appendix: https://th.wiktionary.org/wiki/ภาคผนวก:รายชื่อคำในภาษาไทยที่มักเขียนผิด
- ThaiEditBench: https://aclanthology.org/2026.aacl-short.17/ and https://github.com/yongsa-nut/ThaiEditBench
- Nakwijit & Purver 2022: https://arxiv.org/abs/2206.09680
- Lertpiya et al. 2020: https://doi.org/10.1109/ACCESS.2020.3010828 ; 2023: https://doi.org/10.1109/ACCESS.2023.3295004
- PyThaiNLP spell API: https://pythainlp.org/dev-docs/api/spell.html ; word list: https://github.com/PyThaiNLP/pythainlp
- pythainlp/word-spelling-correction-char2vec: https://huggingface.co/pythainlp/word-spelling-correction-char2vec
- tinyfix: https://huggingface.co/paperweight/tinyfix
- ByT5-small: https://huggingface.co/google/byt5-small ; mT5-small: https://huggingface.co/google/mt5-small
- Thai fine-tunes checked: https://huggingface.co/thanaphatt1/byt5-thai-misspelling-correction-experiment ,
  https://huggingface.co/kennyxaxa/fix_thai-eng_typo , https://huggingface.co/Mularstyle
- Small LLMs: https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b , https://huggingface.co/typhoon-ai/llama3.2-typhoon2-1b-instruct ,
  https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct , https://huggingface.co/Qwen/Qwen3-1.7B ,
  https://huggingface.co/google/gemma-3-1b-it , https://huggingface.co/google/gemma-4-E4B-it ,
  https://huggingface.co/SeaLLMs/SeaLLMs-v3-1.5B-Chat , https://huggingface.co/openthaigpt , https://huggingface.co/aisingapore
