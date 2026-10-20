//! What's new: the release notes this build was made with (`.github/release-notes.md`, the same text the GitHub release
//! shows), for the Settings Updates page and for a window shown once after an update.
use std::path::Path;

const NOTES: &str = include_str!("../../../../.github/release-notes.md");
/// The version this copy last ran as, beside prefs.json.
const SEEN: &str = "version.txt";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release { pub version: String, pub new: Vec<String>, pub fixed: Vec<String> }

/// Every release in the notes, newest first.
pub fn releases() -> Vec<Release> { parse(NOTES) }

/// `**New in X**` / `**Fixed in X**` headings, each followed by `- ` bullets whose continuation lines are indented.
fn parse(md: &str) -> Vec<Release> {
    let mut out: Vec<Release> = vec![];
    // the list bullets go to: (release, fixed)
    let mut list: Option<(usize, bool)> = None;
    for line in md.lines() {
        let t = line.trim();
        if let Some(head) = t.strip_prefix("**").and_then(|t| t.strip_suffix("**")) {
            list = None;
            let (fixed, version) = match (head.strip_prefix("New in "), head.strip_prefix("Fixed in ")) {
                (Some(v), _) => (false, v), (_, Some(v)) => (true, v), _ => continue,
            };
            let i = match out.iter().position(|r| r.version == version) {
                Some(i) => i,
                None => { out.push(Release { version: version.to_string(), ..Release::default() }); out.len() - 1 }
            };
            list = Some((i, fixed));
        } else if let (Some((i, fixed)), Some(item)) = (list, line.strip_prefix("- ")) {
            let r = &mut out[i];
            if fixed { r.fixed.push(item.trim().to_string()) } else { r.new.push(item.trim().to_string()) }
        } else if let (Some((i, fixed)), true) = (list, line.starts_with("  ") && !t.is_empty()) {
            let r = &mut out[i];
            if let Some(last) = if fixed { r.fixed.last_mut() } else { r.new.last_mut() } { last.push(' '); last.push_str(t) }
        } else if !t.is_empty() {
            list = None;
        }
    }
    out
}

/// The releases after `seen`, up to `current`; only `current`'s when `seen` is unknown.
pub fn since(all: &[Release], seen: Option<&str>, current: &str) -> Vec<Release> {
    all.iter().filter(|r| match seen {
        Some(s) => crate::update::newer(&r.version, s) && !crate::update::newer(&r.version, current),
        None => r.version == current,
    }).cloned().collect()
}

/// What to show once at launch, and this build remembered as seen (in `dir`): the releases since the version that ran
/// last, after an update. Nothing on a first run (`first_run`: no prefs yet), on a relaunch of the same version, or
/// after a downgrade. A copy from before this file existed has notes for this version only.
pub fn after_update(dir: &Path, first_run: bool) -> Vec<Release> {
    let path = dir.join(SEEN);
    let seen = std::fs::read_to_string(&path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let current = crate::update::CURRENT;
    if seen.as_deref() == Some(current) { return vec![] }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(&path, current);
    if first_run { return vec![] }
    since(&releases(), seen.as_deref(), current)
}

/// A bullet's text in runs: (text, bold, code). `**bold**` and `` `code` `` are kept as styles, `*emphasis*` marks go.
pub fn runs(text: &str) -> Vec<(String, bool, bool)> {
    let (mut out, mut cur, mut bold, mut code) = (vec![], String::new(), false, false);
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        let flip = match c {
            '`' => Some((bold, !code)),
            '*' if !code && it.peek() == Some(&'*') => { it.next(); Some((!bold, code)) }
            '*' if !code => Some((bold, code)),
            _ => None,
        };
        match flip {
            Some(next) => { if !cur.is_empty() { out.push((std::mem::take(&mut cur), bold, code)) } (bold, code) = next; }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push((cur, bold, code)) }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    const MD: &str = "**typeThai** fixes Thai.\n\n**Fixed in 0.5.11**\n- Windows: the mouse no longer stutters, in games\n  above all.\n\n**New in 0.5.10**\n- **Next-word prediction.** Settings → *Predict*.\n- Two\n\n**Fixed in 0.5.10**\n- The flag `ไทย` is placed right.\n";
    #[test] fn parses_releases_newest_first() {
        let r = parse(MD);
        assert_eq!(r.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["0.5.11", "0.5.10"]);
        assert_eq!(r[0].fixed, ["Windows: the mouse no longer stutters, in games above all."]);
        assert_eq!(r[1].new.len(), 2); assert_eq!(r[1].fixed, ["The flag `ไทย` is placed right."]);
        assert!(!releases().is_empty(), "the embedded notes parse");
    }
    #[test] fn since_takes_the_versions_after_the_last_seen() {
        let r = parse(MD);
        let v = |s: Vec<Release>| s.into_iter().map(|r| r.version).collect::<Vec<_>>();
        assert_eq!(v(since(&r, Some("0.5.9"), "0.5.11")), ["0.5.11", "0.5.10"]);
        assert_eq!(v(since(&r, Some("0.5.10"), "0.5.11")), ["0.5.11"]);
        assert_eq!(v(since(&r, None, "0.5.10")), ["0.5.10"]);
        assert!(since(&r, Some("0.5.11"), "0.5.10").is_empty(), "a downgrade shows nothing");
    }
    #[test] fn after_update_shows_once_and_never_on_a_first_run() {
        let dir = std::env::temp_dir().join(format!("typethai-whatsnew-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(after_update(&dir, true).is_empty(), "first run");
        assert!(after_update(&dir, false).is_empty(), "same version again");
        std::fs::write(dir.join(SEEN), "0.0.1").unwrap();
        assert!(!after_update(&dir, false).is_empty(), "after an update");
        assert!(after_update(&dir, false).is_empty(), "only once");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test] fn runs_keep_bold_and_code() {
        assert_eq!(runs("**Next.** Use `ok` in *Settings*"), [("Next.".into(), true, false), (" Use ".into(), false, false), ("ok".into(), false, true), (" in ".into(), false, false), ("Settings".into(), false, false)]);
    }
}
