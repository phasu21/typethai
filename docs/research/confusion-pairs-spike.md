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

## What was built

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

- No typing through the engine: row timing and its interaction with the Speller rows, completion and Guess are not
  simulated.
- Candidates are the 235 sets only. Edit-1 real-word slips (tinyfix's "finger slips" set) and real typo corpora
  (children's writing, GitHub commits) were not evaluated.
- The model-v6 rebuild with apostrophe pairs (it needs the Wikipedia shard and the full `build.sh`) was not done.
- Per-set margins were not tuned.
- English only. Nothing for Thai real-word errors.
- The GRU port and LLM latency figures are estimates or earlier measurements. Nothing new was run for them.
