//! Updates from GitHub releases. A background thread checks the latest release at launch and every few hours (when the
//! machine wakes from sleep too, and when asked from the tray), downloads this platform's package, checks its ed25519
//! signature against `PUBLIC_KEY` and stages it. Nothing is installed until the user picks "Restart to update" in the
//! tray: then a helper swaps the app (macOS) or runs the installer silently (Windows) once typeThai has quit, and
//! starts the new version.
//!
//! A release the user can act on also raises one system notification (`platform::notify`); a click on it does whatever
//! the tray item would. A copy that cannot replace itself (a dev build, the Windows zip, an app in a folder the user
//! cannot write to) only says that an update exists and opens the release page.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const RELEASES_PAGE: &str = "https://github.com/phasu21/typethai/releases";
/// The newest release, read off a redirect: `releases/latest` answers 302 with the tag in `Location`, so the check
/// never touches the API — unauthenticated api.github.com allows 60 requests an hour per address, and a rate-limited
/// check cannot tell "no update" from "no answer".
const LATEST: &str = "https://github.com/phasu21/typethai/releases/latest";
/// Where every release keeps its package and signature, under the tag.
const DOWNLOADS: &str = "https://github.com/phasu21/typethai/releases/download";
/// The release workflow signs each package with the matching private key (repository secret `UPDATE_SIGNING_KEY`).
const PUBLIC_KEY: &str = "0367ccc7b1339590e0bc4071f7a564024002359e825ac250b749a6ca8c2a26fe";
const BUNDLE_ID: &str = "io.github.phasu21.typethai";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(6 * 3600);
/// A wake from sleep checks only when the last check is at least this old: a laptop wakes many times a day, and an
/// update downloads in the background.
const WAKE_AFTER: Duration = Duration::from_secs(3600);
const MAX_DOWNLOAD: u64 = 256 << 20;

