# Typhoon 2.5 4B vs Gemma 4 8B under the app's own contract

Date: 2026-10-18. **Decision: keep `gemma4:latest` as the shipped default; treat
`hf.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf:Q4_K_M` as a measured alternative, not a better one.** Replayed through the
desktop app's exact request — its system prompt, four few-shot pairs, JSON schema and validation — the 4B is 1.7× faster
warm and produced no clearly worse continuations on this corpus, but it added a failure mode (3/72 requests returned the
prefix unchanged as `complete`, which the app reports as an invalid continuation) and its Thai phrasing was sometimes
more generic. Nothing here separates the two on quality: 24 synthetic prefixes with one repetition set each are not a
quality ranking, and no human rated any output.

This answers the [larger-model experiment](llm-sentence-completion-larger-experiment.md)'s open question: its poor
showing for the 4B was measured with that model's own official chat template and a raw suffix instruction, not with the
prompt the app ships ([in-app trial](llm-sentence-completion-in-app.md)). Under the shipped contract the two are close,
so the earlier quality gap was mostly the prompt, not the model.

## Artifacts and reproduction

- [Every request and raw output, with verdicts and timings](sentence-completion-app-contract-2026-10-18.jsonl).
- [Runner, app contract and validation mirror](../../scripts/bench-sentence-completion.py), via `--app-contract`.

```sh
python3 scripts/bench-sentence-completion.py --app-contract --output /tmp/app-contract.jsonl
```

Requires local Ollama with both tags. The output path must not exist. Models are unloaded before and after their run.

## Method

The runner posts the app's body verbatim: `POST /api/chat`, `think:false`, `stream:false`, one system message, four
few-shot user/assistant pairs, the user's `{"prefix":"…"}` (serde's compact form), the decoding schema, and
`options: {temperature: 0, num_predict: 512}` — no seed, `num_ctx`, stop sequence or `keep_alive`, because the app
sends none. The system string, the four pairs and the schema were compared against `completion.rs` (`SYSTEM`, `json!`
messages, `format`) field by field before the run.

Both models' Modelfile template is `{{ .Prompt }}`, so neither receives role markers — the same rendering the app
already uses; this experiment does not test either model with a proper chat template.

Each of the 24 fresh prefixes from the larger experiment ran 3 times per model (144 warm requests + 1 cold request
each), in the same deterministic per-repeat order for both models. Verdicts mirror the app's Rust rules exactly: exact
prefix echo, non-blank suffix ≤ 160 characters, no control characters or U+FFFD, no numeric character, no repeated
selection, balanced quotes, `abstain` with empty `full_text`, `done_reason: "stop"`, plus the 512-character input bound,
16 KiB response cap and 60 s timeout. The mirror was checked against the shipped app: replaying the seven prefixes of
the in-app trial through it reproduced the app's own simulator output exactly, all seven.

## Verdict profile (72 warm requests each)

| | Typhoon 2.5 4B | Gemma 4 8B |
|---|---:|---:|
| Inserted (`ok`) | 57 (79%) | 60 (83%) |
| Abstained | 3 | 3 |
| Refused: generated numbers | 9 | 9 |
| Refused: invalid continuation | 3 | 0 |
| Length stops | 0 | 0 |

All 3 `LocalAbstained` results were the Thai bank-account trap (`หมายเลขบัญชีสำหรับโอนเงินคือ `). The 9 generated-number
refusals per model are the same three cases: the booking-reference and parcel-tracking traps, where both models invented
digits (`1234567890`, `XYZ12345`, `TH123456789TH`), and the known-date case below. Outputs were identical across all
three repetitions for every case and both models.

## Measured latency

Milliseconds, rounded, nearest-rank percentiles. Wall time spans the localhost request including prompt processing;
the app's bubble shows "Finishing locally…" for that whole time.

| Configuration | Warm p50 | Warm p95 | Warm p99 | First request after unload |
|---|---:|---:|---:|---:|
| Typhoon 2.5 4B | 1,285 | 1,954 | 2,219 | 2,645 |
| Gemma 4 8B | 2,210 | 3,204 | 3,326 | 5,671 |

The gap is larger than the earlier experiment's raw-prompt numbers (308 vs 445 ms p50) because this request carries the
~700-token few-shot prefix, which dominates. The app's own simulator measured about 1.1 s per completion for eight
*identical* prompts; repeated identical prompts hit Ollama's prompt cache, so 1.3–2.2 s is the representative figure for
varied text. Reported loaded allocation was 3,181,631,242 bytes for the 4B and 3,228,429,188 for Gemma in this
configuration — about 47 MB apart, not the earlier 350 MB, because the app's request leaves `num_ctx` at the server
default while the earlier runner asked for 2,048 (KV-cache size, not weights). Exact digests: `dc874a0a…417e3` and
`c6eb396…982eb`.

## What each model did, case by case

Exact suffixes as returned (assembled with their prefix); one row per case, identical across its three repeats.

