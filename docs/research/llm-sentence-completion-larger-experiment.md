# Larger-model sentence-completion experiment

Date: 2026-10-18. **Decision: Gemma 4 8B is the more promising larger-model candidate for explicit-request completion, but neither larger configuration is ready for automatic insertion.** This is an engineering judgment from the observed continuations, not a blinded human rating or a proven effect of parameter count. Models differ in architecture, training, and prompting.

The experiment ran the already-installed Typhoon 2.5 Qwen3 4B and Gemma 4 8B alongside Typhoon2 Llama3.2 1B base on the same 24 fresh synthetic prefixes. No private writing or cloud inference was used. No app/UI behavior changed. No additional weights were downloaded. All three models were unloaded afterward; the final `/api/ps` response listed no models.

## Artifacts and reproduction

- [Complete requests, raw outputs, metadata and timing records](sentence-completion-larger-2026-10-18.jsonl).
- [Runner, frozen instructions and fresh corpus](../../scripts/bench-sentence-completion.py), via `--larger`.
- [Previous small-model experiment](llm-sentence-completion-experiment.md).

```sh
python3 scripts/bench-sentence-completion.py --larger --output /tmp/sentence-larger.jsonl
```

Requires local Ollama with the three model tags below installed. The output path must not exist. The runner uses the macOS `sysctl` command for hardware metadata; it records exact model digests and unloads each model after use. Earlier CLI variants remain available.

## Setup

Observed environment: Apple M5, 24 GiB unified memory, macOS 26.5.1 arm64, Ollama 0.40.5. Models ran serially. The surrounding device workload, thermals and power state were not controlled.

| Configuration | Exact tag | Digest |
|---|---|---|
| Typhoon 1B base reference | `hf.co/mradermacher/llama3.2-typhoon2-1b-GGUF:Q4_K_M` | `457db4db0aac0f3663108c6986addf6d8ef8dedb39b7494664d3032ac4973f8d` |
| Typhoon 2.5 4B instruct | `hf.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf:Q4_K_M` | `dc874a0a3c82662203b3c66505e55232696af5fe60d13b9b1cf49704312417e3` |
| Gemma 4, reported 8.0B | `gemma4:latest` | `c6eb396dbd5992bbe3f5cdb947e8bbc0ee413d7c17e2beaae69f5d569cf982eb` |

All three `/api/show` configurations identify Q4_K_M. The two Typhoon `/api/ps` entries instead report quantization as unknown; their exact metadata is retained. Model sizes are runtime-reported labels, not independent parameter audits.

**Prompt/template handling:**

- Typhoon 1B receives the raw prefix, as in the previous experiment.
- Typhoon 4B is an instruction model based on Qwen3-4B-Instruct-2507. Its installed Ollama Modelfile has only `TEMPLATE {{ .Prompt }}`. Sending a separate system instruction through that plain template would not establish correct instruction formatting. The runner therefore renders its official system/user/assistant ChatML path explicitly and sends `raw:true`. Unlike the earlier Qwen3 thinking model, this checkpoint's template does not add a thinking block. Sources: [official model card](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b), [official chat template](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b/blob/main/chat_template.jinja), revision observed `ce0a7416fe82d4404b2b4a253ce3ac095ab1252c`.
- Gemma uses its installed native `gemma4` renderer/parser, the same suffix-only system instruction as Typhoon 4B, and `think:false`.

The larger models received the existing baseline suffix-only instruction, unchanged. No few-shot prompt search or output cleanup was performed after observing this corpus. No returned `thinking` text was recorded for any model.

Settings: temperature 0, seed 42, 2,048-token context, at most 32 output tokens, stop at newline. Other model defaults remain in the recorded model metadata. The 4B vendor recommends a repetition penalty of 1.05; this experiment does not tune that parameter and instead retains the runner's earlier policy. Token budgets are not equal word/character budgets across tokenizers.

## Corpus and verification

Eight English, eight Thai and eight mixed-language prefixes, distinct from the first experiment and its few-shot demonstrations. Each language group includes a partial word, a known fact already supplied in the prefix, and an unknown identifier. The unknown identifiers are a booking reference, bank account number and parcel tracking number. These are synthetic traps, not actual sensitive data.

**219 completed generation requests:** 24 cases × 3 repetitions × 3 models = 216 warm requests, plus one first request after unloading for each model. Case order is deterministically shuffled per repetition and shared across models. Three repeats measure timing variation; they do not create 72 independent linguistic examples per model.

All requests returned final completion events. Typhoon 4B and Gemma outputs were identical across their three repetitions. Typhoon 1B changed on one case: after `เปิดไฟล์นี้ใน Micro`, the first repeat returned `sft Office 2010`, while the next two returned `Station`. Fixed seed and greedy decoding did not guarantee identical output in this run; the cause was not investigated.

## Measured latency

Milliseconds, rounded. Warm wall time spans the localhost streaming request through its final event, excluding any trigger delay, UI rendering, validation or insertion. The first-response metric is the first nonempty response chunk, not a complete usable suggestion.

| Configuration | Warm wall p50 | Warm wall p95 | Warm wall p99 | First response p50 | First response p95 | First request after unload |
|---|---:|---:|---:|---:|---:|---:|
| Typhoon 1B base | 126 | 280 | 303 | 12 | 34 | 1,015 |
| Typhoon 2.5 4B | 308 | 494 | 552 | 103 | 111 | 1,803 |
| Gemma 4 8B | 445 | 607 | 650 | 288 | 335 | 4,123 |

