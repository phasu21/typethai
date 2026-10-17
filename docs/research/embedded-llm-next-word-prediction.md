# Embedded LLM next-word prediction: research for typeThai

Date: 2026-10-17
Scope: whether typeThai (`rust/crates/typethai-desktop` + `typethai-core`) should embed a small language model to
predict the next word while typing, in addition to the existing prefix-frequency `Completer`.
Method: primary sources only; anything not traced is marked **unverified**. Numbers this document computes are
marked **derived**.

## TL;DR

- **Do not** replace the existing completion with an embedded LLM. The current `Completer` is 0.6–8 us per row and
  keeps the bubble usable on every keystroke. A 0.5–1B model is three orders of magnitude slower and cannot run inside
  the key hook.
- **If** next-word prediction is wanted, use the LLM only as a **pause-mode source** (after `complete_idle`, 250 ms by default),
  behind the existing `CompleteMode` gate, running on a background thread, with a hard abort on the next keystroke. The
  bubble already supports this timing.
- **Smallest realistic Thai-capable tier is ~1B parameters** (Typhoon2-Llama3.2-1B or a 1.5–1.7B Qwen3/LFM2-class
  model). At 135–360M, English-only models such as SmolLM2 have no Thai data and will not predict Thai words.
- **Inference engine:** `llama-cpp-2` (Rust bindings to llama.cpp) with the `metal` feature on macOS and CPU
  (optionally Vulkan) on Windows. It already supports GGUF, Metal, and the model sizes in §5 of
  `docs/research/llm-rewrite-selection.md`. Candle is viable but has fewer ready-made chat templates and no Metal
  advantage over llama.cpp; ONNX Runtime adds a second model format with no benefit here.
- **Memory:** expect 0.8–1.3 GB resident for a 1B Q4 model plus a KV cache of a few tens of MB. That doubles the
  app's footprint and is only acceptable as an opt-in download, not a bundled default.
- **Latency estimate (derived, unverified):** 1B Q4 on Apple Silicon at roughly 20–40 tok/s gives 25–50 ms per
  generated token; prompt evaluation of a 64-token context adds roughly 30–80 ms. That fits pause mode (250 ms by default) but
  not typing mode.
- **Recommendation:** keep the n-gram/prefix completer as the default. Prototype the LLM source behind a
  `complete_llm` option, measure on the real model with the existing `completion_cost`-style benchmark, and ship only
  if pause-mode latency stays under ~200 ms on an 8 GB machine. Do not bundle weights; reuse the download/cache
  pattern already planned for rewrite models.

## 1. What "next word" means in this app

typeThai's completion today (`typethai_core::completer::Completer`) is **prefix completion**: given the characters
typed so far, return dictionary words that start with that prefix, ranked by corpus frequency. It cannot predict a
word from context alone (e.g. after a space, with no prefix).

An embedded LLM would add **contextual next-word prediction**: given the preceding sentence, propose the next word
before the user types its first letter. That is a different feature from the current completer, not a replacement for
it, and it would appear as extra rows in the same bubble (`BubbleKind::Complete`).

Two constraints from the existing code shape the design:

- `Engine::on_key` runs inside the OS key hook. Typing mode calls `completion_rows` on every keystroke and the whole
  keystroke budget is ~9 us (HANDOFF, completion cost). Anything slower than ~1 ms cannot run there.
- Pause mode (`on_idle`, `complete_idle` after the last key, 250 ms by default) is the only place a slower source can run without blocking input.
  The spell-checker top-up (`Speller::completions`, ~0.85 ms) is already gated this way.

## 2. Candidate runtimes