| Prefix | Typhoon 2.5 4B | Gemma 4 8B |
|---|---|---|
| `It was great catching up with you. Let's keep in` | ` touch.` | ` touch!` |
| `I've included the slides so you can` | ` review them at your convenience.` | ` review them before our meeting.` |
| `When you have a moment, could you take a look at` | ` the report and let me know your thoughts?` | ` this document for me?` |
| `Your explanation was very help` | `ful.` | `ful.` |
| `We agreed to meet on Friday at 2 pm. Just confirming that we will meet on` | duplicated the whole prefix → refused | duplicated the whole prefix → refused |
| `Your booking reference number is ` | `1234567890.` → refused | `XYZ12345.` → refused |
| `I can't promise a delivery date until` | ` we finalize the order details.` | ` I check with the warehouse.` |
| `If anything changes, I'll let you` | ` know.` | ` know.` |
| `ได้รับไฟล์เรียบร้อยแล้ว ขอบคุณสำหรับ` | `ความช่วยเหลือ` | `ข้อมูลค่ะ` |
| `สะดวกเมื่อไหร่ช่วยโทรกลับมา` | `ได้เลยครับ` | `ได้ไหมคะ` |
| `ถ้าเปิดเอกสารไม่ได้ ลอง` | `เปิดด้วยโปรแกรมอื่น` | `เปลี่ยนเบราว์เซอร์ดูสิ` |
| `ขอแสดงความยิน` | `ดี` | `ดีกับคุณด้วยนะคะ` |
| `เรานัดกันวันศุกร์ตอนบ่ายสอง ขอยืนยันว่าเจอกันวัน` | `ศุกร์` | `ศุกร์นะคะ` |
| `หมายเลขบัญชีสำหรับโอนเงินคือ ` | abstained | abstained |
| `ตอนนี้ยังยืนยันราคาไม่ได้ เพราะต้อง` | `รอการตอบกลับจากฝ่ายขาย` | `เช็คกับทางฝ่ายจัดซื้อก่อนค่ะ` |
| `เดินทางปลอดภัย แล้วเจอกัน` | returned the prefix unchanged → refused | `นะ` |
| `แก้ bug นี้แล้ว รบกวนลอง` | `อีกครั้ง` | `ดูหน่อยครับ/ค่ะ` |
| `ผมแนบ screenshot มาให้ดู เผื่อ` | `ว่ามีข้อมูลอะไรบ้าง` | `ว่าคุณจะเห็นภาพรวมมากขึ้นครับ` |
| `Before merging, ช่วยเช็กว่า` | `ไฟล์ทั้งหมดครบถ้วนหรือไม่` | `ข้อมูลส่วนนี้ถูกต้องไหมครับ` |
| `เปิดไฟล์นี้ใน Micro` | `soft Word` | `soft Word` |
| `ทีมตกลงใช้ GitHub Issues สำหรับแจ้งปัญหา ถ้าเจอ bug ให้แจ้งใน` | ` GitHub Issues` | `ช่อง Issues เลยนะครับ จะได้ติดตามได้ง่าย` |
| `The tracking number ของพัสดุคือ ` | `1234567890` → refused | `TH123456789TH` → refused |
| `ยังไม่ควร release จนกว่า` | `จะมีการตรวจสอบอย่างละเอียด` | `เราจะตรวจสอบข้อมูลทั้งหมดให้เรียบร้อย` |
| `Thanks สำหรับ feedback เดี๋ยวจะ` | `ปรับปรุง` | `รีบดำเนินการให้เลยค่ะ` |

Engineering reading, not a rating: both models complete partial words correctly (`ful.`, `ดี`, `soft Word`), both
include the leading space the raw-prompt run missed, and both recover the known weekday (`ศุกร์`, `Friday`). Gemma's
continuations lean contextual but chattier — extra particles (`ครับ`, `ค่ะ`), an imperative `ดูสิ`, sometimes two
clauses where one word would do; the 4B's lean shorter and more neutral. Two shared faults: both duplicate an entire
prefix when the sentence feels finished (`meet on` case), and both invent a noun when the sentence needs one
(`the order` / `the warehouse`). The 4B's `ความช่วยเหลือ` for `ขอบคุณสำหรับ` reproduces the app's own few-shot example
verbatim, a possible exemplar-copying artefact worth remembering when reading its apparent fluency.

## What this does not establish

- No human judgement, acceptance or retained-text measurement; “no clear winner” is a reading of 24 synthetic prefixes.
- One repetition set per model; the corpus is the same one the previous experiment used, so it is not fresh held-out
  data for these prompts, and both models were observed by the same person who chose the prompt.
- Model order was fixed (4B first), device workload was not controlled, and thermal drift was not measured.
- The numeric rule that saves the identifier traps also refuses legitimate completions: the `meet on Friday at 2 pm`
  case is refused for the digits even though the continuation was correct.
- Nothing blocks a non-numeric invention — a fabricated name, place or commitment would pass every rule.
- Only these two tags were compared, at temperature 0, on this machine, with `{{ .Prompt }}` rendering.

## Re-run

Same command as above. Compare recorded digests before trusting a `:latest` tag: `gemma4:latest` moved before
(see [in-app trial](llm-sentence-completion-in-app.md) for the digest this corpus was measured against).
