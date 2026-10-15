# LLM rewrite of the selected text: research for typeThai

Date: 2026-10-15
Scope: add an option that sends the text the user has selected (in any app) to an LLM, then puts the result in place of the selection, on the macOS and Windows desktop app (`rust/crates/typethai-desktop`).
Method: primary sources only (Apple, Microsoft, vendor API docs, model cards, tech reports, crate docs and source). Anything not traced to a primary source is marked **unverified**. Numbers marked **derived** or **estimate** are this document's own arithmetic on cited figures, not vendor claims.

## TL;DR

- **Capture the selection** with the APIs typeThai already uses: AX `AXSelectedText` on macOS, UIA `TextPattern.GetSelection` on Windows. If that returns nothing, fall back to a synthesized Cmd/Ctrl+C. Save the clipboard first, detect the copy with `NSPasteboard.changeCount` or `GetClipboardSequenceNumber`, then restore the clipboard. Never capture from password fields (`AXSecureTextField`, UIA `IsPassword`).
- **Replace the selection by pasting** (synthesized Cmd/Ctrl+V of our own clipboard contents, then restore the clipboard). Writing `AXSelectedText` directly can work in native Cocoa text views, but Apple documents no guarantee that it takes effect, and UIA has no API that replaces a substring (`ValuePattern.SetValue` replaces the whole value). Pasting also leaves the target app's own undo working. That undo behaviour is **unverified** per app.
- **Use one provider abstraction on the OpenAI-compatible Chat Completions wire format.** It covers Ollama (`http://localhost:11434/v1/`), llama.cpp `llama-server` (default `127.0.0.1:9931`), OpenAI, and Gemini (`.../v1beta/openai/`). Add one native Anthropic Messages adapter, because Anthropic calls its OpenAI-compatible layer "not … production-ready". The best cloud default for short rewrites is `claude-haiku-5-5`, listed as "Fastest" at $0.10 / $0.50 per MTok for prompts up to 100k tokens. Run it at `effort: "low"` with thinking disabled and streaming on.
- **On-device OS models do not help with Thai today.** Thai is not on Apple Intelligence's language list, so Foundation Models (macOS 26+, Swift-only, ~3B parameters) would reject Thai input. Microsoft does not document which languages Windows Phi Silica supports. Phi Silica is a Limited Access Feature that needs package identity, and Aion Instruct replaces it from January 2027.
- **Local Thai models:** the smallest tier that looks usable is a **Thai-tuned 3–4B model**. Thai-tuned means Typhoon2-Llama3.2-3B, Typhoon2.5-Qwen3-4B or Typhoon2.1-Gemma3-4B; general models of that size often slip into other languages when asked to answer in Thai. At 1B, Thai instruction-following and MT-Bench scores are poor. **8 GB machine:** Typhoon2.5-Qwen3-4B at Q4_K_M, a 2.50 GB file. **16 GB machine:** Typhoon2.1-Gemma3-12B at Q4_K_M, a 7.30 GB file. The Gemma tokenizer uses about 40% fewer tokens for Thai than the Qwen tokenizer in a local measurement, and fewer tokens means less time to generate the same text.
- **Thai has no spaces between words.** Compute the preview diff on grapheme clusters or with a Thai word segmenter (`icu_segmenter::WordSegmenter` supports Thai), not by splitting on whitespace. The prompt must also say explicitly which polite particle to use (ครับ for a male speaker, ค่ะ for a female speaker).
- **Recommended UX:** a new **Ctrl+Alt+R** chord (R is unused by the current Ctrl+Alt hotkeys Z/X/G/1/2/3/P) opens a preset list in the existing caret bubble. The presets are Fix Thai typos, Make polite (ครับ/ค่ะ), and Translate TH↔EN. The rewritten text streams into the bubble as a preview, Tab or Ctrl+Alt+1 accepts it and pastes it, and Esc cancels. Before typeThai sends any text to a cloud provider, the user must have opted in; the API key lives in the Keychain or Credential Manager through the `keyring` crate.

---

## 0. What typeThai already has (fit with the existing stack)

From `rust/crates/typethai-desktop/Cargo.toml` and `src/platform/*.rs`:

- **macOS** (`platform/macos.rs`) already uses:
  - a `CGEventTap` on the session, with key and mouse events (`core-graphics 0.25`);
  - `AXIsProcessTrustedWithOptions` with the prompt option;
  - `AXUIElementCreateSystemWide` and `AXFocusedUIElement`, with the frontmost app's focused element as a fallback;
  - `AXSelectedTextRange` and `AXBoundsForRange` to find the caret;
  - `AXUIElementSetAttributeValue`, used to set `AXManualAccessibility` / `AXEnhancedUserInterface` so Electron and Chromium build their AX trees;
  - `AXUIElementSetMessagingTimeout(0.25)`;
  - a secure-input skip (`IsSecureEventInputEnabled` or subrole `AXSecureTextField`);
  - synthetic typing through `CGEventKeyboardSetUnicodeString`, tagged with a `MAGIC` user-data value so its own events can be ignored;
  - `objc2-app-kit 0.3` with features for NSPanel, NSWindow, NSWorkspace and others. The `NSPasteboard` feature is **not** enabled yet.
- **Windows** (`platform/windows.rs`) already uses:
  - `WH_KEYBOARD_LL` and `WH_MOUSE_LL` hooks;
  - `SendInput` with Unicode input, and it already releases held Ctrl+Alt before injecting;
  - a dedicated MTA thread for UI Automation, which calls `GetFocusedElement`, `CurrentIsPassword`, and `IUIAutomationTextPattern2::GetCaretRange`;
  - an `ES_PASSWORD` check;
  - the `windows 0.62` crate with features including `Win32_UI_Accessibility`, `Win32_System_Com`, `Win32_System_Ole` and `Win32_UI_Input_KeyboardAndMouse`. The clipboard module (`Win32_System_DataExchange`) is **not** enabled yet.
- **Hotkeys** are Ctrl+Alt+letter (`app.rs::hotkey_for`): Z (Undo), X (ForceConvert), G (Guess), 1/2/3 (Accept 0..2) and P (TogglePause).
- The **bubble** is a gpui-kit non-activating panel that sits at the caret. Tab, Shift+Up/Down and Esc control it while it is visible.
- The app has **no HTTP client, no clipboard code, and no secret storage** yet.

So the rewrite feature mostly reuses the existing AX/UIA plumbing. The new dependencies are an HTTP client, a clipboard save/restore module per OS, and `keyring`.

---

## 1. Capturing the selected text system-wide

### 1.1 macOS: Accessibility first