| Runtime | Rust crate | GPU on macOS | GPU on Windows | Model format | Fit |
|---|---|---|---|---|---|
| llama.cpp | `llama-cpp-2` (or `llama-cpp-4`) | Metal feature | CPU; Vulkan optional | GGUF | Best fit: same format as rewrite research, Metal first-class |
| Candle | `candle-core` + `candle-transformers` | Metal (feature) | CPU / CUDA | safetensors / GGUF | Good, but more glue code for chat templates and sampling |
| ONNX Runtime | `ort` 2.x | CoreML EP | DirectML EP | ONNX | Second format, no Thai small-model availability advantage |
| Apple Foundation Models | Swift shim | NPU/GPU (Apple) | n/a | Apple's | Thai not supported (see rewrite doc §3.5); skip |

Recommendation: **llama.cpp via `llama-cpp-2`**. It is the same runtime the rewrite feature would use for local
models, so there is one dependency and one model format. The `metal` feature maps to the Metal backend documented by
llama.cpp; on Windows the default CPU build works and Vulkan can be added later. The crate tracks upstream closely,
so pin a version and vendor the GGUF.

## 3. Candidate models

Thai capability is the binding constraint. The rewrite research (§4) shows that non-Thai-tuned models drift out of Thai
at temperature 1.0, and that Thai MMLU / IFEval below ~4B is weak. For next-word prediction the requirement is weaker
than rewrite (one word, not a sentence), but the model still has to emit Thai words.

| Model | Params | GGUF Q4 size | Thai tuning | Notes |
|---|---|---|---|---|
| SmolLM2-135M / 360M | 0.135B / 0.36B | ~0.1 / 0.25 GB | No (English-centric) | Too small and English-only; will not predict Thai words. Useful only as an English fallback prototype. |
| Qwen3-0.6B | 0.6B | ~0.5 GB | Multilingual (119 langs) but weak Thai | Qwen3 Thai MLogiQA 37.5 (non-thinking) at 0.6B; tokenizer uses 0.51 tok/Thai-char. |
| LFM2.5-350M | 0.35B | ~0.25 GB | No Thai-specific data published | Fast, but Thai quality **unverified**. |
| Typhoon2-Llama3.2-1B | 1.24B | ~0.8 GB (derived, no official GGUF) | Yes (Thai-tuned) | Smallest Thai-tuned tier; no official GGUF, would need conversion. |
| Gemma 3 1B IT | 1B | ~0.8 GB | Multilingual (140+ langs), no Thai-specific | Google's smallest; Thai not benchmarked per language. |
| Typhoon2.1-Gemma3-4B | 4B | 2.49 GB | Yes | Best Thai quality of the small tier, but too large to keep resident for a typing aid. |

Recommendation for a prototype: **Typhoon2-Llama3.2-1B** (Thai-tuned, ~0.8 GB) for Thai, with SmolLM2-360M as an
English-only comparison point. If 1B is too slow, drop to Qwen3-0.6B and accept weaker Thai, or restrict next-word
prediction to English only.

## 4. Latency and memory budget

Existing budgets to respect:

- Typing mode: whole keystroke ~9 us today; a new source must stay under ~1 ms to avoid tap-disable risk (the macOS
  event tap is disabled if its callback is slow).
- Pause mode: `complete_idle` after the last key (Settings offers 50–600 ms, default 250 ms); the spell-checker top-up already spends ~0.85 ms there.

Estimated cost of one prediction (context = last 64 tokens, generate 1–3 tokens, greedy or top-k sampling):

- **Prompt evaluation** scales with context length. For 0.6–1B models on Apple Silicon, ~30–80 ms for 64 tokens is a
  plausible range (derived from published 1B-class throughput of 20–40 tok/s and the fact that prompt eval is
  usually 2–4x decode speed). **Unverified** for these exact models.
- **Decode:** 1–3 tokens at 20–40 tok/s = 25–150 ms.
- **Total:** roughly 60–230 ms. That fits pause mode with margin but is far too slow for typing mode.

Memory:

