//! Local completion setup: find Ollama, start it, and download the configured model, for the Settings button.
//! The endpoint is the one `completion` posts to; only the executable and the tag are looked up here.
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const URL: &str = "http://127.0.0.1:11434";
const TIMEOUT: Duration = Duration::from_secs(3);
const START_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(500);

/// What the Settings row reports, and what its button has to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step { Install, Start, Pull, Ready }

pub fn describe(step: Step, tag: &str) -> String {
    match step {
        Step::Install => "Ollama is not installed".into(),
        Step::Start => "Ollama is installed but not running".into(),
        Step::Pull => format!("{tag} is not downloaded yet"),
        Step::Ready => format!("{tag} is ready"),
    }
}

/// The next step for `tag`, from the machine's current state.
pub fn plan(tag: &str, binary: Option<PathBuf>, server_up: bool, models: Option<&Value>) -> Step {
    match (binary, server_up) {
        (None, _) => Step::Install,
        (Some(_), false) => Step::Start,
        (Some(_), true) if models.is_some_and(|m| has_tag(m, tag)) => Step::Ready,
        (Some(_), true) => Step::Pull,
    }
}

pub fn check(tag: &str) -> Step { plan(tag, binary(), server_up(), models().as_ref()) }

/// `/api/tags` holds the installed names; Ollama answers `gemma4` for a bare name it stores as `gemma4:latest`.
pub fn has_tag(tags: &Value, tag: &str) -> bool {
    let want = if tag.contains(':') { tag.to_string() } else { format!("{tag}:latest") };
    tags.get("models").and_then(Value::as_array).is_some_and(|list| list.iter().any(|m| {
        m.get("name").and_then(Value::as_str).is_some_and(|name| name == want || name == tag)
    }))
}

/// `ollama` on PATH, else where the installer puts it.
pub fn binary() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ollama.exe" } else { "ollama" };
    let on_path = std::env::var_os("PATH").and_then(|path| std::env::split_paths(&path).map(|dir| dir.join(name)).find(|c| c.is_file()));
    on_path.or_else(|| known_locations().into_iter().find(|p| p.is_file()))
}

fn known_locations() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    { vec!["/opt/homebrew/bin/ollama".into(), "/usr/local/bin/ollama".into(), "/Applications/Ollama.app/Contents/Resources/ollama".into()] }
    #[cfg(windows)]
    { let mut out = vec![];
      for key in ["LOCALAPPDATA", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
          if let Some(dir) = std::env::var_os(key) {
              for sub in ["Programs/Ollama", "Ollama"] { out.push(PathBuf::from(&dir).join(sub).join("ollama.exe")) }
          }
      }
      out }
    #[cfg(not(any(target_os = "macos", windows)))]
    { Vec::new() }
}

fn api_get(path: &str) -> Option<Value> {
    let agent = crate::rewrite::http_agent(TIMEOUT);
    let mut response = agent.get(format!("{URL}{path}")).config().proxy(None).max_redirects(0).build().call().ok()?;
    if !(200..=299).contains(&response.status().as_u16()) { return None }
    let text = response.body_mut().with_config().limit(1 << 20).read_to_string().ok()?;
    serde_json::from_str(&text).ok()
}

pub fn server_up() -> bool { api_get("/api/version").is_some() }
pub fn models() -> Option<Value> { api_get("/api/tags") }