/// What the tray shows. Background checks never show `Checking`, `UpToDate` or `Failed`; a check from the tray does.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    Failed,
    Downloading(String),
    /// Staged and verified: the tray offers to restart into it.
    Ready { version: String, staged: PathBuf },
    /// Newer, but this copy installs it by hand: the tray opens `page`.
    Available { version: String, page: String },
}
impl Status {
    /// The tray item's text, and whether it can be clicked.
    pub fn menu(&self) -> (String, bool) {
        match self {
            Status::Idle => ("Check for updates".into(), true),
            Status::Checking => ("Checking for updates…".into(), false),
            Status::UpToDate => (format!("typeThai {CURRENT} is up to date"), true),
            Status::Failed => ("Update check failed. Try again".into(), true),
            Status::Downloading(v) => (format!("Downloading typeThai {v}…"), false),
            Status::Ready { version, .. } => (format!("Restart to update to {version}"), true),
            Status::Available { version, .. } => (format!("Get typeThai {version}…"), true),
        }
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status::Idle);
/// Bumped on every status change, so the UI loop only touches the tray when something changed.
static GEN: AtomicU64 = AtomicU64::new(0);
static AUTO: AtomicBool = AtomicBool::new(true);
static WAKE: OnceLock<Mutex<mpsc::Sender<Why>>> = OnceLock::new();
/// The version already toasted: one notification per release, whatever the status passes through on the way.
static NOTIFIED: Mutex<Option<String>> = Mutex::new(None);
/// The toast the UI loop still has to post (`platform::notify`).
static TOAST: Mutex<Option<Toast>> = Mutex::new(None);

pub fn status() -> Status { STATUS.lock().unwrap().clone() }
pub fn generation() -> u64 { GEN.load(Ordering::SeqCst) }
fn set(s: Status) {
    let mut cur = STATUS.lock().unwrap();
    if *cur == s { return }
    cur.clone_from(&s);
    GEN.fetch_add(1, Ordering::SeqCst);
    let toast = toast_for(&s).filter(|t| NOTIFIED.lock().unwrap().as_deref() != Some(t.version.as_str()));
    drop(cur);
    if let Some(t) = toast { *NOTIFIED.lock().unwrap() = Some(t.version.clone()); *TOAST.lock().unwrap() = Some(t); }
}

/// What the UI loop posts as a system notification, and what a click on it is worth (the tray item's action).
#[derive(Clone, Debug, PartialEq)]
pub struct Toast { pub title: String, pub body: String, pub version: String }
/// The notification for a status worth interrupting for: an update staged and waiting for a restart, or — for a copy
/// that has to be replaced by hand — a release waiting for its page to be opened. A check that finds nothing stays in
/// the tray, and one version is toasted once (`Ready` and `Available` never both, however the status moves).
fn toast_for(s: &Status) -> Option<Toast> {
    let (version, title, body) = match s {
        Status::Ready { version, .. } => (version, format!("typeThai {version} is ready"), "Click to install it and start typeThai again."),
        Status::Available { version, .. } => (version, format!("typeThai {version} is available"), "Click to open the download page."),
        _ => return None,
    };
    Some(Toast { title, body: body.into(), version: version.clone() })
}
pub fn take_toast() -> Option<Toast> { TOAST.lock().unwrap().take() }

/// What woke the checker: a click on the tray item, or the machine coming back from sleep.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Why { Asked, Woke }

/// Start the checker. With `auto` off it only checks when asked from the tray.
pub fn spawn(auto: bool) {
    AUTO.store(auto, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    if WAKE.set(Mutex::new(tx)).is_err() { return }
    let _ = std::thread::Builder::new().name("typethai-update".into()).spawn(move || {
        let mut next = Instant::now() + FIRST_CHECK;
        let mut last: Option<Instant> = None;
        loop {
            let woke = match rx.recv_timeout(next.saturating_duration_since(Instant::now())) {
                Ok(w) => Some(w),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            };
            let now = Instant::now();
            let due = match woke {
                Some(Why::Asked) => true,
                Some(Why::Woke) => AUTO.load(Ordering::SeqCst) && wake_due(last, now),
                None => AUTO.load(Ordering::SeqCst),
            };
            if due { check(woke == Some(Why::Asked)); last = Some(now) }
            // the periodic check waits a full interval from whichever check just ran; a throttled wake leaves it be
            if due || woke.is_none() { next = now + EVERY }
        }
    });
}
/// Whether a wake-triggered check is due: the first one, or nothing checked for `WAKE_AFTER`.
fn wake_due(last: Option<Instant>, now: Instant) -> bool { last.is_none_or(|t| now.duration_since(t) >= WAKE_AFTER) }

/// Settings: turn the periodic check on or off. A check from the tray works either way.
pub fn set_auto(on: bool) { AUTO.store(on, Ordering::SeqCst) }

/// What a click on the tray item should do now.
pub enum Click { Check, Restart(PathBuf), Open(String) }
pub fn click() -> Click {
    match status() {
        Status::Ready { staged, .. } => Click::Restart(staged),
        Status::Available { page, .. } => Click::Open(page),
        _ => Click::Check,
    }
}
pub fn check_now() { wake(Why::Asked) }
/// Called when the machine comes back from sleep (`platform::watch_wake`): the checker decides whether it is due.
pub fn check_on_wake() { wake(Why::Woke) }
fn wake(w: Why) { if let Some(tx) = WAKE.get() { let _ = tx.lock().unwrap().send(w); } }

/// `--check-update`: run the check now and print what it found, without scheduling anything (no app, no tray).
pub fn print_check() {
    match latest_tag() {
        Ok(Some(tag)) => println!("GitHub's latest release is {tag}; this is typeThai {CURRENT}"),
        Ok(None) => println!("GitHub has no release yet; this is typeThai {CURRENT}"),
        Err(e) => { eprintln!("typeThai: update check failed: {e}"); std::process::exit(1) }
    }
    match find() {
        Ok(Some(next)) => println!("  update: {} ({})", next.version, next.page),
        Ok(None) => println!("  nothing newer to install"),
        Err(e) => { eprintln!("typeThai: update check failed: {e}"); std::process::exit(1) }
    }
}

/// Whether the bubble's rail shows its download chip now. A chip click is the tray item's next step: check (nothing is
/// known yet with the periodic check off, or the last one failed), restart into the staged update, or open the release
/// page. No chip while the checker is already at it (`Checking`), once it has confirmed everything current
/// (`UpToDate`), or while `Idle` with the periodic check on — that is only the moment before the first check, which
/// happens on its own.
pub fn chip() -> bool { chip_needed(&status(), AUTO.load(Ordering::SeqCst)) }
fn chip_needed(s: &Status, auto: bool) -> bool {
    match s { Status::Idle => !auto, Status::Checking | Status::UpToDate => false, _ => true }
}

fn check(manual: bool) {
    let before = status();
    if matches!(before, Status::Downloading(_)) { return }
    // an update already staged keeps its tray item; a newer release found meanwhile replaces it
    let staged = if let Status::Ready { version, .. } = &before { Some(version.clone()) } else { None };
    if manual && staged.is_none() { set(Status::Checking) }
    match find() {
        Ok(Some(next)) if staged.as_ref() == Some(&next.version) => {}
        Ok(Some(next)) => {
            let fallback = if staged.is_some() { before } else { Status::Available { version: next.version.clone(), page: next.page.clone() } };
            if install_target().is_none() { set(fallback); return }
            set(Status::Downloading(next.version.clone()));
            match download(&next).and_then(|bytes| stage(&next.version, &bytes)) {
                Ok(staged) => { prune(&next.version); set(Status::Ready { version: next.version, staged }) }
                Err(e) => { eprintln!("typeThai: update {} not installed: {e}", next.version); set(fallback) }
            }
        }
        Ok(None) => if staged.is_none() { set(if manual { Status::UpToDate } else { Status::Idle }) },
        Err(e) => { eprintln!("typeThai: update check failed: {e}"); if staged.is_none() { set(if manual { Status::Failed } else { Status::Idle }) } }
    }
}

/// A newer release and this platform's package in it.
#[derive(Clone, Debug, PartialEq)]
struct Next { version: String, page: String, package: String, signature: String }

/// The asset this platform installs from.
fn package_name(tag: &str) -> Option<String> {
    if cfg!(target_os = "macos") { Some(format!("typeThai-{tag}-macos-universal.zip")) }
    else if cfg!(windows) { Some(format!("typeThai-{tag}-windows-x86_64-setup.exe")) }
    else { None }
}

fn find() -> Result<Option<Next>, String> {
    // TYPETHAI_UPDATE_URL points the check at another releases/latest-shaped document (testing): a GitHub release as
    // JSON. Packages are still checked against PUBLIC_KEY.
    if let Ok(url) = std::env::var("TYPETHAI_UPDATE_URL") {
        let mut res = crate::rewrite::http_agent(Duration::from_secs(20)).get(&url)
            .header("Accept", "application/vnd.github+json").header("User-Agent", &ua())
            .call().map_err(|e| e.to_string())?;
        if res.status().as_u16() != 200 { return Err(format!("HTTP {}", res.status().as_u16())) }
        let body = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
        return Ok(newer_release(&body, CURRENT));
    }
    match latest_tag()? {
        // no package for this platform (Linux): nothing can be installed, so nothing is offered
        Some(tag) if newer(&tag, CURRENT) => Ok(next_for_tag(&tag)),
        _ => Ok(None),
    }
}
fn ua() -> String { format!("typeThai/{CURRENT}") }

/// The tag `releases/latest` redirects to, asked for without following it. `None` when GitHub has no release yet.
fn latest_tag() -> Result<Option<String>, String> {
    let res = crate::rewrite::http_agent_unfollowed(Duration::from_secs(20)).get(LATEST)
        .header("Accept", "text/html").header("User-Agent", &ua())
        .call().map_err(|e| e.to_string())?;
    let code = res.status().as_u16();
    if code == 404 { return Ok(None) }
    if !(300..400).contains(&code) { return Err(format!("HTTP {code}")) }
    let loc = res.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or_default();
    tag_from_location(loc)
}
/// The tag in a `.../releases/tag/<tag>` redirect target: its last path segment. A redirect anywhere else (a login
/// page, a moved repository) is an answer this check does not understand, so it says so instead of guessing a version.
fn tag_from_location(loc: &str) -> Result<Option<String>, String> {
    match loc.trim_end_matches('/').rsplit('/').next() {
        Some(tag) if tag.starts_with('v') && !tag.contains('-') => Ok(Some(tag.to_string())),
        _ => Err(format!("unexpected redirect to {loc:?}")),
    }
}
/// This platform's package and signature for `tag`: the same names under every release. `None` where typeThai has no
/// installer to run.
fn next_for_tag(tag: &str) -> Option<Next> {
    let name = package_name(tag)?;
    Some(Next {
        version: tag.trim_start_matches('v').into(),
        page: format!("{RELEASES_PAGE}/tag/{tag}"),
        package: format!("{DOWNLOADS}/{tag}/{name}"),
        signature: format!("{DOWNLOADS}/{tag}/{name}.sig"),
    })
}
/// The release in `json` (a GitHub release) when it is newer than `current` and has this platform's package and its
/// `.sig`.
fn newer_release(json: &str, current: &str) -> Option<Next> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) { return None }
    let tag = v["tag_name"].as_str()?;
    if !newer(tag, current) { return None }
    let assets = v["assets"].as_array()?;
    let url = |name: &str| assets.iter().find(|a| a["name"].as_str() == Some(name)).and_then(|a| a["browser_download_url"].as_str()).map(String::from);
    let name = package_name(tag)?;
    Some(Next { version: tag.trim_start_matches('v').into(), page: v["html_url"].as_str().unwrap_or(RELEASES_PAGE).into(), package: url(&name)?, signature: url(&format!("{name}.sig"))? })
}
/// `tag` (v1.2.3) is a later release than `current` (1.2.3). Pre-release tags (v1.2.3-beta) never are.
fn newer(tag: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<Vec<u64>> { let s = s.trim_start_matches('v'); if s.contains('-') { return None } s.split('.').map(|p| p.parse().ok()).collect() };
    match (parse(tag), parse(current)) { (Some(a), Some(b)) => a > b, _ => false }
}