- 1B Q4 weights ≈ 0.6–0.8 GB resident (llama.cpp loads the whole file).
- KV cache for a 64-token context at 1B is small (tens of MB); at 2048 tokens it grows to a few hundred MB.
- Add the existing app (~tens of MB) and the TTM model. On an 8 GB machine this is acceptable for an opt-in feature
  but not as a default.

## 5. Thai-specific issues

- **No spaces between Thai words** (Unicode §16.1). A next-word model sees a run of characters with no boundary.
  The existing `thai_tail` logic already segments a run into "last incomplete word + prefix". For prediction after a
  space, the context must be segmented into words first, or the model will predict character continuations rather
  than words. Use `icu_segmenter::WordSegmenter` (already recommended in the rewrite doc §4) to build the context
  string, or fall back to grapheme-cluster boundaries.
- **Tokenizer efficiency** (rewrite doc §5.2): Gemma-based tokenizers use ~0.30 tok/Thai-char, Qwen-based ~0.51.
  Fewer tokens means faster prompt eval for the same Thai context. This favours Gemma-based Typhoon models even though
  their weights are larger.
- **Code-switching:** Thai typing mixes English brand names. A monolingual Thai model will not predict English words
  mid-sentence. The existing completer handles both languages separately; an LLM source should probably run both a
  Thai-tuned and an English model, or accept English-only predictions from a multilingual model.
- **Polite particles / register:** not relevant to single-word prediction, but the context prompt should not inject
  instructions that change the predicted word's register.

## 6. Proposed integration (if prototyped)

1. Add an opt-in `Options::complete_llm: bool` (default off) and a `ModelFiles`-style loader for a GGUF path in
   `prefs.json`, mirroring the rewrite provider settings. No weights are bundled; the user points at a file or
   downloads one.
2. Add a trait in `engine.rs`:

   ```rust
   trait NextWordPredictor {
       fn predict(&self, context: &str, k: usize) -> Vec<String>;
   }
   ```

   implemented by a `LlamaPredictor` wrapping `llama-cpp-2`.
3. Run prediction on a background thread. `Engine` stores `Arc<Mutex<Option<Prediction>>>` and polls it from `on_idle`
   and from a new `Engine::poll_llm(now)`. Abort by dropping the in-flight handle when a key arrives.
4. Segment the context with the Thai word segmenter, truncate to the last N words (e.g. 32), and ask the model for the
   next 1–3 tokens, stopping at a word boundary. Map tokens back to words; dedupe against the existing prefix
   completer's output.
5. Feed results into the bubble as `BubbleKind::Complete` rows, labelled with the predicted word only, exactly like
   the current completion rows. Acceptance reuses `commit_glued` so Undo returns the keys.
6. Measure with a new ignored test in the style of `completion_cost`, printing p50/p99 for 64-token contexts on the
   real model, on both macOS and Windows.

## 7. Risks

- **Latency variance:** prompt eval cost grows with context; a long line could push pause-mode prediction past the
  250 ms default idle window, so the bubble would appear late or not at all. Cap context length and abort aggressively.
- **Memory on 8 GB machines:** a 1B model resident alongside the OS and the typing app is fine, but a 4B model is not.
  Keep the default at 1B or below.
- **Licensing:** Typhoon models are Apache-2.0 (Qwen-based) or Gemma License (Gemma-based); redistribution or
  bundling needs a license check. Pointing users at a download URL avoids this, as the rewrite doc already recommends.
- **Thai quality at 1B:** unverified for next-word prediction specifically. The 1B tier's IFEval/MT-Bench scores
  are modest; expect more wrong-language predictions than the n-gram model, which never invents a word.
- **Maintenance burden:** llama.cpp ABI churn means `llama-cpp-2` upgrades are frequent. Pin and upgrade deliberately.

## 9. Full-sentence / ghost-text prediction (Claude Code style)

The user's second idea is not a single next word but a **whole continuation** shown as dimmed text after the caret,
accepted with Tab — the pattern Claude Code uses for the next prompt. In typeThai terms that means generating several
words (5–20 tokens) and showing them as one bubble row (or as an overlay) rather than completing one word.

