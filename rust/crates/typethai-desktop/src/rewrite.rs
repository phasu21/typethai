//! AI rewrite of the selected text: presets, config, the OpenAI-compatible wire format, the API key, the clipboard
//! protocol and the worker threads. The only module that knows the wire format or can read the key.
use serde_json::{json, Value};
use std::sync::mpsc;
use std::time::Duration;

/// One async request (a selection capture or an HTTP call), minted by the Engine. Ids only grow, so a reply is current
/// exactly when the engine's state still names its id.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReqId(u64);
impl ReqId { pub fn next(self) -> ReqId { ReqId(self.0 + 1) } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset { Fix, Translate }
impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Fix, Preset::Translate];
    pub fn label(self) -> &'static str { match self { Preset::Fix => "Fix typos", Preset::Translate => "Translate TH↔EN" } }
    fn instructions(self) -> &'static str { match self { Preset::Fix => FIX, Preset::Translate => TRANSLATE } }
}
const FIX: &str = "You fix spelling and typing mistakes in Thai and English text. Keep the wording, meaning, tone and punctuation exactly the same; change only misspelled words. If there are no mistakes, return the text unchanged. Output only the resulting text. No quotes, no explanation, no preamble.";
const TRANSLATE: &str = "You translate between Thai and English. Detect the language of the text, then translate Thai into natural English or English into natural Thai. Translate every detail; do not summarize or leave anything out. Do not add ครับ or ค่ะ unless the source text is polite. Output only the translated text. No quotes, no explanation, no preamble.";

/// Nothing selected and no word before the caret.
pub const NO_SELECTION: &str = "Select text or type a word first";
/// A word or selection with nothing to fix and no AI rows.
pub const NO_SUGGESTIONS: &str = "No suggestions";
pub const NO_KEY: &str = "Add an API key in Settings";
pub const REWRITING: &str = "Rewriting…";
/// Longest result label on the bubble; Tab still pastes the whole text.
pub const PREVIEW_CHARS: usize = 600;
const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_MODEL: &str = "google/gemma-4-26b-a4b-it";
const TIMEOUT: Duration = Duration::from_secs(20);
const BAD_URL: &str = "The base URL must start with https:// (http:// only for localhost).";

/// Non-secret settings (prefs.json "rewrite"), normalized on construction. `instructions` is the user's own text,
/// added to every preset's system prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config { pub enabled: bool, pub base_url: String, pub model: String, pub instructions: String }
impl Default for Config { fn default() -> Self { Config { enabled: true, base_url: DEFAULT_BASE_URL.into(), model: DEFAULT_MODEL.into(), instructions: String::new() } } }
impl Config {
    /// Settings fields: empty values take the defaults; a URL that would send the key in clear is refused.
    pub fn new(enabled: bool, base_url: &str, model: &str, instructions: &str) -> Result<Config, String> {
        Ok(Config { enabled, base_url: base_url_of(base_url)?, model: model_of(model), instructions: instructions.trim().into() })
    }
    /// prefs.json: missing or invalid fields fall back to the defaults.
    pub fn from_json(v: Option<&Value>) -> Config {
        let Some(v) = v else { return Config::default() };
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
        Config { enabled: v.get("enabled").and_then(Value::as_bool).unwrap_or(true), base_url: base_url_of(s("base_url")).unwrap_or_else(|_| DEFAULT_BASE_URL.into()), model: model_of(s("model")), instructions: s("instructions").trim().into() }
    }
    pub fn to_json(&self) -> Value { json!({ "enabled": self.enabled, "base_url": self.base_url, "model": self.model, "instructions": self.instructions }) }
}
fn model_of(raw: &str) -> String { let m = raw.trim(); if m.is_empty() { DEFAULT_MODEL.into() } else { m.into() } }
fn base_url_of(raw: &str) -> Result<String, String> {
    let u = raw.trim().trim_end_matches('/');
    if u.is_empty() { return Ok(DEFAULT_BASE_URL.into()) }
    match scheme_host(u) {
        Some((s, _)) if s.eq_ignore_ascii_case("https") => Ok(u.into()),
        Some((s, h)) if s.eq_ignore_ascii_case("http") && (h.eq_ignore_ascii_case("localhost") || h.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())) => Ok(u.into()),
        _ => Err(BAD_URL.into()),
    }
}
fn scheme_host(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let auth = rest.split(['/', '?', '#']).next()?; let hp = auth.rsplit('@').next()?;
    let host = match hp.strip_prefix('[') { Some(r) => &r[..r.find(']')?], None => hp.split(':').next()? };
    (!host.is_empty()).then_some((scheme, host))
}

