# Local sentence completion in the app

Date: 2026-10-18. **Decision: ship the explicit-request preset behind the existing AI-rewrite switch, not automatic
suggestions.** The ⌃⌥R bubble offers *Finish sentence locally*; it calls the local Ollama model the
[larger-model experiment](llm-sentence-completion-larger-experiment.md) picked (`gemma4:latest`) and pastes the result
only when the user picks the row and confirms. This note records a functional smoke test of that path with synthetic
selections. It measures nothing about usefulness, acceptance or writing speed; no real writer was involved.

## What the app enforces

The contract is enforced in code (`rust/crates/typethai-desktop/src/completion.rs`), not by trusting the model:

| Rule | Behaviour when the model breaks it |
|---|---|
| One JSON object `{decision, full_text}` (Ollama `format` schema, so the server constrains decoding) | `LocalInvalid`: nothing is inserted |
| `full_text` must start with the exact selection, byte for byte | `LocalInvalid` (no prefix repair, no whitespace trimming) |
| The appended suffix: non-blank, ≤ 160 characters | `LocalInvalid` |
| No control characters or U+FFFD in the suffix | `LocalInvalid` |
| No numeric characters in the suffix (ASCII, Thai `๔๒`, full-width, `½`, `Ⅳ` — unit-tested) | `LocalNumbers` |
| Suffix must not repeat the selection or leave an unbalanced `"`/`` ` ``/`“”` pair | `LocalInvalid` |
| `decision: abstain` must carry an empty `full_text` | `LocalAbstained` → the row says "The local model has no suggestion for this selection" |
| Ollama's envelope: `done: true`, `done_reason: "stop"`, a non-empty `message.content` | `LocalTruncated` / `LocalInvalid` |

Request shape: `POST http://127.0.0.1:11434/api/chat`, `think:false`, `stream:false`, `options: {temperature: 0,
num_predict: 512}`, a 60 s timeout, at most 512 input characters and a 16 KiB response cap. The agent disables proxies
and redirects (`proxy(None).max_redirects(0)`), so the selection cannot be routed off the machine. The prompt is one
system instruction plus four few-shot pairs (English phrase, partial English word, Thai, and an abstention). Cloud
configuration — API key, base URL, model, Instructions — is ignored by this preset, and a failure never falls back to
a cloud call. The request runs on the existing rewrite worker thread while the bubble shows "Finishing locally…"; a
reply lands only while the bubble still holds the matching request id, so Esc or typing during a request cannot insert
anything.

## How it was exercised

The headless simulator runs the same `completion::run` the worker thread calls, so the request, prompt, parsing and
validation are the shipped ones; selection capture and the clipboard paste are simulated. Transcript:
[sentence-completion-app-2026-10-18.log](sentence-completion-app-2026-10-18.log).

```sh
printf '<SEL:Please let me>^R<TabDown>3<TabUp>\n' | TYPETHAI_MODEL=/tmp/ttmodel/typethai.ttm \
  rust/target/debug/typethai-desktop --simulate --state /tmp/ttsim/personal.json
```

`<SEL:…>` is the selection, `^R` is ⌃⌥R, `<TabDown>3<TabUp>` picks row 3 (*Finish sentence locally*). The last two
lines of the transcript cover Tab confirming the result row and Esc cancelling at the result.

## Observed

The notice wording above is the shipped one; the model became selectable later the same day (`rewrite::Config::local_model`),
so the transcript beside this note records the strings as they were at the time of the run.

Local `gemma4:latest` (digest `c6eb396…982eb`), warm, one repetition per prefix; outputs were identical on a second
run, and repeated prefixes repeated exactly (temperature 0).

| Selection | Bubble after the request |
|---|---|
| `Please let me` | `Please let me know if you need any further assistance.` |
| `Micro` | `Microscope is a powerful tool for viewing tiny objects.` |
| `ขอแสดงความยิน` | `ขอแสดงความยินดีกับคุณด้วยนะคะ` |
| `Before merging, ช่วยเช็กว่า` | `Before merging, ช่วยเช็กว่าข้อมูลส่วนนี้ถูกต้องไหมครับ` |
| `It was great catching up with you. Let's keep in` | `It was great catching up with you. Let's keep in touch!` |
| `My bank account number is` | "The local model has no suggestion for this selection" (nothing inserted) |
| `The tracking number for my order is` | same abstention notice |
| any of the above, Ollama unreachable (network denied to the process) | "Ollama is not running. Start it, or press Set up in Settings → AI rewrite" (nothing inserted) |

Confirmation behaves as documented for the other presets: Tab on the result row pastes the full text over the
selection; Esc at the result closes the bubble and changes nothing.

Two differences from the earlier raw-prompt experiments are worth noting, and both come from the prompt and schema,
not from a different model: the missing-space insertion boundary is gone on the one case that failed before (`…keep in`
+ ` touch!`), and the two identifier traps that all three tested models filled with invented numbers now abstain.
These are two specific traps, not evidence of calibrated abstention.

Warm latency through this path: eight repetitions of one prefix took 17.95 s of simulator wall time against 9.17 s for
eight identical lines with no completion, i.e. about **1.1 s per completion**. Repeated identical prompts hit Ollama's
prompt cache; the [app-contract comparison](llm-sentence-completion-app-contract.md), which varies the prefix, measured
1,285 ms p50 for the 4B and 2,210 ms for `gemma4:latest`, and 2,645 ms / 5,671 ms for the first request after the model
was unloaded. The simulator blocks on the call, unlike the app's worker thread; none of these is a UI-responsiveness or
battery measurement.

## What this does not establish

- No human rating, no acceptance, no retained-text or task-time measurement. The continuations above are plausible
  English/Thai/mixed text, not text a writer asked for; `Microscope is a powerful tool…` is the kind of generic
  completion a real user may reject.
- One synthetic prefix per case, one model, one machine. The abstention result covers two phrasing variants of two
  traps; other unknowns (names, addresses, amounts, dates) are untested here. The larger experiment found every
  tested model inventing identifiers when asked with a raw suffix prompt.
- The simulator cannot exercise the real key capture, the Accessibility/clipboard selection read, the paste into
  another app, Windows, or a second request racing a long one.
- The 1.1 s figure blocks the simulator; whether the bubble's "Finishing locally…" state feels acceptable with a real
  selection and a cold model (4.1 s first load in the larger experiment) was not measured.

## Follow-up

Keep this preset explicit-request only until a trial with real writers shows retained-text benefit without its
meaning, privacy or attention cost, as the [desk research](llm-sentence-completion.md) proposes. The next measurable
step is a counterbalanced comparison on real selections — existing rows only, versus this preset — recording accepted
and kept text, undo/revision rate and how often the result is visibly wrong. The model choice itself was re-tested
under this exact request in the [app-contract comparison](llm-sentence-completion-app-contract.md), which kept
`gemma4:latest` as the default and recorded the 4B as a faster alternative.

## Re-run

Needs a local Ollama with `gemma4:latest` (`ollama pull gemma4:latest`). The command above prints the same request,
validation and result path; add `sandbox-exec -p '(version 1)(allow default)(deny network*)'` before the binary to
reproduce the unreachable-server notice.