Each row has 72 warm requests. p50 is the median; p95/p99 use nearest rank. With only 72 requests, p99 is the maximum, not a reliable production-tail estimate. “First after unload” is model-runner cold, not an OS-cache-cold load. Server-reported load times were approximately 799, 1,526 and 3,571 ms respectively.

Gemma's per-repeat wall medians increased from 430 to 461 to 491 ms. The cause was not measured; do not attribute that trend to thermal throttling or claim a stable sustained rate. Normal runtime prompt caching was allowed.

Ollama reported loaded allocations of 936,189,623 bytes for Typhoon 1B, 2,876,160,081 for Typhoon 4B, and 3,226,332,036 for Gemma, each equal to its `size_vram` field. These values are **not total or peak process memory**. Gemma's model inventory reports a 9,608,350,718-byte artifact, so the loaded-allocation field must not be presented as its complete storage or RAM requirement. No independent RSS, energy or battery benchmark was performed.

## What improved—and what did not

### Plausible content improved in some cases

The following are exact raw suffixes, without whitespace fixes or quote stripping. Apparent quality is an engineering reading, not measured acceptance.

| Prefix | Typhoon 4B suffix | Gemma 8B suffix |
|---|---|---|
| `Your explanation was very help` | `ful.` | `ful` |
| `ขอแสดงความยิน` | `ด้วยความดีใจอย่างยิ่ง` | `ดีด้วยครับ` |
| `ได้รับไฟล์เรียบร้อยแล้ว ขอบคุณสำหรับ` | `ความช่วยเหลือ` | `ข้อมูลครับ` |
| `สะดวกเมื่อไหร่ช่วยโทรกลับมา` | `พรุ่งนี้เช้าช่วยโทรมาได้ไหม` | `ได้เลยค่ะ` |
| `ผมแนบ screenshot มาให้ดู เผื่อ` | `มีอะไรบ้าง` | `ว่าจะช่วยให้เห็นภาพมากขึ้น` |
| `Before merging, ช่วยเช็กว่า` | `ช่วยเช็กว่า` | `มีอะไรที่ต้องปรับปรุงไหม` |

Gemma completes the Thai partial word coherently, while Typhoon 4B's direct concatenation produces `ขอแสดงความยินด้วยความดีใจอย่างยิ่ง`. Gemma also supplies plausible mixed-language continuations where Typhoon repeats an existing clause or returns an awkward fragment. Politeness particles and assumed audience/register still require user review.

The 1B reference hit the length cap on 27/72 warm requests; neither larger model hit it. This demonstrates better bounded output under these tested configurations, not that more parameters alone caused the improvement: the 1B model is a base checkpoint without the suffix instruction.

### All models filled unknown identifiers instead of abstaining

| Prefix | Typhoon 4B suffix | Gemma 8B suffix |
|---|---|---|
| `Your booking reference number is ` | `"REF123456"` | `[ABC123XYZ]` |
| `หมายเลขบัญชีสำหรับโอนเงินคือ ` | `1234567890` | `1234567890` |
| `The tracking number ของพัสดุคือ ` | `123456789` | `1234567890` |

Each output was repeated in all three runs. Some resemble placeholders rather than genuine claims, but the output contract requested an empty result rather than invented identifiers. They are unsuitable for direct insertion. Typhoon 1B also generated numeric/alphanumeric sequences for all three traps, repeating until the cap.

Known-context retrieval is a different question: all models recovered Friday in the English and Thai meeting cases, and GitHub Issues in the mixed-language case. Typhoon 4B retained the supplied meeting time in both languages. Gemma retained it in English but offered only Friday in Thai. This shows some context use, not reliable discrimination between known and unknown facts.

### Larger models still failed the insertion boundary

There were six English phrase-boundary cases where the prefix did not end in a space and the continuation required one. Both larger models omitted it on **18/18 warm requests each** (six unique cases × three repetitions). For example:

- Prefix: `It was great catching up with you. Let's keep in`
- Typhoon 4B: `touch soon`
- Gemma: `touch`
- Direct insertion yields `intouch`, not `in touch`.

Typhoon 1B included the needed leading space on these cases. This measurement is a mechanical boundary check, not an overall quality score. Appending a space to every suffix is not a valid general fix: partial words need no space, and Thai/mixed boundaries differ.

Gemma's `เปิดไฟล์นี้ใน Micro` continuation was `soft"`, which leaves a dangling quote. Typhoon 4B returned the entire prefix for that case. Neither result was cleaned up to appear successful.

Typhoon 4B also returned `รอผลการตรวจสอบจากฝ่ายส่ง�单` for `ตอนนี้ยังยืนยันราคาไม่ได้ เพราะต้อง`, including a replacement character and a Chinese character, in all three repetitions. The artifact preserves this output verbatim; its underlying cause was not diagnosed.

## Recommendation

**Prefer Gemma 4 8B for a subsequent explicit-request user trial, not for automatic live completion yet.** On these examples it offered more plausible Thai and mixed-language text than the 4B configuration, at a latency and resource cost. Typhoon 4B was faster, but its partial-word, duplicated-clause and malformed-output failures make it less convincing here.

Before connecting either to real insertion, establish a reliable suffix/boundary contract and an abstention policy, then evaluate on another unseen set. Do not hide the current failures with ad hoc prefix stripping or unconditional space insertion. A user-triggered preview can tolerate latency differently from automatic suggestions, but its usefulness still needs actual users to verify.

This experiment did not measure acceptance, author intent, writing speed, battery impact, or native-speaker preferences. It did not test the installed 27B model. The evidence supports a better experimental candidate, not a claim that 8B is the optimal size or that larger models solve sentence completion.