/// Work the engine hands out through `Action::Rewrite`. No variant can hold the API key.
#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// Read the focused field's selection. Answers `Reply::Captured`.
    Capture(ReqId),
    /// Call the endpoint. Answers `Reply::Rewritten`.
    Run(Run),
    /// Replace the selection with this text through the clipboard. No answer.
    Paste(String),
}
#[derive(Clone, Debug, PartialEq)]
pub struct Run { pub req: ReqId, pub preset: Preset, pub text: String, pub cfg: Config }

#[derive(Clone, Debug, PartialEq)]
pub enum Captured {
    Text(String),
    /// The field reports an empty selection, or the clipboard fallback saw no copy.
    NoSelection,
    /// Password field / secure input: refuse silently.
    Secure,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Reply {
    /// `has_key`: an API key is stored (read only when text was captured).
    Captured { id: ReqId, sel: Captured, has_key: bool },
    Rewritten(ReqId, Result<String, Failure>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Failure { NoKey, Timeout, Network, Rejected, RateLimited, Status(u16, Option<String>), BadResponse }
impl Failure {
    /// One short line for the notice row.
    pub fn message(&self) -> String {
        match self {
            Failure::NoKey => NO_KEY.into(), Failure::Timeout => "The rewrite timed out".into(), Failure::Network => "Could not reach the rewrite service".into(),
            Failure::Rejected => "The API key was rejected".into(), Failure::RateLimited => "Rate limited. Try again in a moment".into(),
            Failure::Status(c, Some(m)) => format!("Rewrite failed ({c}): {}", preview(m, 80)), Failure::Status(c, None) => format!("Rewrite failed ({c})"),
            Failure::BadResponse => "Unexpected reply from the model".into(),
        }
    }
}

fn endpoint(cfg: &Config) -> String { format!("{}/chat/completions", cfg.base_url) }
fn system(run: &Run) -> String {
    let preset = run.preset.instructions();
    if run.cfg.instructions.is_empty() { preset.into() } else { format!("{preset}\n\nAlso follow these instructions from the user:\n{}", run.cfg.instructions) }
}
fn body(run: &Run) -> String {
    let mut b = json!({ "model": run.cfg.model, "temperature": 0.2, "messages": [{ "role": "system", "content": system(run) }, { "role": "user", "content": run.text }] });
    // OpenRouter-only fields; other OpenAI-compatible servers may reject unknown keys
    if scheme_host(&run.cfg.base_url).is_some_and(|(_, h)| h.eq_ignore_ascii_case("openrouter.ai")) {
        b["reasoning"] = json!({ "enabled": false }); b["provider"] = json!({ "data_collection": "deny" });
    }
    b.to_string()
}
fn parse(status: u16, body: &str) -> Result<String, Failure> {
    let v: Option<Value> = serde_json::from_str(body).ok();
    match status {
        200..=299 => v.as_ref().and_then(|v| v["choices"][0]["message"]["content"].as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from).ok_or(Failure::BadResponse),
        401 | 403 => Err(Failure::Rejected), 429 => Err(Failure::RateLimited),
        c => Err(Failure::Status(c, v.as_ref().and_then(|v| v["error"]["message"].as_str()).map(String::from))),
    }
}
/// An HTTP client that answers every status (no error for 4xx/5xx). Windows uses SChannel and the OS certificate store.
pub fn http_agent(timeout: Duration) -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = if cfg!(windows) { TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier) } else { TlsConfig::builder() };
    ureq::Agent::new_with_config(ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).tls_config(tls.build()).build())
}
/// Blocking POST, run on its own short-lived thread.
fn call(run: &Run, key: &secret::ApiKey) -> Result<String, Failure> {
    let agent = http_agent(TIMEOUT);
    let fail = |e: ureq::Error| if matches!(e, ureq::Error::Timeout(_)) { Failure::Timeout } else { Failure::Network };
    let mut res = agent.post(&endpoint(&run.cfg)).header("Authorization", &key.bearer()).header("Content-Type", "application/json").send(&body(run)).map_err(fail)?;
    let text = res.body_mut().read_to_string().map_err(fail)?;
    parse(res.status().as_u16(), &text).map(|out| finish(&run.text, &out))
}
/// Put the selection's own leading/trailing whitespace back around the model output (a selected line keeps its "\n").
fn finish(original: &str, out: &str) -> String {
    let lead = &original[..original.len() - original.trim_start().len()]; let trail = &original[original.trim_end().len()..];
    format!("{lead}{}{trail}", out.trim())
}
/// Label for the result row: at most `max` chars, "…" when cut.
pub fn preview(text: &str, max: usize) -> String {
    if text.chars().count() <= max { return text.to_string() }
    format!("{}…", text.chars().take(max.saturating_sub(1)).collect::<String>())
}