fn download(next: &Next) -> Result<Vec<u8>, String> {
    let agent = crate::rewrite::http_agent(Duration::from_secs(600));
    let get = |url: &str, limit: u64| -> Result<Vec<u8>, String> {
        let mut res = agent.get(url).header("User-Agent", &format!("typeThai/{CURRENT}")).call().map_err(|e| e.to_string())?;
        if res.status().as_u16() != 200 { return Err(format!("HTTP {} for {url}", res.status().as_u16())) }
        res.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| e.to_string())
    };
    let sig = String::from_utf8(get(&next.signature, 1024)?).map_err(|_| "signature is not text".to_string())?;
    let bytes = get(&next.package, MAX_DOWNLOAD)?;
    if !verify(&bytes, &sig, PUBLIC_KEY) { return Err("bad signature".into()) }
    Ok(bytes)
}
/// `sig_hex`: the 64-byte ed25519 signature of `data`, in hex.
fn verify(data: &[u8], sig_hex: &str, key_hex: &str) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};
    let (Some(sig), Some(key)) = (unhex(sig_hex.trim()), unhex(key_hex)) else { return false };
    let (Ok(sig), Ok(key)) = (<[u8; 64]>::try_from(sig.as_slice()), <[u8; 32]>::try_from(key.as_slice())) else { return false };
    VerifyingKey::from_bytes(&key).is_ok_and(|k| k.verify_strict(data, &Signature::from_bytes(&sig)).is_ok())
}
fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 { return None }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

