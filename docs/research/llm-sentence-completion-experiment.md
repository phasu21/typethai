# Local sentence-completion experiment

Date: 2026-10-18. Decision: **do not enable either tested configuration as automatic sentence completion.** Typhoon base produced useful Thai/mixed-language suffixes at promising warm latency, but also invented facts, repeated text, and mishandled a partial English word. Qwen failed the suffix contract with the baseline and both follow-up prompt/template configurations. These results concern the exact artifacts, settings, and small synthetic corpus below—not all configurations of either model family.

No desktop/UI behavior was changed. No private typing, application contents, or API credentials were used. Two model downloads remain in Ollama's cache; the experiment unloaded both from inference memory afterward (`/api/ps` returned no models).

Follow-up: [larger-model experiment](llm-sentence-completion-larger-experiment.md) compares Typhoon 1B, Typhoon 2.5 4B and Gemma 4 8B on 24 fresh prefixes. Gemma produced more plausible continuations in several cases, but both larger models still invented unknown identifiers and failed English insertion spacing.

## Reproducible artifacts

- [Runner and all 24 synthetic prefixes](../../scripts/bench-sentence-completion.py).
- [Baseline: Qwen instruction prompt and Typhoon raw prefix](sentence-completion-2026-10-18.jsonl).
- [Qwen fixed few-shot prompt](sentence-completion-few-shot-2026-10-18.jsonl).
- [Qwen few-shot prompt with official hard-off template](sentence-completion-official-template-2026-10-18.jsonl).
- [Research rationale and sources](llm-sentence-completion.md).

The JSONL files preserve each request, unmodified suffix, any returned thinking, completion reason, token counts, server timing, wall timing, model metadata/template, runtime version, and loaded-model allocation. Each has final summary records. All 292 requests completed: 288 warm requests plus four first requests after model unloading. Each configuration used 24 cases repeated three times; output text was identical across the three repetitions for every case. The repetitions improve timing coverage, not linguistic sample size.

## Environment and method

Observed hardware: MacBook Air, Apple M5, 24 GiB unified memory, 10-core CPU and 10-core GPU. Runtime reported macOS 26.5.1, arm64; Ollama 0.40.5, `llamacpp` runner. Both models' reported loaded allocation equaled `size_vram`. This is not a full process-RSS, peak-memory, or power measurement.

| Artifact | Quantization | Ollama digest | Reported loaded allocation |
|---|---|---|---:|
| `qwen3:1.7b` | Q4_K_M | `8f68893c685c3ddff2aa3fffce2aa60a30bb2da65ca488b61fff134a4d1730e7` | 1,214,555,094 bytes |
| `hf.co/mradermacher/llama3.2-typhoon2-1b-GGUF:Q4_K_M` | Q4_K_M | `457db4db0aac0f3663108c6986addf6d8ef8dedb39b7494664d3032ac4973f8d` | 936,189,623 bytes |