/// The API key, in the macOS Keychain or the Windows Credential Manager. `ApiKey` and `load` never leave this file.
mod secret {
    pub struct ApiKey(String);
    impl std::fmt::Debug for ApiKey { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("ApiKey(***)") } }
    impl ApiKey { pub fn bearer(&self) -> String { format!("Bearer {}", self.0) } }
    #[cfg(any(windows, target_os = "macos"))]
    fn entry() -> Result<keyring::Entry, String> { keyring::Entry::new("typethai", "ai-rewrite").map_err(|e| e.to_string()) }
    /// May block on a Keychain prompt: call only off the capture and UI threads.
    pub fn load() -> Option<ApiKey> {
        #[cfg(any(windows, target_os = "macos"))] { entry().ok()?.get_password().ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).map(ApiKey) }
        #[cfg(not(any(windows, target_os = "macos")))] { None }
    }
    /// Trimmed; empty deletes the entry.
    pub fn save(key: &str) -> Result<(), String> {
        #[cfg(any(windows, target_os = "macos"))] {
            let (e, k) = (entry()?, key.trim());
            if !k.is_empty() { return e.set_password(k).map_err(|e| e.to_string()) }
            match e.delete_credential() { Ok(()) | Err(keyring::Error::NoEntry) => Ok(()), Err(e) => Err(e.to_string()) }
        }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = key; Err("No keychain on this OS".into()) }
    }
}
/// Settings: store the key, or clear it with an empty string. Call off the UI thread.
pub fn save_key(key: &str) -> Result<(), String> { secret::save(key) }
/// Settings: whether a key is stored. Call off the UI thread.
pub fn has_key() -> bool { secret::load().is_some() }