fn stage_base() -> PathBuf {
    #[cfg(target_os = "macos")] { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Caches/typethai/update") }
    #[cfg(windows)] { std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("typethai\\update") }
    #[cfg(not(any(windows, target_os = "macos")))] { std::env::temp_dir().join("typethai-update") }
}
/// Where downloads are unpacked: a folder per version, emptied first. Other versions stay until this one is staged.
fn stage_dir(version: &str) -> Result<PathBuf, String> {
    let dir = stage_base().join(version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}
/// Remove every staged version but `keep`.
fn prune(keep: &str) {
    let Ok(entries) = std::fs::read_dir(stage_base()) else { return };
    for e in entries.flatten() { if e.file_name() != keep { let _ = std::fs::remove_dir_all(e.path()); } }
}

/// Unzip the app and check it is typeThai `version` with an intact signature. Returns the staged `typeThai.app`.
#[cfg(target_os = "macos")]
fn stage(version: &str, zip: &[u8]) -> Result<PathBuf, String> {
    use std::process::Command;
    let dir = stage_dir(version)?;
    let z = dir.join("update.zip");
    std::fs::write(&z, zip).map_err(|e| e.to_string())?;
    let ok = |c: &mut Command| c.status().is_ok_and(|s| s.success());
    if !ok(Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&z).arg(&dir)) { return Err("unzip failed".into()) }
    let _ = std::fs::remove_file(&z);
    let app = dir.join("typeThai.app");
    let plist = app.join("Contents/Info.plist");
    let read = |key: &str| Command::new("/usr/libexec/PlistBuddy").args(["-c", &format!("Print :{key}")]).arg(&plist).output().ok()
        .filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if read("CFBundleIdentifier").as_deref() != Some(BUNDLE_ID) { return Err("not the typeThai app".into()) }
    if read("CFBundleShortVersionString").as_deref() != Some(version) { return Err("version does not match the release".into()) }
    if !ok(Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app)) { return Err("code signature invalid".into()) }
    Ok(app)
}
#[cfg(windows)]
fn stage(version: &str, exe: &[u8]) -> Result<PathBuf, String> {
    let p = stage_dir(version)?.join(format!("typeThai-{version}-setup.exe"));
    std::fs::write(&p, exe).map_err(|e| e.to_string())?;
    Ok(p)
}
#[cfg(not(any(windows, target_os = "macos")))]
fn stage(_: &str, _: &[u8]) -> Result<PathBuf, String> { Err("no installer on this OS".into()) }