The Qwen artifact is identified by its tag and digest, not an independently verified weight conversion. Its GGUF metadata reports `general.size_label=1.7B` but `general.parameter_count=2031739904` and Ollama's display says 2.0B. This discrepancy is preserved, not silently reconciled. Typhoon's `/api/show` reports Q4_K_M and 1.24B; `/api/ps` reported its quantization as unknown. The [community quantizer's card](https://huggingface.co/mradermacher/llama3.2-typhoon2-1b-GGUF) identifies the source as the [official pretrained/base checkpoint](https://huggingface.co/typhoon-ai/llama3.2-typhoon2-1b), not its instruct sibling. The quantizer repository revision observed was `db5001f9070f76759f60ef9f171a1d05cfd4ef04`.

Settings held fixed: temperature 0 (greedy), seed 42, 2,048-token context, maximum 32 generated tokens, newline stop. Model-specific remaining defaults are captured in `/api/show` records. This is a deployment-configuration comparison, not an equal-compute architecture benchmark. A token cap is not an equal word/character limit across tokenizers/languages.

There were eight English, eight Thai, and eight mixed-language cases. Each group included a partial word and a missing-date trap. Prefixes were shuffled with fixed seeds for each repetition, using the same orders for each configuration. Models ran serially, not in competition with each other. The app/device workload was not otherwise controlled. Only prefixes were supplied: no right-of-caret text and no prior conversation.

Each model was explicitly unloaded before the first request and kept resident for the warm set. **“Cold” here means model-runner unloaded, not cleared OS filesystem cache.** The follow-up Qwen cold timings are consequently not comparable to the initial load as storage-cold measurements. Warm timings may benefit from normal runtime prompt caching; this is not a no-cache benchmark or a realistic keystroke replay.

## Measured latency

Times below are milliseconds, rounded. Wall time covers the localhost streaming request through its final completion event. It excludes any typing-pause trigger, UI rendering, quality filtering, and insertion. First-response time is the first nonempty response chunk, **not the first usable suggestion**. Fast invalid output does not count as a successful completion.

| Configuration | Warm wall p50 | Warm wall p95 | Warm wall p99 | First response p50 | First response p95 | First request after unload |
|---|---:|---:|---:|---:|---:|---:|
| Qwen, baseline instruction | 204 | 374 | 399 | 46 | 51 | 5,432 |
| Typhoon, raw base continuation | 84 | 260 | 266 | 12 | 18 | 1,046 |
| Qwen, few-shot / Ollama template | 105 | 202 | 214 | 49 | 71 | 1,586 |
| Qwen, few-shot / official template | 88 | 157 | 168 | 48 | 51 | 1,173 |

Each row has 72 warm requests. p50 is the median; p95/p99 use nearest rank. With this small sample, p99 is effectively the maximum and does not estimate production tail behavior reliably. Initial server-reported model load time was 4,889 ms for Qwen and 785 ms for Typhoon.

**Finding:** the earlier single-request multi-second measurements are not a universal lower bound for warm local completion. This device did generate some useful short continuations quickly. However, lower latency in the Qwen follow-ups mostly reflects shorter bad outputs, not a better product.

## Output quality: baseline

### Qwen: the requested suffix contract failed

Every one of the 24 unique baseline cases repeated the entire literal input prefix somewhere in its output (72/72 warm requests). This is mechanically checked, not a subjective quality rating. It would duplicate existing text if directly inserted. Some outputs also quoted the prefix, gave instructions instead of a continuation, or invented dates.

Examples of raw outputs:

| Prefix | Qwen baseline output | Problem |
|---|---|---|
| `I have attached the report for your` | `I have attached the report for your review.` | Full sentence rather than suffix |
| `Thanks for your help. I really apprec` | `"Thanks for your help. I really apprec"` | Quoted prefix, partial word still incomplete |
| `The deadline สำหรับงานนี้คือ ` | `The deadline สำหรับงานนี้คือ 30th April 2024` | Unsupported date and repeated prefix |
| `ขอบคุณมากสำหรั` | `ขอบคุณมากสำหรั้น` | Repeated prefix and malformed completion |

Thinking was disabled through Ollama's `think:false`; all recorded `thinking` strings were empty. We did not strip repeated prefixes or quotes to make the results look successful.

### Typhoon: useful suffixes, but unsafe and inconsistent

Some raw continuations were plausible direct insertions:

| Prefix | Typhoon suffix | Concatenated interpretation |
|---|---|---|
| `ขอบคุณที่ช่วย` | `เหลือ` | `ขอบคุณที่ช่วยเหลือ` |
| `รบกวนช่วยส่งเอกสารให้` | `หน่อยค่ะ` | A natural polite request, but assumes a politeness particle |
| `ช่วย review โค้ดส่วนนี้ให้` | `หน่อยครับ` | Natural mixed-language request, likewise assumes a particle |
| `เดี๋ยวตรวจสอบรายละเอียดแล้ว` | `จะแจ้งให้ทราบต่อไปครับ` | Coherent continuation |
| `I will check the details and get back to` | ` you.` | Correct suffix including leading space |

These are engineering judgments, not native-speaker ratings or user acceptance measurements. Important failures:

- `The meeting has been moved to ` → `6:30pm on Tuesday, 12th February 2019.` No date/time was supplied.
- `เลื่อนประชุมไปเป็นวัน` → an invented date, time, and meeting room, eventually clipped by the token limit.
- `The deadline สำหรับงานนี้คือ ` → `31 มีนาคม 2564`. All three missing-date cases received invented specifics rather than abstention.
- `Thanks for your help. I really apprec` → `ite it.` Direct concatenation yields **apprecite**, not **appreciate**. Partial-word boundaries cannot be assumed correct with raw prefix tokenization.
- `I have attached the report for your` → ` review. I have attached the report for your review...`, repeating until the cap.
- `I don't know yet whether` → a personal story about attending the 2019 World Series of Poker and planning a Las Vegas trip.
- `เดี๋ยว update สถานะให้ใน Sla` → empty output. With a newline stop, emptiness is not proof of calibrated abstention.

Four unique cases hit the 32-token cap, repeated across all three runs: 12/72 warm requests. One unique case returned nothing: 3/72. Stopping at a newline or token cap does not guarantee a complete phrase or a valid Thai boundary.

## Controlled Qwen follow-ups

The baseline failure prompted two recorded follow-ups. Their corpus is the same diagnostic corpus, so these are **in-sample prompt investigations**, not independent held-out validation.

1. **Fixed few-shot suffix examples.** Added five examples, including Thai, mixed language, an incomplete English word, and an unknown-date blank. These were not exact evaluation prefixes. The configuration still used Ollama's stored template and `think:false`. It emitted the literal `/ no_think` for 16 of 24 cases (48/72 warm requests), plus other unusable fragments. There were no empty suggestions or length stops.
2. **Official non-thinking template control.** Kept the same few-shot system text and user input but rendered the simple system/user/assistant path from [Qwen's official tokenizer template](https://huggingface.co/Qwen/Qwen3-1.7B/blob/main/tokenizer_config.json), ending the assistant prefix with `<think>\n\n</think>\n\n`. Sent it using Ollama `raw:true`, bypassing the stored template. In contrast, the stored template appends `/no_think` to the user message. The literal marker leakage disappeared, but completion quality did not recover: `Please let me know if` → `ing.`, `I will check the details and get back to` → `ing you.`, and the partial `Sla` → `ing`.

The control demonstrates that template choice changes failure mode; it does **not** establish that the injected marker alone explains all baseline failures. It also does not prove that Qwen cannot perform this task with a different artifact, prompt, decoding policy, or fine-tuning. No production workaround was added and no further prompt search was performed.

## Decision and next experiment

**No-go for automatic display using these configurations.** Neither is ready for insertion into arbitrary apps. A suffix-only prompt is not sufficient enforcement; refusing duplication would still leave factual invention and malformed word boundaries.

**Best lead from this run: Typhoon base, evaluated further as an explicit-request continuation source.** Its warm latency is promising and it produced useful Thai/mixed suffixes without chat prompting. This is not a recommendation to ship it unchanged. The next bounded experiment should compare it against a larger Thai-tuned model on new held-out prefixes, including longer context and partial-word cases, with bilingual human ratings. Treat intended meaning, register, unsupported specifics, and output boundaries as separate criteria; do not optimize only latency or fluency.

No acceptance rate, writing-speed improvement, battery cost, or real UI responsiveness was measured. Those require actual interaction and cannot be inferred from this runner. Keep the existing n-gram/word completion unchanged until there is evidence of user benefit.

## Re-run

Requires a running local Ollama service. Models download only if not cached:

```sh
ollama pull qwen3:1.7b
ollama pull hf.co/mradermacher/llama3.2-typhoon2-1b-GGUF:Q4_K_M
python3 scripts/bench-sentence-completion.py --output /tmp/sentence-baseline.jsonl
python3 scripts/bench-sentence-completion.py --qwen-few-shot --output /tmp/sentence-few-shot.jsonl
python3 scripts/bench-sentence-completion.py --qwen-official-template --output /tmp/sentence-official-template.jsonl
```

Output files are created exclusively: choose new paths for subsequent runs rather than overwriting measured evidence. Model tags can change; compare the recorded digests. The runner's hardware metadata command is macOS-specific. It leaves the requested models downloaded, unloads each after its run, and makes generation requests only to `localhost:11434`.