/// The OS primitives the clipboard protocol needs. Built and used only on the desk thread.
pub trait Host {
    type Snap;
    fn is_secure(&mut self) -> bool;
    /// The focused field's selection: None when Accessibility cannot tell, Some("") when nothing is selected.
    fn selection_via_accessibility(&mut self) -> Option<String>;
    fn clip_seq(&mut self) -> u64;
    /// Every item and format on the clipboard.
    fn clip_snapshot(&mut self) -> Self::Snap;
    fn clip_restore(&mut self, s: Self::Snap);
    fn clip_read_text(&mut self) -> Option<String>;
    /// Set `text`, marked so clipboard managers and history skip it.
    fn clip_write_transient(&mut self, text: &str);
    fn post_copy(&mut self);
    fn post_paste(&mut self);
    fn pause(&mut self, ms: u64);
}
/// Accessibility first. Only when it cannot tell, copy through the clipboard and put the user's clipboard back.
pub fn capture<H: Host>(h: &mut H) -> Captured {
    if h.is_secure() { return Captured::Secure }
    // an empty selection never falls back to Cmd/Ctrl+C: some editors copy the whole line then
    if let Some(t) = h.selection_via_accessibility() { return if t.trim().is_empty() { Captured::NoSelection } else { Captured::Text(t) } }
    let (snap, seq) = (h.clip_snapshot(), h.clip_seq()); h.post_copy();
    for _ in 0..30 {
        h.pause(10);
        if h.clip_seq() != seq {
            // the change count moves when the copier clears the clipboard, before it writes the text
            h.pause(20); let t = h.clip_read_text(); h.clip_restore(snap);
            return t.map_or(Captured::NoSelection, Captured::Text)
        }
    }
    Captured::NoSelection
}
/// Paste `text` over the selection, then put the user's clipboard back unless something else was copied meanwhile.
pub fn paste<H: Host>(h: &mut H, text: &str) {
    let snap = h.clip_snapshot(); h.clip_write_transient(text); let ours = h.clip_seq(); h.post_paste(); h.pause(250);
    if h.clip_seq() == ours { h.clip_restore(snap) }
}