/// What the update replaces: the running `.app` (macOS) or the installer's folder (Windows), when this user can write
/// there. None for a dev build, the Windows zip and translocated or read-only apps.
fn install_target() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    #[cfg(target_os = "macos")] {
        let app = exe.parent()?.parent()?.parent()?;
        (app.extension().is_some_and(|e| e == "app") && exe.parent()?.ends_with("Contents/MacOS") && writable(app.parent()?)).then(|| app.to_path_buf())
    }
    #[cfg(windows)] {
        let dir = exe.parent()?;
        (dir.join("unins000.exe").exists() && writable(dir)).then(|| dir.to_path_buf())
    }
    #[cfg(not(any(windows, target_os = "macos")))] { let _ = exe; None }
}
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".typethai-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Start the helper that installs `staged` once this process has exited, and relaunches typeThai. On success the
/// caller quits the app (saving state as on Quit); on failure the tray opens the release page instead.
pub fn restart_into(staged: &Path) -> Result<(), String> {
    let target = install_target().ok_or("this copy of typeThai cannot update itself")?;
    spawn_installer(staged, &target).map_err(|e| e.to_string())
}
#[cfg(target_os = "macos")]
fn spawn_installer(staged: &Path, app: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    // Wait for this process, move the old app aside, move the new one in (or put the old one back), start whichever is
    // there, and remove the staging folder.
    const SWAP: &str = r#"pid=$1 new=$2 dst=$3
while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
old="$dst.old.$$"
if mv "$dst" "$old"; then
  if mv "$new" "$dst"; then rm -rf "$old"; else mv "$old" "$dst"; fi
fi
rm -rf "$(dirname "$new")"
open "$dst""#;
    std::process::Command::new("/bin/sh").args(["-c", SWAP, "typethai-update", &std::process::id().to_string()]).arg(staged).arg(app)
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .process_group(0).spawn().map(|_| ())
}
#[cfg(windows)]
fn spawn_installer(setup: &Path, _dir: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008; const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // the installer closes typeThai if it is still running (CloseApplications=force) and starts it again when silent
    std::process::Command::new(setup).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn().map(|_| ())
}
#[cfg(not(any(windows, target_os = "macos")))]
fn spawn_installer(_: &Path, _: &Path) -> std::io::Result<()> { Err(std::io::ErrorKind::Unsupported.into()) }