- **The attributes.** `kAXSelectedTextAttribute` is "The currently selected text within this accessibility object. This attribute is required for all accessibility objects that represent editable text elements" ([Apple](https://developer.apple.com/documentation/applicationservices/kaxselectedtextattribute)). `kAXSelectedTextRangeAttribute` "Indicates the range of characters (not bytes) that defines the currently selected text … required for all accessibility objects that represent editable text elements" ([Apple](https://developer.apple.com/documentation/applicationservices/kaxselectedtextrangeattribute)). Both are documented as required **only for editable text elements**. Selections in read-only content, such as web pages and PDFs, are therefore not guaranteed to be exposed. That conclusion is **derived** from the wording; Apple does not list the cases.
- **Focused element.** Use the system-wide element's `AXFocusedUIElement`, falling back to the frontmost app's element. `focused_element()` in `macos.rs` already does this. Read the attribute with `AXUIElementCopyAttributeValue`, which returns error codes on failure ([Apple](https://developer.apple.com/documentation/applicationservices/1462085-axuielementcopyattributevalue)).
- **Permission.** The process must be a trusted accessibility client. `AXIsProcessTrustedWithOptions` "Returns whether the current process is a trusted accessibility client" ([Apple](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions)). typeThai already asks for this permission at launch.
- **Electron and Chromium.** Electron enables accessibility automatically only "in the presence of assistive technology". Third-party tools can turn it on by setting `AXManualAccessibility` with `AXUIElementSetAttributeValue` ([Electron docs](https://www.electronjs.org/docs/latest/tutorial/accessibility)). typeThai already does this, and also sets `AXEnhancedUserInterface` on Chromium browsers. Chromium's handling of `AXEnhancedUserInterface` is **unverified** against a Chromium primary source; it is taken from the existing code.
- **Apps that do not expose the selection.**
  - Grammarly, whose desktop app has the same problem, says that on Mac "the textual content must be made visible to accessibility", that Grammarly "doesn't initialize in input fields" in some web and Electron cases, and that iframes "require custom integration" ([Grammarly support](https://support.grammarly.com/hc/en-us/articles/10139846131213-How-do-I-integrate-Grammarly-with-my-website-or-application)).
  - Specific coverage in terminals (Terminal.app, iTerm2), browsers' web content, and canvas-rendered editors such as Google Docs is **unverified**. Expect gaps and rely on the clipboard fallback.
- **Clipboard fallback.**
  1. Snapshot the general pasteboard's items and types.
  2. Record `changeCount`.
  3. Post a Cmd+C key-down/key-up pair through `CGEvent` with **only** the Command flag set. The user is still holding Ctrl+Alt from the hotkey, so the event flags must be set explicitly. `inject()` on Windows already does the equivalent.
  4. Poll `changeCount` until it increases or a timeout passes.
  5. Read the string, then restore the snapshot.

  Apple documents that the change count "increments each time the pasteboard ownership changes", and that you can "record the value … and compare it with a later value" ([Apple](https://developer.apple.com/documentation/appkit/nspasteboard/changecount)).
  - The timeout value is a tuning choice; a sensible starting point is 150–300 ms, which is **unverified**.
  - If `changeCount` does not change, nothing was selected, or the app ignored Cmd+C. Do **not** fall back to stale clipboard contents.
  - Snapshotting can be lossy or expensive with lazily-provided (promised) pasteboard data. That is **unverified**, so copy only the concrete types that are present.
  - Rust: `NSPasteboard` is in `objc2-app-kit` behind the `NSPasteboard` feature ([docs.rs](https://docs.rs/objc2-app-kit/latest/objc2_app_kit/struct.NSPasteboard.html)).
  - Clipboard-manager etiquette: when typeThai writes temporary content, add the marker type `org.nspasteboard.TransientType`. That type means "the pasteboard will either be restored to previous content, or the current content will be replaced within seconds" and such data "should not be recorded". `org.nspasteboard.ConcealedType` marks confidential content ([nspasteboard.org](http://nspasteboard.org/)). This is a voluntary community convention, not an Apple API.
- **macOS Services as an alternative entry point.** An app can advertise a Services menu item through `NSServices` in Info.plist ([Apple](https://developer.apple.com/documentation/bundleresources/information-property-list/nsservices)). An entry can declare `NSReturnTypes`, "the data types that the service returns" ([Apple](https://developer.apple.com/documentation/bundleresources/information-property-list/nsservices/nsreturntypes)), and an `NSKeyEquivalent`, "a keyboard shortcut that invokes the service menu command" ([Apple](https://developer.apple.com/documentation/bundleresources/information-property-list/nsservices/nskeyequivalent)). The service method receives the selection on an `NSPasteboard` and "writes any results back to the pasteboard". The provider is registered with `NSApp.servicesProvider`, and "service requests can arrive immediately after you register" ([Services Implementation Guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/SysServices/Articles/providing.html), [servicesProvider](https://developer.apple.com/documentation/appkit/nsapplication/servicesprovider)).
  - Pros: the host app hands over the selection and replaces it itself, with no clipboard trickery and correct undo. That benefit is **unverified** per app.
  - Cons: it only works in Cocoa apps that implement the Services protocol for their text views; Electron and many cross-platform apps are **unverified**. It needs the Info.plist entry, and typeThai already ships `packaging/Info.plist`. A service is also a synchronous round trip, so streaming into the caret bubble is awkward.
  - Treat Services as an optional second entry point, not the main path.

### 1.2 Windows: UI Automation first

- **`IUIAutomationTextPattern::GetSelection`** "Retrieves a collection of text ranges that represents the currently selected text in a text-based control" ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationtextpattern-getselection)):
  - Non-contiguous spans return one range each.
  - A bare insertion point returns "a degenerate (empty) text range".
  - The result is `NULL` if the control "does not contain a text insertion point or does not support text selection".
  - The docs recommend checking `SupportedTextSelection` first.

  Call `GetText(-1)` on each range. Run all of this on typeThai's existing UIA worker thread, which uses the focused element. `IUIAutomationTextPattern2` derives from TextPattern, so the existing `GetCurrentPatternAs(UIA_TextPattern2Id)` call can reuse `GetSelection`; that is **derived** from the interface naming.
- **Coverage.** Grammarly's Windows guidance for native apps is that text fields must "properly support UI Automation Text Pattern", and that Electron or web content must not have a zero-size parent in the accessibility tree ([Grammarly support](https://support.grammarly.com/hc/en-us/articles/10139846131213-How-do-I-integrate-Grammarly-with-my-website-or-application)). Which apps expose TextPattern is **unverified** app by app; expect Win32 Edit/RichEdit, WPF, WinUI and Chromium content to vary.
- **Clipboard fallback.** The steps are: save the clipboard, record `GetClipboardSequenceNumber`, `SendInput` Ctrl+C, wait for the sequence number to change, read `CF_UNICODETEXT`, then restore.
  - The sequence number "is incremented whenever the contents of the clipboard change or the clipboard is emptied", but "if clipboard rendering is delayed, the sequence number is not incremented until the changes are rendered" ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboardsequencenumber)).
  - `SendInput` "does not reset the keyboard's current state", so held Ctrl+Alt must be released first; typeThai already does this. It is also "subject to UIPI", and failures caused by UIPI are not reported by the return value or `GetLastError` ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)). As a result, elevated (admin) target windows silently get nothing.
  - Rust: `OpenClipboard` and related functions live in `windows::Win32::System::DataExchange` ([windows-rs 0.62.2 docs](https://microsoft.github.io/windows-docs-rs/doc/windows/Win32/System/DataExchange/fn.OpenClipboard.html)). The Cargo feature name `Win32_System_DataExchange` follows the crate's module-to-feature convention; that name is **derived**.
- **Clipboard history and cloud clipboard.** Windows recognizes three registered formats ([Microsoft, Clipboard Formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats)):
  - `ExcludeClipboardContentFromMonitorProcessing`: "Place any data … in this format to prevent all clipboard formats being included in the clipboard history or synchronized to the user's other devices."
  - `CanIncludeInClipboardHistory`: a DWORD 0 excludes the item from history.
  - `CanUploadToCloudClipboard`: a DWORD 0 prevents syncing.

  typeThai should add `ExcludeClipboardContentFromMonitorProcessing` to **its own** writes: the result it pastes, and the restore. The **Ctrl+C performed by the target app** writes the user's selection without that marker, so the captured text can land in Win+V history and cloud sync. That conclusion is **derived**: the formats only work when the app that places the data includes them. This is another reason to prefer UIA capture.

### 1.3 Never send password fields

- **macOS:** skip when the focused element's subrole is `AXSecureTextField` or when secure event input is on. typeThai already does both (`secure_now()`).
- **Windows:** `CurrentIsPassword` "Indicates whether the element contains a disguised password" ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationelement-get_currentispassword)). typeThai already checks it together with the `ES_PASSWORD` style.
- Run this check **before** any capture, including the clipboard fallback. Fail closed: if the check times out, do not send.

---

## 2. Replacing the selection

- **AX set.** `AXUIElementSetAttributeValue` returns errors such as `kAXErrorAttributeUnsupported`, `kAXErrorCannotComplete` and `kAXErrorNotImplemented` ([Apple](https://developer.apple.com/documentation/applicationservices/1460434-axuielementsetattributevalue)). `AXUIElementIsAttributeSettable` can be checked first ([Apple](https://developer.apple.com/documentation/applicationservices/1459972-axuielementisattributesettable)). Apple does not say whether a success return guarantees that the text changed. The common claim that a set "silently fails" (returns success with no change) in Electron and web views is **unverified**; this research found no primary source for it. If typeThai uses AX set, it should **verify** the result by re-reading `AXSelectedTextRange` and the value afterwards, and fall back to paste on any mismatch. Whether an AX set is registered with the target's undo manager is also **unverified**.
- **UIA.** `IUIAutomationValuePattern::SetValue` requires `IsEnabled` and `!IsReadOnly` ([Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomationvaluepattern-setvalue)). `IValueProvider` "does not support the retrieval of formatting information or substring values" ([Value control pattern](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-implementingvalue)). SetValue therefore replaces the **whole** field, which loses formatting, the rest of the text and undo granularity. **Do not use it for selection replacement.** TextPattern is read-only for text content, so it has no "replace range" call.
- **Paste (recommended for both OSes).**
  1. Write the result to the clipboard with the "don't record" markers from §1.
  2. Synthesize Cmd+V or Ctrl+V.
  3. Wait a short time, then restore the previous clipboard.

  Restoring too early can paste the old contents in slow apps. The wait is a tuning value and is **unverified**; PopClip's own `restorePasteboard` option shows this pattern is common practice ([PopClip developer docs](https://www.popclip.app/dev/all.md)). Paste goes through the app's normal edit path, so the app's own Cmd+Z / Ctrl+Z reverts it. That is the expected behaviour in standard text views but **unverified** for every app.
- **Fallback for no-paste fields.** typeThai's existing Unicode typing path (`inject()`: `CGEventKeyboardSetUnicodeString` in chunks of 20 or fewer UTF-16 units, or `SendInput` Unicode) can type the result over the selection. It is slower for long text and creates many undo steps.
- **typeThai's own undo.** Keep the original text. Ctrl+Alt+Z can offer "restore original" only while focus and caret have not moved; it would re-select the text with Shift+Left×n, which is fragile with combining characters. The primary undo should be the app's own Cmd+Z / Ctrl+Z.

---

## 3. LLM providers

### 3.1 Anthropic Claude API (verified 2026-10-15)

Current models ([Models overview](https://platform.claude.com/docs/en/models/overview)):

| Model ID | Comparative latency | Price per MTok (input / output) |
|---|---|---|
| `claude-fable-5-1` | Slower | $10 / $50 |
| `claude-opus-5-5` | Moderate | $4 / $20 |
| `claude-sonnet-5-5` | Fast | $2 / $10 |
| `claude-haiku-5-5` | Fastest | from $0.10 / $0.50 |

All four have a 1M-token context and 128K max output. Haiku 5.5 is described as "for high-volume, latency-sensitive tasks". Its tiered pricing is $0.10 input and $0.50 output for prompts up to 100,000 tokens ([Haiku 5.5 overview](https://platform.claude.com/docs/en/models/haiku-5-5/overview)).

- **Low latency.**
  - Set `output_config.effort` to `"low"`, which is "Most efficient … best speed and lowest costs".
  - On Haiku 5.5 you can also send `thinking: {"type": "disabled"}` "at `high` effort or below" ([Effort](https://platform.claude.com/docs/en/build-with-claude/effort)).
  - Omit `temperature`, `top_p` and `top_k` on Haiku 5.5 ([Haiku 5.5 overview](https://platform.claude.com/docs/en/models/haiku-5-5/overview)).
  - Haiku 5.5 uses a newer tokenizer that counts "approximately 30% more tokens" than Haiku 4.5 for the same text (same page).
- **Streaming.** Set `"stream": true` on `POST /v1/messages` to receive server-sent events ([Streaming](https://platform.claude.com/docs/en/build-with-claude/streaming)).
- **Rate limits.** In the Start tier, Haiku 5.5 allows 1,000 RPM, 2,000,000 ITPM and 400,000 OTPM. Limits use a token bucket, and the Start tier has a $500 monthly spend cap ([Rate limits](https://platform.claude.com/docs/en/api/rate-limits)).
- **OpenAI compatibility.** `https://api.anthropic.com/v1/` works with OpenAI SDKs, but Anthropic says it "is not considered a long-term or production-ready solution". Under it, prompt caching is unsupported and most unknown fields are "silently ignored" ([OpenAI SDK compatibility](https://platform.claude.com/docs/en/api/openai-sdk)). **Implement Anthropic natively.**
- **Thai.** Anthropic's multilingual table does not include Thai; it lists 14 other languages relative to English. It also states "Claude is capable in many languages beyond those benchmarked" ([Multilingual support](https://platform.claude.com/docs/en/build-with-claude/multilingual-support)). Thai quality is therefore **not benchmarked by Anthropic**. The same page recommends stating the target language explicitly in the system prompt.

### 3.2 OpenAI (verified 2026-10-15)

| Model ID | Price per MTok (input / output) | Notes |
|---|---|---|
| `gpt-6-astra` | $10 / $50 | |
| `gpt-6.1-sol` | $2 / $10 | |
| `gpt-6-luna` | $0.10 / $0.50 | "most efficient model for focused, high-volume tasks"; supports reasoning effort `none` |

Sources: [OpenAI models](https://developers.openai.com/api/docs/models) and [gpt-6-luna](https://developers.openai.com/api/docs/models/gpt-6-luna). Chat Completions is `POST https://api.openai.com/v1/chat/completions`, and `stream: true` returns server-sent events. OpenAI recommends the Responses API for new projects ([API reference](https://developers.openai.com/api/reference/resources/chat)). Chat Completions is still the lowest common denominator across providers. OpenAI publishes no Thai-specific benchmark in these sources (**unverified**).

### 3.3 Google Gemini (verified 2026-10-15)

| Model ID | Status | Paid price per MTok (input / output) |
|---|---|---|
| `gemini-3.8-flash` | Stable | $0.75 / $3.75 through 2026-12-31 |
| `gemini-3.5-flash-lite` | Stable | $0.30 / $2.50 |
| `gemini-3.1-flash-lite` | Stable | $0.25 / $1.50 |

Sources: [Gemini models](https://ai.google.dev/gemini-api/docs/models) and [Gemini pricing](https://ai.google.dev/gemini-api/docs/pricing). On the **free tier, content is used to improve Google products**; on the paid tier it is not. This matters for user text. The OpenAI-compatible base URL is `https://generativelanguage.googleapis.com/v1beta/openai/` and it supports streaming ([Gemini OpenAI compatibility](https://ai.google.dev/gemini-api/docs/openai)).

### 3.4 Local runtimes

- **Ollama.**
  - The OpenAI-compatible base is `http://localhost:11434/v1/`, with `/v1/chat/completions` (streaming) and `/v1/models`. The API key is "required but ignored" ([Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility)).
  - The native endpoint is `POST /api/chat`, which streams NDJSON by default and reports `load_duration`, `prompt_eval_duration`, `eval_count` and `eval_duration` ([Ollama /api/chat](https://docs.ollama.com/api/chat)).
  - It binds `127.0.0.1:11434` by default. Models stay loaded for 5 minutes by default, and an empty request preloads a model ([Ollama FAQ](https://docs.ollama.com/faq)). Preloading on hotkey-down hides cold-load latency.
  - Any Hub GGUF can be run with `ollama run hf.co/{user}/{repo}:{quant}`; Q4_K_M is the default when it exists ([Hugging Face docs](https://huggingface.co/docs/hub/ollama)). This is how Typhoon GGUFs run under Ollama.
- **llama.cpp `llama-server`.**
  - Exposes OpenAI-compatible `/v1/chat/completions` and `/v1/models`, synchronous or streaming.
  - Has `--api-key`, and Jinja chat templates are on by default.
  - **Defaults to `127.0.0.1:9931`, not 8080** ([server README](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)).
  - Apple silicon is "a first-class citizen" through Metal ([llama.cpp README](https://github.com/ggml-org/llama.cpp/blob/master/README.md)).

### 3.5 Apple Foundation Models (on-device)

- `LanguageModelSession` and `SystemLanguageModel` are available on macOS 26.0+ ([Apple](https://developer.apple.com/documentation/foundationmodels/languagemodelsession)). Apple's documentation for them has only a **Swift** variant; the AX APIs, by contrast, have both Objective-C and Swift. That observation comes from the docs' language metadata.
- The on-device model is "a ∼3B-parameter on-device model" with "2-bit quantization-aware training", and the framework is "Swift-centric" ([Apple ML Research, 2025 tech report](https://machinelearning.apple.com/research/apple-foundation-models-tech-report-2025)). Apple lists the on-device context as 4K tokens, against 32K for the Private Cloud Compute model. PCC requires a managed entitlement and has a daily limit ([Apple](https://developer.apple.com/documentation/foundationmodels/adding-server-side-intelligence-with-private-cloud-compute)).
- **Thai: not supported.** The model "understands and generates text in any language that Apple Intelligence supports", and unsupported input throws `unsupportedLanguageOrLocale` ([Apple](https://developer.apple.com/documentation/foundationmodels/supporting-languages-and-locales-with-foundation-models)). The Apple Intelligence language list is English, Danish, Dutch, French, German, Italian, Norwegian, Portuguese, Spanish, Swedish, Turkish, Vietnamese, Chinese (simplified and traditional), Japanese and Korean. **Thai is absent** ([Apple Support 121115](https://support.apple.com/en-us/121115), published 2026-09-14).
- **Calling it from Rust** would need a Swift shim, through `swift-rs` (1.0.9, "Call Swift from Rust") or `swift-bridge` (0.1.59), or a small Swift helper process ([crates.io: swift-rs](https://crates.io/crates/swift-rs), [swift-bridge](https://crates.io/crates/swift-bridge)). typeThai's Info.plist sets `LSMinimumSystemVersion` 12.0, so this would also need a runtime availability gate.
- **Verdict:** not worth building for typeThai until Apple adds Thai. At most, an English-only rewrite path.

### 3.6 Windows on-device (Windows AI APIs / Phi Silica)

- Phi Silica runs on the NPU of Copilot+ PCs, and now also on NVIDIA RTX 30+ or AMD RX 9060+ GPUs with 6 GB or more VRAM. GPU mode currently needs Developer Mode and an Insider Experimental build ([Microsoft](https://learn.microsoft.com/en-us/windows/ai/apis/phi-silica)).
- It is a **Limited Access Feature** that needs an unlock token. It is "not available in China".
- It ships built-in "Text Intelligence Skills", including **Rewrite** with tone.
- **Phi Silica is being replaced by Aion Instruct.** Rollout reaches Insiders in November 2026 and retail devices in January 2027, at which point Phi Silica is removed (same page).
- Apps need the `systemAIModels` capability in `Package.appxmanifest`, meaning package identity, plus the Windows App SDK ([Get started](https://learn.microsoft.com/en-us/windows/ai/apis/get-started)).
- **Languages:** "Performance may vary across languages, and some languages may not be supported", with no list ([Transparency Note](https://learn.microsoft.com/en-us/windows/ai/apis/phi-silica-transparency-note)). **Thai support is unverified.**
- **Verdict:** not a fit now. It is a narrow hardware base, needs packaging and identity, has a model change pending, and Thai support is unknown.

---

## 4. Thai-language quality: first-party evidence

- **Typhoon 2 tech report** (SCB 10X, [arXiv 2412.13702](https://arxiv.org/abs/2412.13702)). Instruct-model tables 13–15, where CS is the code-switching accuracy (the response is mostly Thai with no foreign characters) at temperature 0.7 / 1.0:

  | Model | IFEval TH / EN | MT-Bench TH / EN | CS t=0.7 / 1.0 |
  |---|---|---|---|
  | Typhoon2-Llama3.2-**1B**-Instruct | 52.46 / 53.35 | 3.97 / 5.21 | 96.4 / 88.0 |
  | Llama-3.2-1B-Instruct | 31.76 / 51.15 | 2.58 / 6.23 | 97.8 / 22.6 |
  | Qwen2.5-1.5B-Instruct | 44.42 / 48.45 | 2.94 / 6.93 | 82.6 / 20.6 |
  | Typhoon2-Llama3.2-**3B**-Instruct | 68.36 / 72.18 | 5.34 / 7.21 | 99.2 / 96.0 |
  | Llama3.2-3B-Instruct | 44.84 / 71.98 | 4.32 / 7.73 | 93.8 / **21.2** |
  | Qwen2.5-3B-Instruct | 58.86 / 67.25 | 4.63 / 7.85 | 78.6 / 38.0 |
  | Typhoon2-Qwen2.5-**7B**-Instruct | 74.3 / 73.3 | 6.18 / 8.09 | 99.2 / 96.8 |
  | OpenThaiGPT 1.5 7B | 67.3 / 75.4 | 5.69 / 8.10 | 93.8 / 28.0 |
  | Llama3.1-8B-Instruct | 58.0 / 77.6 | 5.10 / 8.11 | 93.0 / 11.2 |

  Takeaway: models that are **not** Thai-tuned often drift out of Thai at temperature 1.0. That directly breaks a "rewrite this Thai text" feature, so pin a low temperature and use Thai-tuned models.

  The same report gives ThaiExam scores for base models: Llama3.2-1B 25.38, Typhoon2-1B 26.83, Llama3.2-3B 40.42, Typhoon2-3B 44.53, Llama3.1-8B 45.80, Typhoon2-Llama-8B 51.20, Typhoon2-Qwen2.5-7B 58.86. The authors caution that exam scores are affected by "contamination and saturation".
- **Typhoon 2.1 (Gemma 3 base) model cards.** These are images on the cards; values transcribed here.

  | Model | IFEval TH | MT-Bench TH | Code-switching 0.7 / 1.0 |
  |---|---|---|---|
  | Typhoon2.1-Gemma3-4B ([card](https://huggingface.co/typhoon-ai/typhoon2.1-gemma3-4b)) | 74.0 | 6.5 | 95.4 / 94.2 |
  | Gemma 3 4B IT (same card) | 69.4 | 6.4 | **52.2 / 47.2** |
  | Typhoon2.1-Gemma3-12B ([card](https://huggingface.co/typhoon-ai/typhoon2.1-gemma3-12b)) | 83.0 | 7.5 | 98.0 / 94.6 |
  | Gemma 3 12B IT (same card) | 81.4 | 7.5 | 71.0 / 67.4 |
- **Typhoon 2.5** ([Typhoon2.5-Qwen3-4B card](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b)). This is a 4B model with 256K context, Thai and English as its primary languages, under Apache-2.0, and its license links to Qwen3-4B-Instruct-2507. The card's table (an image) reports Thai averages of typhoon-qwen3-4b **48.3**, qwen3-4b-instruct-2507 43.1, typhoon2.1-gemma3-4b 42.9 and gemma-3-4b-it 39.0. The sub-scores are HotpotQA, instruction-following and MT-Bench, scaled to 100. The card recommends "a low temperature" and `repetition_penalty` 1.05.
- **Typhoon Translate 1.5** ([card](https://huggingface.co/typhoon-ai/typhoon-translate1.5-4b)). This is a 4B model for "controllable, high-quality Thai ↔ English translation", with a fixed prompt template: "Translate the following {English/Thai} text into {Thai/English}, strictly following the rules below and return only the translated text." The card is internally inconsistent: it says "based on Qwen3 architecture" but lists the Gemma License. Its evaluation uses GPT-4o-mini as judge, with no numeric table in the card text.
- **Qwen3 tech report** ([arXiv 2505.09388](https://arxiv.org/abs/2505.09388)). Qwen3 covers "119 languages and dialects". Table 35 (Thai) gives MLogiQA scores:

  | Mode | 0.6B | 1.7B | 4B | 8B | 14B |
  |---|---|---|---|---|---|
  | Non-thinking | 37.5 | 42.5 | 43.8 | 42.5 | 47.5 |
  | Thinking | 33.8 | 48.8 | 60.0 | 68.8 | 65.0 |

  This is a reasoning benchmark, not rewriting, but it shows that Thai ability below 4B is weak.
- **Gemma 3** ([model card](https://ai.google.dev/gemma/docs/core/model_card_3)). "Multilingual support in over 140 languages". There is no per-language Thai score. Multilingual instruction-tuned scores by size:

  | Benchmark | 1B | 4B | 12B | 27B |
  |---|---|---|---|---|
  | Global-MMLU-Lite | 34.2 | 54.5 | 69.5 | 75.1 |
  | WMT24++ | 35.9 | 46.8 | 51.6 | 53.4 |

  Google's Gemma page now leads with **Gemma 4** (E2B, E4B, 12B, 26B A4B, 31B) ([Gemma docs](https://ai.google.dev/gemma/docs/core)); its Thai figures were not found (**unverified**).
- **Llama 3.2** ([model card](https://github.com/meta-llama/llama-models/blob/main/models/llama3_2/MODEL_CARD.md)). **Thai is one of 8 officially supported languages.** Thai MMLU (5-shot) is 34.7 for 1B, 44.5 for 3B and 50.3 for Llama 3.1 8B. A "Vanilla PTQ" 4-bit quantization lowers 3B Thai to 41.3.
- **SEA-HELM** (AI Singapore, [leaderboard](https://leaderboard.sea-lion.ai/)). It includes a Thai track, but per-model Thai scores could not be extracted from the page (**unverified**).
- **Apple and Microsoft on-device models:** see §3.5 and §3.6. Thai is absent from Apple's list and unspecified by Microsoft.
- **No spaces between words.** "Thai words are not separated by spaces" (Unicode Standard 18.0, [§16.1](https://www.unicode.org/versions/Unicode18.0.0/core-spec/chapter-16/)). Implications:
  - **Diff:** do not use whitespace tokenization. `icu_segmenter` 2.3.0's `WordSegmenter::new_auto` / `new_dictionary` / `new_lstm` cover Thai with dictionary or LSTM data ([docs.rs](https://docs.rs/icu_segmenter/latest/icu_segmenter/struct.WordSegmenter.html)). Use it for word-level highlights, or fall back to grapheme-cluster diffs so vowel and tone marks are never split from their base consonant. That is a design recommendation.
  - **Prompts:** tell the model "do not insert spaces between Thai words; keep existing spacing". Small models sometimes add or remove spaces (**unverified**; check during evaluation).
  - **Polite particles:** ครับ is used by male speakers and ค่ะ / คะ by female speakers. The "make polite" preset needs a user setting for this and should never guess.

---

## 5. Local model sizing: the smallest usable Thai model

*(Section added at the coordinator's request.)*

### 5.1 Memory at Q4 and Q8 (file sizes from official repos and Ollama)

llama.cpp's reference table for Llama-3.1-8B gives **Q4_K_M = 4.89 bits/weight (4.58 GiB)** and **Q8_0 = 8.50 bits/weight (7.95 GiB)**. It also notes that RAM need ≈ file size, because models are fully loaded ([llama.cpp quantize README](https://github.com/ggml-org/llama.cpp/blob/master/tools/quantize/README.md)). Add the KV cache and runtime overhead on top. Google's Gemma table explicitly excludes "the context window (KV cache) and supporting software" ([Gemma docs](https://ai.google.dev/gemma/docs/core)).

| Model (tier) | Q4 file | Q8_0 file | Source |
|---|---|---|---|
| Qwen3-0.6B | 523 MB (Ollama q4_K_M) | 0.64 GB | [Ollama](https://ollama.com/library/qwen3/tags), [Qwen GGUF](https://huggingface.co/Qwen/Qwen3-0.6B-GGUF) |
| Qwen3-1.7B | 1.4 GB | 1.83 GB | Ollama, [Qwen GGUF](https://huggingface.co/Qwen/Qwen3-1.7B-GGUF) |
| Gemma 3 1B | 806 MB (q4_K_M); 1.00 GB (QAT q4_0) | 1.1 GB | [Ollama](https://ollama.com/library/gemma3/tags), [Google QAT GGUF](https://huggingface.co/google/gemma-3-1b-it-qat-q4_0-gguf) |
| Typhoon2-Llama3.2-1B / 3B | no official GGUF (MLX 4-bit only) | n/a | [typhoon-ai on HF](https://huggingface.co/typhoon-ai); **derived** from llama.cpp bits/weight (4.89 / 8.50), assuming about 1.24B and 3.2B parameters (**unverified**): ≈0.8 / 1.3 GB (1B) and ≈2.0 / 3.4 GB (3B) |
| Qwen3-4B | 2.50 GB | 4.28 GB | [Qwen GGUF](https://huggingface.co/Qwen/Qwen3-4B-GGUF) |
| **Typhoon2.5-Qwen3-4B** | **2.50 GB** | ≈4.3 GB (**derived**, same architecture) | [typhoon-ai GGUF](https://huggingface.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf) |
| **Typhoon2.1-Gemma3-4B** (text-only) | **2.49 GB** | n/a | [typhoon-ai GGUF](https://huggingface.co/typhoon-ai/typhoon2.1-gemma3-4b-gguf) |
| Typhoon-Translate1.5-4B | 2.50 GB | n/a | [typhoon-ai GGUF](https://huggingface.co/typhoon-ai/typhoon-translate1.5-4b-gguf) |
| Gemma 3 4B (with vision) | 3.4 GB (q4_K_M); 3.16 GB + 0.85 GB mmproj (QAT) | 5.0 GB | Ollama, [Google QAT GGUF](https://huggingface.co/google/gemma-3-4b-it-qat-q4_0-gguf) |
| OpenThaiGPT 1.5 7B | 4.68 GB | 8.10 GB | [OpenThaiGPT GGUF](https://huggingface.co/openthaigpt/openthaigpt1.5-7b-instruct-GGUF) |
| Llama 3.1 8B | 4.58 GiB | 7.95 GiB | llama.cpp quantize README |
| Qwen3-8B | 5.03 GB | 8.71 GB | [Qwen GGUF](https://huggingface.co/Qwen/Qwen3-8B-GGUF) |
| **Typhoon2.1-Gemma3-12B** | **7.30 GB** | n/a | [typhoon-ai GGUF](https://huggingface.co/typhoon-ai/typhoon2.1-gemma3-12b-gguf) |
| Gemma 3 12B | 8.2 GB (q4_K_M); 8.07 GB (QAT) | 13 GB | Ollama, Google |
| Qwen3-14B | 9.00 GB | 15.70 GB | [Qwen GGUF](https://huggingface.co/Qwen/Qwen3-14B-GGUF) |

### 5.2 Thai tokenizer efficiency

- **Documented.** The Typhoon 1 paper reports token efficiency on Thai relative to GPT-4's tokenizer ([arXiv 2312.13951](https://arxiv.org/abs/2312.13951), Table 2):

  | Tokenizer | Efficiency vs GPT-4 |
  |---|---|
  | Typhoon | 262% |
  | Mistral | 93% |
  | Character-level | 89% |
  | Word-level (newmm) | 326% |

  **Derived** from those ratios: GPT-4's tokenizer uses about 0.89 tokens per Thai character, Mistral about 0.96, and Typhoon-1 about 0.34. Typhoon 2 deliberately did **not** extend the tokenizer, because doing so "can degrade overall performance" ([arXiv 2412.13702](https://arxiv.org/abs/2412.13702) §2.4). Anthropic documents only an all-language average for its current tokenizer: 1M tokens ≈ 2.5M Unicode characters ([Models overview](https://platform.claude.com/docs/en/models/overview)). It gives no Thai figure.
- **Measured locally (not a vendor claim).** On a 376-character Thai sample (three everyday sentences), using each repo's `tokenizer.json` with HF `tokenizers`:

  | Tokenizer | Tokens | Tokens per Thai character |
  |---|---|---|
  | Qwen3 / Typhoon2.5-Qwen3 / OpenThaiGPT 1.5 (Qwen2.5) | 190 | **0.51** |
  | Llama 3.2 (Typhoon2-1B) | 174 | 0.46 |
  | Gemma 3 (Typhoon2.1-Gemma3) | 111 | **0.30** |

  For the same Thai output, a Gemma-based model generates about 40% fewer tokens than a Qwen-based one, and that translates directly into lower rewrite latency.

### 5.3 Speed (tokens per second)

- **Apple silicon** (llama.cpp scoreboard, [discussion #4167](https://github.com/ggml-org/llama.cpp/discussions/4167)). Llama-2-7B text generation (tg128); builds from November 2023, and the thread notes newer builds are faster:

  | Chip | Q4_0 (t/s) | Q8_0 (t/s) |
  |---|---|---|
  | M1 (8-core GPU) | 14.15 | 7.91 |
  | M2 | 21.91 | 12.21 |
  | M3 | 21.34 | 12.27 |
  | M4 | 24.11 | 13.54 |
  | M1 Pro | 36.41 | 22.34 |
  | M2 Pro | 37.87 | 22.70 |
  | M4 Pro | 50.74 | 30.69 |
  | M3 Max | 66.31 | 42.75 |
- **Windows laptops** (Vulkan scoreboard, [discussion #10879](https://github.com/ggml-org/llama.cpp/discussions/10879); community-submitted). Llama-2-7B Q4_0 tg128:

  | Hardware | t/s |
  |---|---|
  | RTX 4050 Mobile | 41.89 |
  | RTX 4060 Mobile | 59.53 |
  | AMD Ryzen AI 300 iGPU | 22.41 |
  | Intel Iris Xe (i7-1185G7) | about 6–8 (thread comments, not the scoreboard) |
- **Estimate.** Generation is roughly memory-bandwidth bound, so a 2.5 GB 4B Q4 model should run about 1.3–1.5× faster than the 7B Q4_0 figures. This is **unverified**; benchmark on target machines. For a typical 200-character Thai sentence:
  - a Qwen-tokenizer model emits about 100 tokens, which at about 20 t/s (M1/M2 base) is **5 s**;
  - a Gemma-tokenizer model emits about 60 tokens, about **3 s**;
  - plus prompt processing and any cold load.

  That is acceptable for "preview, then accept" and too slow for anything automatic.

### 5.4 Apple's ~3B on-device model

It is about 3B parameters with 2-bit QAT, but **it does not support Thai**: Apple Intelligence's language list has no Thai, and the framework throws `unsupportedLanguageOrLocale` (see §3.5).

### 5.5 Smallest usable tier, and defaults

These are judgments from the evidence above.

- **1B and smaller is not usable for Thai rewriting.** Typhoon2-1B scores MT-Bench TH 3.97/10 and IFEval-TH 52.5. Qwen3-0.6B and 1.7B score 37.5 and 42.5 on Thai MLogiQA (non-thinking). Untuned 1–3B models code-switch at temperature 1.0: Llama-3.2-3B scores 21.2% and Qwen2.5-3B 38.0%.
- **3–4B Thai-tuned is the smallest usable tier.** Typhoon2-3B reaches IFEval-TH 68.4 with code-switching 99.2 / 96.0. Typhoon2.1-Gemma3-4B reaches IFEval-TH 74.0 with code-switching 95.4 / 94.2. Typhoon2.5-Qwen3-4B has the best Thai average among the 4B models on its card. Expect it to handle typo fixes and particle or tone changes; the translation preset is better served by Typhoon-Translate1.5-4B. Verify both with a small typeThai-specific eval set.
- **Recommended default for 8 GB machines: Typhoon2.5-Qwen3-4B at Q4_K_M (2.50 GB).** Use Typhoon2.1-Gemma3-4B Q4_K_M (2.49 GB) when speed matters more, since its tokenizer uses 0.30 against 0.51 tokens per Thai character. Keep a single model resident; swapping in Typhoon-Translate for translations doubles RAM or adds a load delay.
- **Recommended default for 16 GB machines: Typhoon2.1-Gemma3-12B at Q4_K_M (7.30 GB).** It scores IFEval-TH 83.0, MT-Bench TH 7.5 and code-switching 98.0 / 94.6, and benefits from the efficient tokenizer. Alternatives are Typhoon2-Qwen2.5-7B or Qwen3-8B Q4_K_M (about 5 GB).
- On machines slower than an M1 base or a Ryzen AI iGPU (for example Iris Xe at about 6–8 t/s on 7B), recommend a cloud provider instead.

---

## 6. Latency and privacy

- **Streaming** is supported by every candidate API:
  - Anthropic over SSE ([Streaming](https://platform.claude.com/docs/en/build-with-claude/streaming));
  - OpenAI over SSE ([API reference](https://developers.openai.com/api/reference/resources/chat));
  - Gemini through its OpenAI-compatible endpoint ([Gemini OpenAI](https://ai.google.dev/gemini-api/docs/openai));
  - Ollama and llama.cpp ([Ollama](https://docs.ollama.com/api/openai-compatibility), [llama.cpp](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)).

  Stream into the bubble so the first characters show quickly.
- **Time to first token** is not published as a number by any vendor checked; Anthropic gives only relative ranks such as "Fastest" (**unverified**). To reduce it:
  - use Haiku 5.5 with `effort: "low"` and thinking disabled ([Effort](https://platform.claude.com/docs/en/build-with-claude/effort));
  - keep prompts short;
  - for local models, preload them (Ollama's empty request) and keep them alive ([Ollama FAQ](https://docs.ollama.com/faq)).
- **Consent.** A first-use dialog must name the provider and say that the selected text leaves the machine. Show a provider badge in the bubble on every request. Default to a local provider or "off".
  - Gemini's free tier uses content to improve products ([pricing](https://ai.google.dev/gemini-api/docs/pricing)). Warn the user, or require a paid key.
  - Anthropic states retained API data "is never used for model training without your express permission" ([API and data retention](https://platform.claude.com/docs/en/manage-claude/api-and-data-retention)).
  - Ollama: "We don't see your prompts or data when you run locally" ([Ollama FAQ](https://docs.ollama.com/faq)).
- **API key storage.** Use the `keyring` crate, version 4.2.0. Its default `v1` feature enables `apple-native-keyring-store/keychain` and `windows-native-keyring-store`, the macOS Keychain and Windows Credential Manager ([crates.io API](https://crates.io/crates/keyring), [README](https://github.com/open-source-cooperative/keyring-rs)). The README advises applications needing control to depend on `keyring-core` plus specific stores instead. Never store keys in typeThai's JSON prefs.
- **Never send:**
  - text from password fields (§1.3);
  - text captured while secure input is on;
  - text from apps on a user-editable denylist (password managers, banking). The existing prefs file can hold this list.

  Cap the length of captured text (for example 2,000 characters) and refuse anything longer, to avoid accidentally sending a whole document.

---

## 7. UX patterns from existing tools (official docs)

- **Apple Writing Tools.**
  - *Trigger:* select text, Control-click, Writing Tools, then Show Writing Tools.
  - *Options:* Describe a change, Proofread, Rewrite with Friendly / Professional / Concise tones, and Summarize / Key Points ([Mac User Guide](https://support.apple.com/guide/mac-help/writing-tools-write-improve-summarize-mchldcd6c260/mac)).
  - *Preview and accept:* developer docs describe a "limited" experience that "keeps changes in the Writing Tools interface until the person approves them" (an overlay panel), and a "complete" inline experience. "Both experiences give people the ability to accept or reject changes" ([Apple](https://developer.apple.com/documentation/uikit/customizing-writing-tools-behavior-for-system-views), [NSWritingToolsBehavior](https://developer.apple.com/documentation/appkit/nswritingtoolsbehavior)).
  - Writing Tools depends on Apple Intelligence, which lacks Thai (§3.5). Thai users therefore have no built-in equivalent, which is the gap typeThai can fill (**derived**).
- **Raycast AI Commands.**
  - *Preset prompts:* saved prompts with a `{selection}` placeholder. The built-ins include Fix Spelling and Grammar, Improve Writing, and Change Tone to Professional/Friendly.
  - *Output behaviour:* "Open in Raycast", "Replace Selection", and "Highlight Editing Changes".
  - *Quick Fix:* runs Fix Spelling and Grammar in any app on a double press of right Shift, and needs Accessibility permission on macOS ([Raycast manual](https://manual.raycast.com/ai/ai-commands)).
  - Pattern: hotkey trigger, presets plus custom prompts, and a choice between replacing in place and previewing.
- **PopClip.**
  - *Trigger:* a popup on text selection. Actions read `popclip.input.text`.
  - *After-steps:* `paste-result` ("If the app's Paste command is available, paste the text … Otherwise, copy it"), `preview-result` (shows the result, click to paste), `copy-result` and `show-result`.
  - `restorePasteboard` restores the clipboard after `paste-result`. `excludedApps` excludes apps by bundle ID ([PopClip developer docs](https://www.popclip.app/dev/all.md)).
  - Pattern: replacement **by paste**, preview before paste, and a per-app exclusion list.
- **Grammarly desktop.** It needs Accessibility on Mac and reads text through the OS accessibility layer, which is UIA TextPattern on Windows ([Grammarly support](https://support.grammarly.com/hc/en-us/articles/10139846131213-How-do-I-integrate-Grammarly-with-my-website-or-application)). Admins can block specific apps by bundle ID ([Grammarly](https://support.grammarly.com/hc/en-us/articles/27454187430029-Manage-application-controls)). Details of the suggestion-card UI were not found in primary docs (**unverified**).

**Synthesis:**

| Aspect | Options seen | Choice for typeThai |
|---|---|---|
| Trigger | hotkey (Raycast) or popup on selection (PopClip) | hotkey, because typeThai is keyboard-first |
| Actions | a small preset list plus optional custom prompt | same |
| Preview | before replacing (Writing Tools, PopClip `preview-result`) | always preview |
| Replacement | paste | paste |
| Undo | host app's undo, or explicit reject | host app's undo, plus reject before accepting |

---

## 8. Recommendation for typeThai

### 8.1 Minimal design

1. **Trigger:** `Ctrl+Alt+R`. Add `'r' => Hotkey::Rewrite` in `app.rs::hotkey_for`; it is free, and the existing chords are Z/X/G/1/2/3/P. It works on both OSes through the existing tap and hook path.
2. **Guard:** if the focused element is secure or a password field, or the frontmost app is on the denylist, show "Not available here" and stop (§1.3).
3. **Capture**, in this order:
   1. AX `AXSelectedText` (macOS) or UIA `GetSelection` → `GetText` (Windows), on the existing UIA worker. Accept non-empty text, use at most about 100 ms, and keep the existing 0.25 s AX messaging timeout.
   2. The clipboard fallback: save the clipboard, release the held Ctrl/Alt, synthesize Cmd/Ctrl+C, wait for `changeCount` or the sequence number to change (about 300 ms timeout), read the text, restore the clipboard.
   3. Nothing captured: show "Select some text first".
4. **Preset menu in the existing bubble:**
   - **Fix Thai typos** (default; Accept 0)
   - **Make polite** (uses the configured ครับ or ค่ะ; Accept 1)
   - **Translate TH↔EN** (direction picked automatically from the script, using the core crate's existing Thai/English detection; Accept 2)

   Further presets can live under Shift+Up/Down, and a custom prompt can be set in Settings. Use row 1/2/3 or Tab to choose. Preload the local model on hotkey-down.
5. **Request:** stream from the provider, with a system prompt that:
   - forces "return only the rewritten text";
   - names the output language;
   - tells the model to preserve line breaks and not to add spaces between Thai words;
   - fixes particle gender.

   Use a low temperature for local models, and omit sampling parameters on Claude 5.x.
6. **Preview:** stream the result into the bubble with a grapheme- or word-level diff highlight (§4).
   - **Tab or Ctrl+Alt+1** accepts. Paste with a "don't record" clipboard marker, then restore the clipboard after a short delay.
   - **Esc** cancels and aborts the stream.
   - If focus or the selection changed while the result was generating, re-check before pasting, and copy the result to the clipboard instead if it changed.
7. **Undo:** rely on the host app's Cmd+Z / Ctrl+Z, and say so in the bubble hint ("⌘Z to undo"). Keep the original text in memory for one "restore original" action.

### 8.2 Provider abstraction

- **`trait RewriteProvider { fn stream(&self, req: RewriteRequest, on_delta: impl FnMut(&str)) -> Result<()> }`** with two implementations:
  - **OpenAICompatible { base_url, api_key: Option, model }**, which covers:
    - Ollama: `http://localhost:11434/v1/`
    - llama-server: `http://127.0.0.1:9931/v1/`
    - OpenAI: `https://api.openai.com/v1/`
    - Gemini: `https://generativelanguage.googleapis.com/v1beta/openai/`
  - **Anthropic** (native `/v1/messages` with SSE, `claude-haiku-5-5` by default, `output_config.effort: "low"`, `thinking: {type: "disabled"}`).
- **HTTP:** a blocking client on a worker thread fits the existing thread-and-channel model. `ureq` 3.4.2 is current on [crates.io](https://crates.io/crates/ureq); `reqwest` 0.13.5 is the async alternative ([crates.io](https://crates.io/crates/reqwest)). Parse SSE `data:` lines by hand; both wire formats are simple.
- **Settings** (`prefs.rs` / `ui/settings.rs`):
  - provider, base URL, model, and a key stored in `keyring`;
  - polite-particle gender;
  - denylist;
  - max characters;
  - a "send to cloud" consent flag with a timestamp.
- **New dependencies** (all additions; nothing existing changes):
  - `objc2-app-kit` feature `NSPasteboard`;
  - `windows` feature `Win32_System_DataExchange`, plus `Win32_System_Memory` for `GlobalAlloc` (**derived** from windows-rs conventions);
  - `keyring`;
  - an HTTP client;
  - optionally `icu_segmenter`.

### 8.3 Open risks

- **Coverage gaps:** AX/UIA selection is missing in some Electron, web, terminal and canvas apps (**unverified** per app). The clipboard fallback then exposes the selection to Windows clipboard history (§1.2), and elevated windows are unreachable through UIPI.
- **Clipboard races:** restoring too early pastes stale content. Apps with lazy rendering can delay the `changeCount` or sequence-number update. Multi-format restore can lose exotic types.
- **Latency on low-end hardware:** about 6–8 t/s on Iris Xe-class iGPUs (§5.3). Thai on Qwen-tokenizer models needs about 1.7× the tokens of Gemma-tokenizer models.
- **Quality:** Thai is unbenchmarked for current Claude, OpenAI and Gemini models in vendor docs. Small models may code-switch, change meaning in "fix typos", or alter spacing. typeThai needs a small Thai eval set: wrong-layout leftovers, typos, politeness and TH↔EN. Run it before choosing defaults.
- **Interaction with typeThai's engine:** pasting triggers the app's own key handling. Make sure the engine ignores the synthesized Cmd/Ctrl+C/V; the existing `MAGIC` tag can cover `CGEvent`s, and on Windows `dwExtraInfo` is the equivalent. Also reset the engine buffer after replacement.
- **Platform churn:** Phi Silica is being replaced by Aion Instruct (January 2027). Apple's Thai support may change. Model IDs and prices above are as of 2026-10-15.
- **Model licensing:** Gemma-based Typhoon models use the Gemma License. Qwen-based ones are Apache-2.0, but Typhoon-Translate1.5's card is inconsistent. Redistributing or bundling weights needs a license check; pointing users to `ollama run hf.co/...` avoids redistribution.