/// Start the server the way this platform's users expect to be able to see and quit it.
pub fn start(binary: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if Path::new("/Applications/Ollama.app").exists() {
        return Command::new("open").args(["-a", "Ollama"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().map(|_| ()).map_err(|e| format!("could not open Ollama: {e}"));
    }
    let mut command = Command::new(binary);
    command.arg("serve").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // never flash a console window on Windows
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
    command.spawn().map(|_| ()).map_err(|e| format!("could not start Ollama: {e}"))
}

fn wait_up() -> bool {
    let deadline = std::time::Instant::now() + START_WAIT;
    loop {
        if server_up() { return true }
        if std::time::Instant::now() >= deadline { return false }
        std::thread::sleep(POLL);
    }
}

/// `ollama pull`, with its progress lines (stderr, carriage-return repainted) handed to `log` one per call.
pub fn pull(binary: &Path, tag: &str, log: &mut dyn FnMut(&str)) -> Result<(), String> {
    let mut child = Command::new(binary).args(["pull", tag]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("could not run ollama pull: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    let pipes: Vec<Box<dyn std::io::Read + Send>> = [child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
                                                     child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>)].into_iter().flatten().collect();
    let readers: Vec<_> = pipes.into_iter().map(|pipe| {
        let tx = tx.clone();
        std::thread::spawn(move || for line in std::io::BufRead::lines(std::io::BufReader::new(pipe)).map_while(Result::ok) {
            let _ = tx.send(line);
        })
    }).collect();
    drop(tx);
    while let Ok(line) = rx.recv() {
        let line = line.rsplit('\r').next().unwrap_or("").trim();
        if !line.is_empty() { log(line) }
    }
    for reader in readers { let _ = reader.join(); }
    let status = child.wait().map_err(|e| e.to_string())?;
    if status.success() { Ok(()) } else { Err(format!("ollama pull failed ({status})")) }
}

/// The percentage in an `ollama pull` line, when it has one ("pulling 6d0b3f1c2a44: 42% ▕██░░▏ 4.0 GB/9.6 GB").
pub fn pull_percent(line: &str) -> Option<u32> {
    let before = line.split_once('%')?.0;
    before.rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

pub fn open_download_page() {
    let command = if cfg!(windows) { ("cmd", vec!["/C", "start", "", "https://ollama.com/download"]) } else { ("open", vec!["https://ollama.com/download"]) };
    let _ = Command::new(command.0).args(command.1).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Everything the feature needs: Ollama running, and the configured tag downloaded. Returns the line to show.
pub fn ensure(tag: &str, log: &mut dyn FnMut(&str)) -> Result<String, String> {
    let Some(binary) = binary() else { return Err("Ollama is not installed — press Get Ollama first".into()) };
    if !server_up() {
        log("Starting Ollama…");
        start(&binary)?;
        if !wait_up() { return Err("Ollama did not start within 20 seconds".into()) }
        log("Ollama is running");
    }
    if models().as_ref().is_some_and(|m| has_tag(m, tag)) { return Ok(describe(Step::Ready, tag)) }
    log(&format!("Downloading {tag} — this can take a while"));
    pull(&binary, tag, log)?;
    if models().as_ref().is_some_and(|m| has_tag(m, tag)) { Ok(describe(Step::Ready, tag)) } else { Err(format!("{tag} is still missing after the download")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test] fn a_tag_matches_its_stored_name_or_the_latest_suffix() {
        let tags = json!({ "models": [{ "name": "gemma4:latest" }, { "name": "hf.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf:Q4_K_M" }] });
        assert!(has_tag(&tags, "gemma4:latest"));
        assert!(has_tag(&tags, "gemma4"), "Ollama stores a bare name as :latest");
        assert!(has_tag(&tags, "hf.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf:Q4_K_M"));
        assert!(!has_tag(&tags, "gemma4:2b"), "a different size is a different tag");
        assert!(!has_tag(&tags, "llama3"));
        assert!(!has_tag(&json!({}), "gemma4:latest"));
        assert!(!has_tag(&json!({ "models": "nope" }), "gemma4:latest"));
    }

    #[test] fn the_next_step_names_what_is_missing() {
        let binary = || Some(PathBuf::from("/usr/local/bin/ollama"));
        assert_eq!(plan("m:1", None, true, None), Step::Install);
        assert_eq!(plan("m:1", binary(), false, Some(&json!({ "models": [{ "name": "m:1" }] }))), Step::Start, "a missing server wins over a downloaded model");
        assert_eq!(plan("m:1", binary(), true, None), Step::Pull);
        assert_eq!(plan("m:1", binary(), true, Some(&json!({ "models": [{ "name": "m:1" }] }))), Step::Ready);
        assert_eq!(describe(Step::Ready, "m:1"), "m:1 is ready");
        assert_eq!(describe(Step::Pull, "m:1"), "m:1 is not downloaded yet");
        assert_eq!(describe(Step::Install, "m:1"), "Ollama is not installed");
        assert_eq!(describe(Step::Start, "m:1"), "Ollama is installed but not running");
    }

    #[test] fn the_percentage_comes_out_of_a_progress_line() {
        assert_eq!(pull_percent("pulling 6d0b3f1c2a44: 42% ▕███████░░░░░░░░░░░░▏ 4.0 GB/9.6 GB"), Some(42));
        assert_eq!(pull_percent("pulling 2af3b8d5c1e4: 100% ▕████████████████████▏ 9.6 GB"), Some(100));
        for line in ["pulling manifest", "verifying sha256 digest", "writing manifest", "success", "%"] {
            assert_eq!(pull_percent(line), None, "{line}");
        }
    }

    /// The machine's real Ollama. `cargo test -p typethai-desktop local_setup_live -- --ignored --nocapture`.
    /// Pulls a tag that is already installed, so it only re-checks the manifest.
    #[test] #[ignore]
    fn local_setup_live() {
        let found = binary();
        println!("binary: {found:?}");
        println!("server up: {}", server_up());
        let names = |m: &Value| m["models"].as_array().map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default();
        let installed = models().unwrap_or(Value::Null);
        println!("installed: {:?}", names(&installed));
        let tag = std::env::var("TYPETHAI_LOCAL_TAG").ok().filter(|t| !t.is_empty())
            .or_else(|| names(&installed).first().cloned()).unwrap_or_default();
        let Some(binary) = found.filter(|_| !tag.is_empty()) else { println!("SKIP: no Ollama binary or no installed model"); return };
        println!("plan: {:?}", check(&tag));
        let mut log = |line: &str| println!("  {line}");
        println!("pull {tag}: {:?}", pull(&binary, &tag, &mut log));
        println!("ready: {}", describe(check(&tag), &tag));
    }
}