/// Open `url` in the default browser.
pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")] { let _ = std::process::Command::new("open").arg(url).spawn(); }
    #[cfg(windows)] { let _ = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn(); }
    #[cfg(not(any(target_os = "macos", windows)))] { eprintln!("typeThai: {url}"); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn compares_versions_numerically_and_skips_prereleases() {
        assert!(newer("v0.10.0", "0.9.9")); assert!(newer("v1.0.0", "0.4.0")); assert!(newer("v0.4.1", "0.4.0"));
        assert!(!newer("v0.4.0", "0.4.0")); assert!(!newer("v0.3.9", "0.4.0"));
        assert!(!newer("v0.5.0-beta", "0.4.0")); assert!(!newer("model-v3", "0.4.0"));
    }
    fn release(tag: &str, names: &[&str]) -> String {
        let assets: Vec<_> = names.iter().map(|n| serde_json::json!({ "name": n, "browser_download_url": format!("https://dl/{n}") })).collect();
        serde_json::json!({ "tag_name": tag, "html_url": format!("https://gh/{tag}"), "draft": false, "prerelease": false, "assets": assets }).to_string()
    }
    #[cfg(any(target_os = "macos", windows))]
    #[test] fn picks_this_platforms_package_and_signature() {
        let pkg = package_name("v0.5.0").unwrap(); let sig = format!("{pkg}.sig");
        let next = newer_release(&release("v0.5.0", &["typethai.ttm", &pkg, &sig]), "0.4.0").expect("newer");
        assert_eq!(next, Next { version: "0.5.0".into(), page: "https://gh/v0.5.0".into(), package: format!("https://dl/{pkg}"), signature: format!("https://dl/{sig}") });
        assert_eq!(newer_release(&release("v0.5.0", &[&pkg]), "0.4.0"), None, "no signature, no update");
        assert_eq!(newer_release(&release("v0.4.0", &[&pkg, &sig]), "0.4.0"), None);
        assert_eq!(newer_release("not json", "0.4.0"), None);
    }
    #[test] fn verifies_ed25519_signatures() {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[7; 32]);
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let (pk, sig) = (hex(key.verifying_key().as_bytes()), hex(&key.sign(b"package").to_bytes()));
        assert!(verify(b"package", &format!("{sig}\n"), &pk));
        assert!(!verify(b"packagf", &sig, &pk));
        assert!(!verify(b"package", &sig, PUBLIC_KEY), "signed with another key");
        assert!(!verify(b"package", "zz", &pk));
    }
    #[test] fn the_public_key_is_a_valid_point() {
        let k = <[u8; 32]>::try_from(unhex(PUBLIC_KEY).unwrap().as_slice()).unwrap();
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&k).is_ok());
    }
    #[test] fn the_chip_shows_when_a_click_can_say_something() {
        // hidden when nothing can come of a click; a click is the tray item's next step otherwise
        assert!(!chip_needed(&Status::Idle, true), "the periodic check is seconds away, on its own");
        assert!(chip_needed(&Status::Idle, false), "no periodic check: the chip is the way to start one");
        assert!(!chip_needed(&Status::Checking, true));
        assert!(!chip_needed(&Status::UpToDate, false));
        assert!(chip_needed(&Status::Failed, true), "a click retries");
        assert!(chip_needed(&Status::Downloading("0.5.15".into()), true));
        assert!(chip_needed(&Status::Ready { version: "0.5.15".into(), staged: PathBuf::new() }, true));
        assert!(chip_needed(&Status::Available { version: "0.5.15".into(), page: "p".into() }, true));
    }
    #[test] fn reads_the_tag_off_the_redirect() {
        let tag = |l: &str| tag_from_location(l).unwrap();
        assert_eq!(tag("https://github.com/phasu21/typethai/releases/tag/v0.5.18").as_deref(), Some("v0.5.18"));
        assert_eq!(tag("https://github.com/phasu21/typethai/releases/tag/v0.5.18/").as_deref(), Some("v0.5.18"));
        // a redirect to anywhere else is an answer this check cannot use, and saying so beats guessing a version
        assert!(tag_from_location("https://github.com/login").is_err());
        assert!(tag_from_location("").is_err(), "no Location at all");
        assert!(tag_from_location("https://github.com/phasu21/typethai/releases/tag/0.5.18").is_err(), "a tag without its v");
    }
    #[cfg(any(target_os = "macos", windows))]
    #[test] fn the_package_urls_follow_the_tag() {
        let next = next_for_tag("v0.5.18").expect("this platform has a package");
        let name = package_name("v0.5.18").unwrap();
        assert_eq!(next.version, "0.5.18");
        assert_eq!(next.page, format!("{RELEASES_PAGE}/tag/v0.5.18"));
        assert_eq!(next.package, format!("{DOWNLOADS}/v0.5.18/{name}"));
        assert_eq!(next.signature, format!("{DOWNLOADS}/v0.5.18/{name}.sig"));
    }
    #[test] fn one_notification_per_version() {
        let ready = |v: &str| Status::Ready { version: v.into(), staged: PathBuf::from("/tmp/typethai-staged") };
        set(ready("9.9.9"));
        assert_eq!(take_toast().map(|t| t.version), Some("9.9.9".into()));
        assert_eq!(take_toast(), None, "posted once");
        set(Status::Downloading("9.9.9".into()));
        set(ready("9.9.9"));
        assert_eq!(take_toast(), None, "the same release is not toasted twice");
        set(ready("9.9.10"));
        assert_eq!(take_toast().map(|t| t.version), Some("9.9.10".into()), "a newer release is a new toast");
    }
    #[test] fn the_toast_says_what_the_click_does() {
        let ready = toast_for(&Status::Ready { version: "0.5.18".into(), staged: PathBuf::new() }).unwrap();
        assert!(ready.title.contains("0.5.18") && ready.body.contains("install"), "{ready:?}");
        let available = toast_for(&Status::Available { version: "0.5.18".into(), page: "p".into() }).unwrap();
        assert!(available.body.contains("download page"), "{available:?}");
        // everything else is the tray's business: a check in flight, or one that found nothing
        assert_eq!(toast_for(&Status::Downloading("0.5.18".into())), None);
        assert_eq!(toast_for(&Status::UpToDate), None);
        assert_eq!(toast_for(&Status::Idle), None);
        assert_eq!(toast_for(&Status::Failed), None);
    }
    #[test] fn a_wake_check_waits_an_hour() {
        let now = Instant::now();
        assert!(wake_due(None, now), "the first wake checks");
        assert!(!wake_due(Some(now - Duration::from_secs(60)), now), "a laptop that slept for a minute is not worth a request");
        assert!(wake_due(Some(now - WAKE_AFTER - Duration::from_secs(1)), now));
    }
}
