//! Updates from GitHub releases. A background thread checks the latest release at launch and every few hours (and when
//! asked from the tray), downloads this platform's package, checks its ed25519 signature against `PUBLIC_KEY` and stages
//! it. Nothing is installed until the user picks "Restart to update" in the tray: then a helper swaps the app (macOS)
//! or runs the installer silently (Windows) once typeThai has quit, and starts the new version.
//!
//! A copy that cannot replace itself (a dev build, the Windows zip, an app in a folder the user cannot write to) only
//! says that an update exists and opens the release page.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::Duration;

pub const RELEASES_PAGE: &str = "https://github.com/phasu21/typethai/releases";
const LATEST_API: &str = "https://api.github.com/repos/phasu21/typethai/releases/latest";
/// The release workflow signs each package with the matching private key (repository secret `UPDATE_SIGNING_KEY`).
const PUBLIC_KEY: &str = "0367ccc7b1339590e0bc4071f7a564024002359e825ac250b749a6ca8c2a26fe";
const BUNDLE_ID: &str = "io.github.phasu21.typethai";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const FIRST_CHECK: Duration = Duration::from_secs(30);
const EVERY: Duration = Duration::from_secs(6 * 3600);
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
static WAKE: OnceLock<Mutex<mpsc::Sender<()>>> = OnceLock::new();

pub fn status() -> Status { STATUS.lock().unwrap().clone() }
pub fn generation() -> u64 { GEN.load(Ordering::SeqCst) }
fn set(s: Status) { let mut cur = STATUS.lock().unwrap(); if *cur != s { *cur = s; GEN.fetch_add(1, Ordering::SeqCst); } }

/// Start the checker. With `auto` off it only checks when asked from the tray.
pub fn spawn(auto: bool) {
    AUTO.store(auto, Ordering::SeqCst);
    let (tx, rx) = mpsc::channel();
    if WAKE.set(Mutex::new(tx)).is_err() { return }
    let _ = std::thread::Builder::new().name("typethai-update".into()).spawn(move || {
        let mut wait = FIRST_CHECK;
        loop {
            let manual = match rx.recv_timeout(wait) { Ok(()) => true, Err(mpsc::RecvTimeoutError::Timeout) => false, Err(_) => return };
            if manual || AUTO.load(Ordering::SeqCst) { check(manual) }
            wait = EVERY;
        }
    });
}
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
pub fn check_now() { if let Some(tx) = WAKE.get() { let _ = tx.lock().unwrap().send(()); } }

fn check(manual: bool) {
    if matches!(status(), Status::Ready { .. } | Status::Downloading(_)) { return }
    if manual { set(Status::Checking) }
    match find() {
        Ok(Some(next)) => {
            let fallback = Status::Available { version: next.version.clone(), page: next.page.clone() };
            if install_target().is_none() { set(fallback); return }
            set(Status::Downloading(next.version.clone()));
            match download(&next).and_then(|bytes| stage(&next.version, &bytes)) {
                Ok(staged) => set(Status::Ready { version: next.version, staged }),
                Err(e) => { eprintln!("typeThai: update {} not installed: {e}", next.version); set(fallback) }
            }
        }
        Ok(None) => set(if manual { Status::UpToDate } else { Status::Idle }),
        Err(e) => { eprintln!("typeThai: update check failed: {e}"); set(if manual { Status::Failed } else { Status::Idle }) }
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
    // TYPETHAI_UPDATE_URL points the check at another releases/latest-shaped document (testing). Packages are still
    // checked against PUBLIC_KEY.
    let url = std::env::var("TYPETHAI_UPDATE_URL").unwrap_or_else(|_| LATEST_API.into());
    let mut res = crate::rewrite::http_agent(Duration::from_secs(20)).get(&url)
        .header("Accept", "application/vnd.github+json").header("User-Agent", &format!("typeThai/{CURRENT}"))
        .call().map_err(|e| e.to_string())?;
    if res.status().as_u16() != 200 { return Err(format!("HTTP {}", res.status().as_u16())) }
    let body = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    Ok(newer_release(&body, CURRENT))
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

/// Where downloads are unpacked: a folder per version, emptied first.
fn stage_dir(version: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")] let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join("Library/Caches/typethai/update");
    #[cfg(windows)] let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default().join("typethai\\update");
    #[cfg(not(any(windows, target_os = "macos")))] let base = std::env::temp_dir().join("typethai-update");
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join(version);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
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
}
