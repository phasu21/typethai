# English real-word fixes from confusion pairs: spike

Date: 2026-11-21
Scope: can typeThai offer English real-word fixes (`form` → `from`, `their` → `there`, `then` → `than`,
`your` → `you're`) cheaply? The candidates come from the confusion list of
[paperweight/tinyfix](https://huggingface.co/paperweight/tinyfix), and the scores from the English word pair counts
(TTM section 13) and word frequencies (section 11) the model already ships. No new runtime library. Spike only: the
code is in `typethai-core` and an eval binary. The desktop engine and UI are unchanged.

## Summary

**It works for the words the pair table knows.** The test text is 907K words of held-out conversational English:
Tatoeba, Taskmaster-1 and Common Voice. No Wikipedia text was in the test. At the chosen setting the row fires on
**0.23 correct words per 1,000**; tinyfix reports about 0.5 on its own wider test text. In text with one injected
mix-up per line, the top-1 row is the right word **77.0%** of the time. That is **84.0%** of the cases where both
words have pairs, and 62.2% averaged over word pairs. **99.4%** of the rows offered on injected words are right, and
about 99.2% once the false flags on the other words of those lines are counted too. These figures use the previous
and the next word, so the row appears one word late. With the previous word only (the row offered as the word is
finished), recall falls to **61.8%** (47.5% averaged over pairs) at the same false-flag rate. A check costs 5–10 µs,
and the only new data is the 2.9 KB list.

**The big gap is contractions.** Section 13 has no word with an apostrophe (`corpus::word_runs` ends a run at one),
and section 11 has none either. So `your`/`you're`, `its`/`it's`, `they're`, `we're`, `who's` and `let's` cannot be
scored. Comparing them anyway (no evidence guard, same β and margin) gives **5.5 false flags per 1,000** instead of 0.23:
96% of the flags are a correct `it's`, `you're`, `we're`, `let's`, `they're` or `who's` losing to its pair. With the
guard, these sets are skipped: no false flags, but no fixes either. A rough recount of the apostrophe pairs from three
of the training sources fixes many of them (`you're` → `your` 81%). Its counts are not built like the shipped ones,
though, so it also adds false flags on `there` and `were`. The proper fix is to rebuild sections 11 and 13 with
apostrophe words kept.

**Verdict:** worth building. Stage 1 is the n-gram row, with the next-word ("one word late") mode as the target, plus a
model rebuild that keeps contractions. Stage 2, if wanted, is a pure-Rust port of tinyfix's GRU. Full-sentence fixing
stays an explicit-request LLM rewrite (see the last section).

## Update: built into the app, with model-v6 (2026-11-21)

The spike's stage 1 is now in the engine, and a model-v6 candidate keeps contractions.

**What the user sees.** A finished English word that is probably the wrong real word gets a red-marked (typo-style)
row with the word it was likely meant as, **one word late**: after `a letter form the ` the bubble offers *from*.
When punctuation ends the run (`form, `), the word itself is checked on its left alone, and the row keeps the
punctuation (*from,*). Tab (then Space, on the default Tab key), ⌃⌥1-3, Shift+arrows then a pick, or a click applies
it: the words from it to the caret are retyped with it replaced, and ⌃⌥Z puts the typed word back. Space alone,
Enter, typing the next letter (the row goes) and auto-convert never apply it; no text changes without a key press.
It is offered only when the word just finished has no spelling row of its own, only after a Space, never for a
capitalised word mid-sentence (a name), a word in capitals or a don't-translate word, and only with the typo
suggestions on. It is logged in the rank log under the source `real_word`, so accept and dismiss rates can tune it.

**Code.** `confusion::RealWords` (the sets, an owned `Scorer`, `MARGIN` 6.5 and per-set `MARGINS`);
`BubbleKind::RealWord { back, typed, word }`, `Engine::real_word_row` / `apply_real_word` in `engine.rs`.

**Model-v6** (`scripts/bigrams/model-v6.sh`; candidate in `~/typethai-artifacts/model-v6-candidate/`, SHA-256
`e4e8d480…`, not published). Model-v5 with its English sections rebuilt from the same corpora (`fetch.sh`; the
Tatoeba weekly export is newer than the pinned one, as the spike's was):
- `corpus::word_runs` keeps an apostrophe inside an English word (`you're`, `it's`, `don't`; ’ is read as '). An
  apostrophe at the start or end of a token is a quote mark and ends the run there.
- Section 11 gets the 90 contractions seen 10+ times in the English training runs (`tt-bigrams contractions`),
  scaled to the frequency list over the words both share (×33.0). Possessives (`john's`) are left out. Every other word
  keeps its frequency.
- Section 13 is counted again at model-v4's level (`--vocab --min-count 3`, all followers): 484,045 pairs (v5
  464,860), 17,673 of them with an apostrophe. Without those, 466,372, close to v5's count: the rebuild reproduces
  model-v5's English pairs apart from the newer Tatoeba export and the quote-mark change. `--en-only` counts the
  English pairs alone (16 s, 0.55 GB); the Wikipedia shard was needed and was light (a 420 MB download, 40M
  characters read).
- The Thai sections, the dictionary, the word lists and the classifier are model-v5's, byte for byte (the Thai pairs
  stay the LATTE union).
- Size: 3,533,850 → 3,573,097 bytes (+39 KB). The packed file is reproducible (same SHA-256 on a second pack).
- Held-out completion (`tt-bigram-eval`, all English held-out text, the same word reader for both): English context
  top 3 65.1% → 67.7%, shipped ranking 66.1% → 68.7%; Thai unchanged (79.4% / 79.5%).

**Margins.** A single margin cannot reach 0.1 false flags per 1,000 words without losing most of the recall the
noisy sets are not responsible for. `tt-confusion-eval --tune` picks one margin per typed word on dev (the best
trade of hits against false flags at one exchange rate, set so dev stays under the budget). Tuning every set
overfitted (dev 0.086, test 0.13-0.15 false flags per 1,000), so only typed words seen 300+ times on dev get their own
margin: base 6.5, 22 sets stricter (`than` 9, `too` 8, `two` 8.5, `off` 8, `to` 10.5, `there` 10.5, `they're` 14,
`we're` 15.5, ...).

Results, test half (907K words; Tatoeba, Taskmaster-1, Common Voice), left+1, shipped margins
(`tt-confusion-eval --beta 200 --shipped`):

| Model | False flags / 1K words | Recall | Macro recall | Precision | Contractions recall |
|---|---|---|---|---|---|
| model-v5 | 0.071 | 62.6% | 51.5% | 99.7% | 0 of 6,510 |
| **model-v6** | **0.092** | **68.3%** | **57.5%** | 99.8% | **2,752 of 6,510 (42.3%)** |

With model-v6 at the spike's margin 5 (one margin for all) it is 0.343 false flags and 84.8% recall: the strict
margins trade about 16 points of recall for a quarter of the false flags. Contractions, v6, shipped margins:
`you're→your` 1987/2282 (87%), `its→it's` 219/1092 (20%), `it's→its` 93/195, `they're→their` 110/308,
`they're→there` 58/628, `your→you're` 145/679 (21%), `we're→were` 50/404; false flags on typed contractions 8 (0.009
per 1,000 words). The noisy sets stay quiet: `too` 6, `two` 2, `than` and `off` 0 false flags on the test half.
Other text: Taskmaster + Common Voice alone (119K words) 0.109 false flags per 1,000, 67.2% recall; held-out
Wikipedia 0.119 and 52.5% (its training half is in the counts, but encyclopedic text has more names and rare words).
A check costs 6-30 µs per finished word in a set (release build, the machine shared with other builds).

Not done: per-set margins from the rank log's accept / dismiss counts (none collected yet); the GRU port (stage 2);
`I think your right` (the deciding word is two to the left, out of a bigram's reach).

## What was built (spike)

- `typethai_core::confusion` (`rust/crates/typethai-core/src/confusion.rs`):
  - `Confusions`: the tinyfix list (`data/tinyfix-confusions.txt`, embedded with `include_str!`; its MIT licence is
    in `data/tinyfix-LICENSE`). One set per line: the typed word, then the words it is mistaken for. There are 235 sets.
  - `Scorer`: `score(w) = ln P(w | prev) + ln P(next | w)`, with `P(b | a) = (c(a,b) + β·Pu(b)) / (M(a) + β)`.
    Here `c` is the stored pair count, `M(a)` the stored mass after `a`, and `Pu` the add-one unigram probability from
    section 11. It keeps a lowercased copy of section 11 (46,676 words). A missing `prev` uses `Pu(w)`, so at the
    start of a run the gain is a pure frequency ratio. A missing `next` drops the right-hand term, so in the next-word
    mode a word at the end of a run is judged on its left alone, against the same margin.
  - `Confusions::check(scorer, prev, typed, next, margin) -> Option<Fix>`: the best alternative if it beats the
    typed word by `margin` nats, in the typed word's case. **Evidence guard:** only words with stored followers
    (mass > 0) are compared. A typed word without them gets no row, and alternatives without them are never offered.
    Every set word that has followers is also in section 11.
  - It only returns a suggestion and never edits text. In the app it would be a `Typo`-style row (red mark) that the
    user accepts with a key, as typeThai's rule requires.
- `tt-confusion-eval` (`rust/crates/typethai-core/src/bin/tt-confusion-eval.rs`): the tinyfix-style bench below.

## Method

- **Model:** the shipped model-v5 `typethai.ttm` (`/Applications/typeThai.app`, identical to
  `typethai-artifacts/model-v5-candidate`, SHA-256 `36bd1e54…`). Section 13 holds 464,860 English pairs, and section 11
  holds 46,676 words.
- **Held-out text:** the English held-out split of `scripts/bigrams/extract.py`, using the same crc32-of-text rule.
  `build.sh` counts pairs from `text/train/*.txt` only, so no held-out line was counted. This holds for a newer
  Tatoeba export as well, because the split depends on the sentence text alone. The sources are Tatoeba (204K lines;
  the weekly export of 2026-11-21, so the file differs from the one in `SHA256SUMS`), and Taskmaster-1 dialogs (22K)
  and Common Voice (3.4K) from scb-mt-en-th-2020 (zip SHA-256 matches). The Wikipedia shard was not downloaded. Even
  lines form **dev**, and odd lines form **test**. Each half has about 907K words, 143K of them in a confusion set.
  The margin and β were chosen on dev, and every number below is from test.
- **Tokens:** a word is a run of letters and apostrophes (’ is read as '). Quote marks at its edges are trimmed. A
  run ends at any punctuation. Unlike section 13's counting, words with an apostrophe stay in the run. With the shipped
  pairs they only add a neighbour the pairs do not know, so that term falls back to `Pu`.
- **Clean text (false flags):** every word that is in a confusion set is checked as written. A row is a false flag.
  Capitalised words are skipped as names (tinyfix maps them to `<name>`), except at the start of a line or after
  `. ! ? " “ : (`. All-caps words are skipped too.
- **Injected text (recall, precision):** for each line that has a word some set lists as a target, one such word is
  replaced. A hash picks the word and the typed form, for example `from` → `form`, `there` → `their` or `to` → `too`.
  There are 78K cases per half. Columns:
  - *Recall*: the share of cases where the top-1 row is the original word.
  - *Fixable*: recall over the cases where both words have pairs, which excludes the contraction cases the guard skips.
  - *Macro*: recall averaged over the 146 typed→wanted pairs with at least 20 cases. The plain figure is dominated by
    `too`/`two` → `to`, `off` → `of` and `than` → `that`.
  - *Precision*: the share of the rows offered on injected words that are right.
  - *All*: the same, but with the clean false-flag rate applied to the other words of each line. It is an estimate of
    precision on text that has one mix-up per line.
- **Modes:** *left* uses the previous word only, so the row is offered when the word is finished. *left+1* also uses
  the next word, so the row is offered one word late, after the next word is typed.
- **Selection:** margins were swept from 2 to 14 in steps of 0.5, for β ∈ {50, 200, 1000}, with and without the recount.
  The rule: for each mode, take the smallest margin whose dev false-flag rate is at most 0.25 per 1,000 words, then the
  β with the best dev recall. That gives β = 200 and margin 5 for both modes. Test recall at the chosen margin, in left
  / left+1 mode: β 50 57.9% / 76.1%, **β 200 61.8% / 77.0%**, β 1000 55.5% / 75.6%.

## Results (test half, β 200)

| Pairs | Mode | Margin | False flags / 1K words | / 1K set words | Recall | Fixable | Macro | Precision | All |
|---|---|---|---|---|---|---|---|---|---|
| shipped, **no guard** | left | 5 | 5.54 | 35.3 | 66.8% | 72.9% | | 99.1% | 93.1% |
| shipped, **no guard** | left+1 | 5 | 5.54 | 35.2 | 82.2% | 89.7% | | 99.3% | 94.3% |
| **shipped + guard** | left | 5 | **0.232** | 1.47 | 61.8% | 67.4% | 47.5% | 99.5% | 99.2% |
| **shipped + guard** | **left+1** | 5 | **0.229** | 1.46 | **77.0%** | **84.0%** | **62.2%** | 99.4% | 99.2% |
| shipped + guard, stricter | left | 6.5 | 0.097 | 0.62 | 49.1% | 53.6% | | 99.6% | 99.4% |
| shipped + guard, stricter | left+1 | 7.5 | 0.067 | 0.43 | 63.7% | 69.4% | | 99.7% | 99.6% |
| shipped + guard, looser | left | 4.5 | 0.362 | 2.30 | 66.6% | 72.7% | | 99.4% | 99.0% |
| shipped + guard, looser | left+1 | 3.5 | 0.473 | 3.01 | 81.6% | 89.0% | | 99.2% | 98.8% |
| + apostrophe recount | left | 5 | 0.250 | 1.59 | 67.0% | | 54.4% | 99.4% | 99.1% |
| + apostrophe recount | left+1 | 8 | 0.232 | 1.47 | 68.2% | | 53.7% | 99.6% | 99.4% |

The "stricter" rows keep the dev false-flag rate at or below 0.1 per 1,000, and the "looser" rows at or below 0.5. In
the recount rows, apostrophe words have pairs, so fixable recall equals recall. Of the false flags at the chosen
setting, 2 of 210 (left) and 21 of 208 (left+1) are on the first word of a run, where only frequency decides.

For reference, tinyfix measures on its own test text, typed through its app, and also offers edit-distance-1
candidates. It reports about 0.5 wrong changes per 1,000 words. On "classic mix-ups put into email, chat and
encyclopedia text" it fixes 69%, and 99% of its fixes are right. That is the closest of its sets to this one, but the
text, the injection and the candidates differ, so the numbers are not directly comparable.

**By source** (shipped + guard, margin 5, test):

| Source | Words | left: false flags / 1K, recall | left+1: false flags / 1K, recall, fixable, macro, precision |
|---|---|---|---|
| Taskmaster + Common Voice (chat-like) | 119K | 0.311, 53.5% | 0.286, 72.8%, 82.3%, 61.2%, 99.5% |
| Tatoeba | 788K | 0.220, 62.9% | 0.221, 77.6%, 84.2%, 63.3%, 99.4% |

Tatoeba has many near-duplicate sentences. The split is by exact text, so near-duplicates of training lines leak into
the held-out half and flatter the pair counts. The chat sources are the fairer figure: their recall is close, but they
have about 0.3 false flags per 1,000, slightly above the 0.25 target.

**Classic pairs** (shipped + guard, left+1, margin 5, test, hits / cases):
`their→there` 468/676 (69%), `there→their` 293/352 (83%), `then→than` 253/341 (74%), `than→then` 40/211 (19%),
`to→too` 100/278 (36%), `too→to` 7846/8246 (95%), `to→two` 122/213, `were→where` 153/200, `where→were` 354/430,
`form→from` 1188/1390 (85%), `of→off` 88/509 (17%), `off→of` 4725/5902 (80%), `hear→here` 744/961 (77%),
`no→know` 660/875, `weather→whether` 98/107, `quiet→quite` 106/168.
**Contractions: 0** (`your→you're` 0/677, `you're→your` 0/2295, `its→it's` 0/1073, `it's→its` 0/197,
`they're→there` 0/640).
With the apostrophe recount (left+1, margin 8): `you're→your` 1862/2295 (81%), `they're→there` 409/640,
`its→it's` 405/1073 (38%), `your→you're` 151/677 (22%).

**Where false flags come from** (left+1, margin 5, test, flags / occurrences): `than` 29/1233, `too` 20/1061,
`off` 18/763, `two` 14/854, `our` 11/1010, `to` 11/31555. Most are the same high-frequency short words. A per-set
margin is the obvious next tuning step: larger for `than`/`that` and `too`/`two`/`to`, as tinyfix requires a bigger
margin for a less likely slip.

**Cost:** `Confusions::best` alone takes 5–10 µs per checked word (release build, one thread, Apple Silicon, the
neighbours prepared beforehand). This includes lowercasing three strings and a linear scan of each candidate's
followers in `Scorer::count`; a binary search per group would make the scan cheaper. About 16% of words are in a set,
and the check runs once per finished word, not per key. Memory: the pairs the app already loads, plus a lowercased copy
of section 11 (46,676 words, a few MB; it could share the completer's copy). Size: the 2.9 KB list.

**The recount variant** adds 35,665 pairs with an apostrophe word (count ≥ 2) from the training split of Tatoeba,
Taskmaster and Common Voice to the shipped pairs. It also adds 2,490 apostrophe words to the frequency list, scaled
from the same text. It gains on contractions and on left-only recall. Its counts are unpruned and leave out the
Wikipedia shard, so they do not match the shipped table. In left+1 mode, `they're` and `we're` then beat a correct
`there` and `were` (59 and 55 false flags). The variant shows what a rebuild would gain; it is not the rebuild.

## What it would take in the app

1. **Data (model-v6):** keep apostrophe words in English runs in `corpus::word_runs` (`Class::Latin` with `'`),
   counted and pruned like every other pair. Add their frequencies to section 11 (`tt-pack --enfreq` drops them today).
   Then re-run this eval with the guard; the contraction sets then have pairs on both sides and are scored.
2. **Engine:** after a forward boundary, when the Speller has no typo row for the word and `Confusions::check` returns a
   fix, offer it as a `Typo` row on the existing path (`en_items`), so Undo works. For **left+1** mode, the row is for
   the word *before* the one just finished. Accepting it rewrites a word behind the caret, which the Guess walk-back
   (`apply_guess`) already does for multiple words. Expire the row after one more word.
3. **Settings:** behind the existing typo switch, with the stricter margins first (about 0.07–0.1 false flags per 1,000).
   Then tune per-set margins against the rank log's accept/dismiss counts, not this offline eval alone.

## Path to full-sentence fix

"Full sentence fix" means more than one word changes: grammar and agreement, missing words, word order, and Thai as
well as English. There are two ways to get there without ONNX Runtime.

| | tinyfix-style GRU scorer, pure Rust | Small LLM rewrite (the existing ⌃⌥R path) |
|---|---|---|
| What it fixes | One word at a time, chosen from candidates (confusion sets + edit-1 real words). No insertions or reordering | Anything: grammar, missing words, word order, Thai and mixed text |
| Model | 1.9M params: 20K×64 embeddings, 2 GRUs (hidden 256) over 16 words left and 4 right, scored against candidate embeddings. 2.1 MB at int8 | Thai-capable tier: 3–4B Q4 (Typhoon2.5-Qwen3-4B, 2.5 GB file) or Gemma 4 8B. At 1B, Thai instruction following is poor (`llm-rewrite-selection.md`) |
| Runtime | A hand-written forward pass, as the LATTE spike's section 4 sketches for its 1.17M-param CNN+BiLSTM: roughly 200–300 lines, nothing added to the binary. The MIT weights would be read from tinyfix's ONNX file at build time (with the `onnx` Python package, never the `.pt` pickle) into our own format | Ollama today (local `gemma4:latest`); `llama-cpp-2` with Metal if embedded (`embedded-llm-next-word-prediction.md`) |
| Latency (estimate) | About 5M multiply-adds per checked word (20 GRU steps × 3·320·256), so roughly 0.5–1.5 ms scalar and much less with SIMD. The left state can be carried across words, so about one step per word. tinyfix reports about 3 ms in browser WASM | Measured: 1.3 s p50 for the 4B and 2.2 s for Gemma 4 warm through the app's contract, and 2.6–5.7 s for the first request after a load (`llm-sentence-completion-app-contract.md`). Explicit request only |
| Size / memory | +2.1 MB asset, a few MB of RAM | 2.5–5 GB download, about 1–5 GB resident, opt-in only |
| Quality (expected) | tinyfix's own numbers: 69% of classic mix-ups, 29% of finger slips that make a real word, 18% of real GitHub typos, about 0.5 false changes per 1,000 words. Better than bigrams on long context (`I think your right` needs `think` *and* `right`). **English only**. A Thai version would mean training our own (tinyfix took about 1 h 45 min on one A10G) | Best coverage, but it rewrites more than it should and can invent text (`llm-sentence-completion-experiment.md`). It needs a diff preview and the app's strict output validation. Not usable as automatic correction |
| Licence | Weights and code MIT (Caleb Fahlgren). Its training data (FineWeb-Edu, SmolTalk, OASST1) would need a note in `DATA_LICENSES.md` | As today: model licences per download |

**Recommended staged plan:**

1. **Now:** the n-gram confusion row from this spike, in left+1 mode at a strict margin, and model-v6 with
   contraction pairs. It costs 5–10 µs per checked word and needs no new dependency. Measure accept and dismiss rates in the rank log.
2. **Next, only if stage 1 sticks:** port tinyfix's GRU to pure Rust as a second English scorer. It would cover edit-1
   real-word slips and long context. Gate it on this same bench plus tinyfix-style real-typo sets, and require it to
   beat stage 1 at an equal false-flag rate. A Thai single-word scorer would be a separate training project.
3. **Full sentence:** keep it as an explicit request (⌃⌥R "Fix typos", local model first), shown as a word-level diff
   that the user accepts. Use stages 1–2 to flag which sentence is worth sending, never to send text by themselves. An
   always-on LLM pass is out of reach on latency (over 1 s) and memory (GBs) for a keyboard helper.

## Reproduce

```sh
# held-out text: scripts/bigrams/extract.py's heldout/{tatoeba,taskmaster,commonvoice}-en.txt (same crc32 rule)
cd rust && cargo build --release -p typethai-core --bin tt-confusion-eval
M=/Applications/typeThai.app/Contents/Resources/typethai.ttm
./target/release/tt-confusion-eval $M --beta 200 --margins 3.5,4.5,5,6.5,7.5 --report 5:5 heldout/*-en.txt
./target/release/tt-confusion-eval $M --beta 200 --no-guard --margins 5 heldout/*-en.txt
# apostrophe recount variant
./target/release/tt-confusion-eval $M --beta 200 --recount train/tatoeba-en.txt,train/taskmaster-en.txt,train/commonvoice-en.txt \
  --margins 5,8 --report 5:8 heldout/*-en.txt
```

Each run takes about 5 s and a few hundred MB.

## Not done

- The recall figures are offline: the engine's typing path is covered by its tests and `--simulate`, not by a typed
  run over the held-out text.
- Candidates are the 235 sets only. Edit-1 real-word slips (tinyfix's "finger slips" set) and real typo corpora
  (children's writing, GitHub commits) were not evaluated.
- English only. Nothing for Thai real-word errors.
- The GRU port and LLM latency figures are estimates or earlier measurements. Nothing new was run for them.