/// Owned by the capture thread; `submit` and `poll` never block.
pub struct Worker { desk: mpsc::Sender<Job>, rx: mpsc::Receiver<Reply> }
impl Worker {
    /// The desk thread builds its host with `make` and runs captures and pastes one at a time, so it is the only
    /// clipboard writer in the process. Each run gets its own HTTP thread: a slow call never delays a capture or
    /// paste, and an abandoned one answers into a stale id.
    pub fn spawn<H: Host>(make: impl FnOnce() -> H + Send + 'static) -> Worker {
        let (tx, rx) = mpsc::channel(); let (desk, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("typethai-desk".into()).spawn(move || { let mut h = make(); for job in jobs { match job {
            Job::Capture(id) => { let sel = capture(&mut h); let has_key = matches!(sel, Captured::Text(_)) && secret::load().is_some(); let _ = tx.send(Reply::Captured { id, sel, has_key }); }
            Job::Paste(text) => paste(&mut h, &text),
            Job::Run(r) => { let tx = tx.clone(); std::thread::spawn(move || { let res = secret::load().ok_or(Failure::NoKey).and_then(|k| call(&r, &k)); let _ = tx.send(Reply::Rewritten(r.req, res)); }); }
        } } }).expect("desk thread");
        Worker { desk, rx }
    }
    pub fn submit(&self, job: Job) { let _ = self.desk.send(job); }
    pub fn poll(&self) -> Option<Reply> { self.rx.try_recv().ok() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(base_url: &str) -> Run { Run { req: ReqId::default().next(), preset: Preset::Fix, text: "helo wrld".into(), cfg: Config::new(true, base_url, "m/x", "").unwrap() } }
    #[test] fn openrouter_body_denies_data_collection_and_reasoning() {
        let b: Value = serde_json::from_str(&body(&run(""))).unwrap();
        assert_eq!(b, json!({ "model": "m/x", "temperature": 0.2, "reasoning": { "enabled": false }, "provider": { "data_collection": "deny" },
            "messages": [{ "role": "system", "content": FIX }, { "role": "user", "content": "helo wrld" }] }));
    }
    #[test] fn other_servers_get_only_the_standard_fields() {
        let b: Value = serde_json::from_str(&body(&run("http://localhost:11434/v1"))).unwrap();
        assert_eq!(b, json!({ "model": "m/x", "temperature": 0.2, "messages": [{ "role": "system", "content": FIX }, { "role": "user", "content": "helo wrld" }] }));
    }
    #[test] fn user_instructions_follow_the_preset_prompt() {
        let r = Run { cfg: Config::new(true, "", "m/x", "  Use formal Thai.\n").unwrap(), ..run("") };
        let b: Value = serde_json::from_str(&body(&r)).unwrap();
        assert_eq!(b["messages"][0]["content"], format!("{FIX}\n\nAlso follow these instructions from the user:\nUse formal Thai."));
    }
    #[test] fn endpoint_joins_without_double_slash() {
        assert_eq!(endpoint(&Config::new(true, " https://api.example.com/v1/ ", "", "").unwrap()), "https://api.example.com/v1/chat/completions");
        assert_eq!(endpoint(&Config::default()), "https://openrouter.ai/api/v1/chat/completions");
    }
    #[test] fn config_new_trims_defaults_and_refuses_clear_text_urls() {
        assert_eq!(Config::new(false, "  ", "  ", "  "), Ok(Config { enabled: false, base_url: "https://openrouter.ai/api/v1".into(), model: "google/gemma-4-26b-a4b-it".into(), instructions: "".into() }));
        assert_eq!(Config::new(true, "http://127.0.0.1:8080/", " llama ", "").map(|c| (c.base_url, c.model)), Ok(("http://127.0.0.1:8080".into(), "llama".into())));
        assert_eq!(Config::new(true, "HTTP://[::1]:1234/v1", "", "").map(|c| c.base_url), Ok("HTTP://[::1]:1234/v1".into()));
        assert_eq!(Config::new(true, "http://localhost", "", "").map(|c| c.base_url), Ok("http://localhost".into()));
        for bad in ["http://api.example.com/v1", "http://localhost.evil.com", "http://user@127.0.0.1.evil.com", "ftp://localhost", "openrouter.ai/api/v1", "https://"] {
            assert_eq!(Config::new(true, bad, "", ""), Err(BAD_URL.to_string()), "{bad}");
        }
    }
    #[test] fn config_json_round_trips_and_falls_back() {
        let c = Config::new(false, "https://api.example.com/v1", "m/x", "Keep it short.").unwrap();
        assert_eq!(Config::from_json(Some(&c.to_json())), c);
        assert_eq!(Config::from_json(None), Config::default());
        let v = json!({ "enabled": false, "base_url": "http://example.com", "model": "m/y" });
        assert_eq!(Config::from_json(Some(&v)), Config { enabled: false, base_url: "https://openrouter.ai/api/v1".into(), model: "m/y".into(), instructions: "".into() });
    }
    #[test] fn parse_maps_status_and_content() {
        assert_eq!(parse(200, r#"{"choices":[{"message":{"content":"  hello world\n"}}]}"#), Ok("hello world".into()));
        assert_eq!(parse(200, r#"{"choices":[{"message":{"content":"  "}}]}"#), Err(Failure::BadResponse));
        assert_eq!(parse(200, r#"{"choices":[]}"#), Err(Failure::BadResponse));
        assert_eq!(parse(200, "not json"), Err(Failure::BadResponse));
        assert_eq!(parse(401, r#"{"error":{"message":"No auth"}}"#), Err(Failure::Rejected));
        assert_eq!(parse(429, ""), Err(Failure::RateLimited));
        assert_eq!(parse(500, r#"{"error":{"message":"model overloaded"}}"#), Err(Failure::Status(500, Some("model overloaded".into()))));
        assert_eq!(parse(502, "<html>"), Err(Failure::Status(502, None)));
    }
    #[test] fn failure_messages_are_one_short_line() {
        assert_eq!(Failure::Status(500, Some("model overloaded".into())).message(), "Rewrite failed (500): model overloaded");
        assert_eq!(Failure::Status(400, Some("x".repeat(200))).message(), format!("Rewrite failed (400): {}…", "x".repeat(79)));
        assert_eq!(Failure::NoKey.message(), "Add an API key in Settings");
    }
    #[test] fn finish_keeps_selection_whitespace() {
        assert_eq!(finish("  helo wrld\n", "hello world"), "  hello world\n");
        assert_eq!(finish("helo", " hello \n"), "hello");
    }
    /// A clipboard that records what the protocol did. `copies` is what the app puts there on Cmd/Ctrl+C.
    #[derive(Default)]
    struct Fake { secure: bool, ax: Option<String>, copies: Option<String>, user_copies: bool, seq: u64, clip: String, ops: Vec<String>, waited: u64 }
    impl Host for Fake {
        type Snap = String;
        fn is_secure(&mut self) -> bool { self.secure }
        fn selection_via_accessibility(&mut self) -> Option<String> { self.ax.clone() }
        fn clip_seq(&mut self) -> u64 { self.seq }
        fn clip_snapshot(&mut self) -> String { self.clip.clone() }
        fn clip_restore(&mut self, s: String) { self.ops.push(format!("restore {s}")); self.clip = s; self.seq += 1; }
        fn clip_read_text(&mut self) -> Option<String> { Some(self.clip.clone()) }
        fn clip_write_transient(&mut self, t: &str) { self.ops.push(format!("write {t}")); self.clip = t.into(); self.seq += 1; }
        fn post_copy(&mut self) { self.ops.push("copy".into()); if let Some(c) = self.copies.clone() { self.clip = c; self.seq += 1; } }
        fn post_paste(&mut self) { self.ops.push(format!("paste {}", self.clip)); if self.user_copies { self.clip = "user".into(); self.seq += 1; } }
        fn pause(&mut self, ms: u64) { self.waited += ms; }
    }
    fn saved() -> Fake { Fake { clip: "saved".into(), ..Fake::default() } }
    #[test] fn capture_refuses_secure_fields() {
        let mut h = Fake { secure: true, ax: Some("pw".into()), ..saved() };
        assert_eq!(capture(&mut h), Captured::Secure); assert_eq!(h.ops, Vec::<String>::new());
    }
    #[test] fn capture_prefers_accessibility_and_never_copies_an_empty_selection() {
        let mut h = Fake { ax: Some("helo".into()), copies: Some("whole line".into()), ..saved() };
        assert_eq!(capture(&mut h), Captured::Text("helo".into())); assert_eq!(h.ops, Vec::<String>::new());
        let mut h = Fake { ax: Some("".into()), copies: Some("whole line".into()), ..saved() };
        assert_eq!(capture(&mut h), Captured::NoSelection); assert_eq!(h.ops, Vec::<String>::new());
    }
    #[test] fn capture_falls_back_to_copy_and_restores_the_clipboard() {
        let mut h = Fake { copies: Some("helo".into()), ..saved() };
        assert_eq!(capture(&mut h), Captured::Text("helo".into()));
        assert_eq!(h.ops, ["copy", "restore saved"]); assert_eq!(h.clip, "saved");
    }
    #[test] fn capture_gives_up_after_300ms_without_touching_the_clipboard() {
        let mut h = saved();
        assert_eq!(capture(&mut h), Captured::NoSelection);
        assert_eq!(h.ops, ["copy"]); assert_eq!((h.clip.as_str(), h.waited), ("saved", 300));
    }
    #[test] fn paste_restores_the_clipboard() {
        let mut h = saved(); paste(&mut h, "hello");
        assert_eq!(h.ops, ["write hello", "paste hello", "restore saved"]); assert_eq!(h.clip, "saved");
    }
    #[test] fn paste_keeps_a_copy_the_user_made_meanwhile() {
        let mut h = Fake { user_copies: true, ..saved() }; paste(&mut h, "hello");
        assert_eq!(h.ops, ["write hello", "paste hello"]); assert_eq!(h.clip, "user");
    }
    #[test] fn preview_cuts_with_ellipsis() {
        assert_eq!(preview("สวัสดีครับ", 10), "สวัสดีครับ");
        assert_eq!(preview("abcdef", 4), "abc…");
    }
}