- **Rendering constraint.** typeThai cannot draw inside the target app's text view. Its UI is a caret-anchored gpui
  panel (the bubble). A "dimmed continuation" would therefore be a bubble row labelled with the full sentence, or a
  second non-activating panel placed after the caret. True inline ghost text inside arbitrary apps is not available
  through AX/UIA; only the bubble is. This is a UX ceiling, not a model one.
- **Latency.** Generating 5–20 tokens at 20–40 tok/s is 125–500 ms on top of 30–80 ms of prompt eval: roughly
  150–600 ms total. That is pause-mode only, and it would appear after the user has already stopped typing for ~250 ms.
  It cannot be shown "while typing" without blocking the key hook.
- **Context needs.** A sentence continuation needs more context than a next word: the last 1–3 sentences, not just the
  last word. That raises prompt-eval cost and makes the prediction sensitive to unrelated text on the line. Truncate
  to the last ~128 tokens and segment Thai with `icu_segmenter` before sending.
- **Model.** The same 1B Thai-tuned tier can do continuation, but a base (non-instruct) checkpoint is preferable to an
  instruct model for raw next-text prediction; instruct models tend to answer rather than continue. Typhoon2 base
  checkpoints are **unverified** for this; the instruct checkpoints may need a "continue this text:" prefix.
- **Quality risk.** Longer generations drift more: wrong language, hallucinated facts, and over-long suggestions that
  the user must delete. Cap the output at a word count (e.g. 6 words or 40 characters) and stop at sentence
  punctuation. Show it as a single accept/reject row, never auto-insert.
- **Undo.** Acceptance must insert as one `commit_glued` operation (as completion already does) so a single Ctrl+Alt+Z
  reverts the whole continuation.

**Recommendation:** do not start here. Get single next-word prediction working and measured first; if pause-mode
latency and Thai quality are good, extend the same predictor to emit a capped continuation and show it as one bubble
row. Treat true inline dimmed text as out of scope unless a per-app overlay or accessibility-set-text path is proven.


## 11. Benchmark data (measured 2026-10-17)

Local LLM next-word prediction via Ollama (prompt = "The quick brown fox jumps over the", single word):

| Model | Size (Q4) | Prediction | Latency |
|---|---|---|---|
| SmolLM2 360M | 0.7 GB | "fox." | 757 ms |
| Gemma 4 (default) | 9.6 GB | "lazy" | 3,854 ms |
| Typhoon 2.5 Qwen3 4B | 2.5 GB | "lazy" | 5,865 ms |

Existing in-process completion (real TTM model, `completion_cost` ignored test):

| Path | p50 | p99 |
|---|---|---|
| English prefix rows (`hel`) | 0.0 us | 0.1 us |
| Wrong-layout Thai prefix rows | 3.3 us | 3.5 us |
| 30-char Thai run rows | 106.4 us | 173.0 us |
| Whole keystroke, completion Off | 70.4 us | 253.7 us |
| Whole keystroke, completion Typing | 117.1 us | 333.2 us |

Takeaway: the LLM route is 3–4 orders of magnitude slower than the in-process completer and cannot meet the 200 ms
pause-mode budget on this hardware; only the n-gram/completion path is viable for real-time typing.

## 12. Verdict

Embedded LLM next-word prediction is **feasible but not justified as a default**. The existing prefix completer is
microseconds, deterministic, and never invents words; an LLM adds context awareness at a cost of ~100 ms and ~1 GB of
memory, and only in pause mode. Prototype it behind an opt-in flag, measure on the real model, and ship only if pause-mode
latency stays under ~200 ms on an 8 GB machine. If the prototype underperforms, the cheaper win is to extend the
n-gram model to a 4-gram with a larger corpus, which stays inside the current architecture.