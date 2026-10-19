//! What the bubble offered, and which row the user picked: one JSON line per offer in `rank.jsonl`, next to
//! `personal.json`, and the report that reads it back (`--rank-report`).
//!
//! Step one of learning the order rows appear in, and the measurement that says whether there is anything to learn:
//! a bubble whose first row is almost always the one picked has no room for a ranker. Nothing reads the log while you
//! type. It is local, capped in size, off while **Learn from my typing** is off, and cleared by **Forget learned
//! words**.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Offers held in memory at once; a flush comes long before this.
const MAX_PENDING: usize = 512;
/// A flush waits for this many offers before the app writes them out itself (see `App::maybe_save`).
pub const FLUSH_EVERY: usize = 64;
/// Past this many bytes the file is cut to its last half, at startup (some weeks of typing).
pub const MAX_BYTES: u64 = 4 << 20;

/// One row as it stood on the bubble: the text it would apply, and why it was offered (`th`, `en`, `typo`, and none for
/// a completion, a next-word row or an AI row).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub w: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r: Option<String>,
    /// The producing source's own score, in its own domain. Absent in logs written before the trigger, and for rows
    /// no producer could score. A completion row carries its offer's score, not a per-row share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<f32>,
    /// The generator that produced the row, as a string so a tag a future build adds cannot make an older one drop a
    /// whole line. Absent in logs written before the trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s: Option<String>,
}
/// The key that applied a row. Tab and Space are the reflex keys: they apply whatever is highlighted. The others are
/// deliberate, so a rank-0 pick with one of them is real evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    Tab,
    Space,
    Digit,
    Arrow,
    #[default]
    Other,
}
impl How {
    /// Report labels, in `idx` order.
    pub const LABELS: [&'static str; 5] = ["tab", "space", "digit", "arrow", "other"];
    fn idx(self) -> usize {
        match self {
            How::Tab => 0, How::Space => 1, How::Digit => 2, How::Arrow => 3, How::Other => 4,
        }
    }
}
/// The row the user picked, by position on the bubble (0 is the first). The pick is the row that was chosen, which is
/// what a ranker learns from: a guard may still turn down a row that went stale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pick { pub i: usize, pub how: How }
/// One offer, written when it closes: the word it was about, its context, the rows, and the row picked if there was one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Unix seconds.
    pub t: u64,
    /// The app in front, as `prefs::app_key` spells it.
    #[serde(default)]
    pub app: String,
    /// The word the offer was about: the keys in the buffer while it is being typed, else the word just finished.
    pub w: String,
    /// The word before it on the line, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    /// The Thai layout was active.
    #[serde(default)]
    pub rev: bool,
    pub rows: Vec<Row>,
    /// The row the user picked, when one was picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<Pick>,
    /// How long the offer stood.
    #[serde(default)]
    pub ms: u64,
    /// Rows a non-empty candidate set would have shown but the trigger kept off the bubble. Zero for an ordinary
    /// offer; written only when it is not, so a log line of an offer that was shown is unchanged.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub suppressed: u8,
}
fn is_zero(n: &u8) -> bool { *n == 0 }
/// What an offer is about: which word on the line (`line`, `rev`) and its latest text (`w`, `prev`). One word's rows,
/// however many keystrokes refreshed them, are one offer; a change of word closes the offer standing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ctx {
    /// The words committed on the line before it: the word an offer is for is `line` words in, and the buffer being
    /// typed has not reached the line yet.
    pub line: usize,
    /// `Event::w`, as the word reads when the offer closes (a live offer's buffer grows keystroke by keystroke).
    pub w: String,
    /// `Event::prev`.
    pub prev: Option<String>,
    /// `Event::rev`.
    pub rev: bool,
}
/// The offer standing on the bubble.
#[derive(Clone, Debug)]
struct Open {
    ctx: Ctx,
    /// The app it was offered in, as it was then.
    app: String,
    rows: Vec<Row>,
    /// The rows as they were when the row was picked: what the pick's rank is a rank of.
    picked_rows: Option<Vec<Row>>,
    pick: Option<Pick>,
    at: Instant,
}
/// The offers of this run, and the one standing on the bubble.
#[derive(Debug, Default)]
pub struct RankLog {
    /// The app in front (`App::set_front_app`), written with every offer.
    pub app: String,
    open: Option<Open>,
    /// Closed offers, not yet on disk. Bounded: the oldest go first, which only happens if nothing flushes.
    pending: Vec<Event>,
    /// A reset threw the log away: the file goes too.
    pub cleared: bool,
}
impl RankLog {
    /// The bubble now offers `rows` for `ctx`; empty `rows` closes the offer standing there. Rows for the word already
    /// offered — a keystroke refreshing the bubble, or the boundary that finishes it — update it in place: the offer is
    /// the word, not the keystroke. The text kept is the latest, and it is written when the offer closes.
    pub fn offer(&mut self, ctx: Ctx, rows: Vec<Row>) {
        if rows.is_empty() { return self.close() }
        match &mut self.open {
            Some(o) if o.ctx.line == ctx.line && o.ctx.rev == ctx.rev => { o.ctx = ctx; o.rows = rows }
            _ => { self.close(); self.open = Some(Open { ctx, app: self.app.clone(), rows, picked_rows: None, pick: None, at: Instant::now() }) }
        }
    }
    /// The row at `i` was applied. The rows of this moment are kept: later rows are not the ones ranked.
    pub fn picked(&mut self, i: usize, how: How) {
        if let Some(o) = &mut self.open {
            o.picked_rows = Some(o.rows.clone());
            o.pick = Some(Pick { i, how });
        }
    }
    /// The gate kept a non-empty candidate set off the bubble: record what was suppressed, and close the offer that
    /// stood. Without this a suppressed offer is indistinguishable from no offer at all.
    pub fn suppressed(&mut self, ctx: Ctx, n: u8) {
        if n == 0 { return }
        self.close();
        if self.pending.len() >= MAX_PENDING { self.pending.remove(0); }
        self.pending.push(Event { t: unix_now(), app: self.app.clone(), w: ctx.w, prev: ctx.prev, rev: ctx.rev, rows: vec![], pick: None, ms: 0, suppressed: n });
    }
    /// The offer is over (applied, dismissed, or another word started).
    pub fn close(&mut self) {
        let Some(o) = self.open.take() else { return };
        let rows = o.picked_rows.unwrap_or(o.rows);
        if rows.is_empty() { return }
        if self.pending.len() >= MAX_PENDING { self.pending.remove(0); }
        let ms = o.at.elapsed().as_millis() as u64;
        self.pending.push(Event { t: unix_now(), app: o.app, w: o.ctx.w, prev: o.ctx.prev, rev: o.ctx.rev, rows, pick: o.pick, ms, suppressed: 0 });
    }
    /// Forget everything (`Forget learned words`), on disk too.
    pub fn reset(&mut self) { self.open = None; self.pending.clear(); self.cleared = true }
    /// Take the closed offers for the saver.
    pub fn take_pending(&mut self) -> Vec<Event> { std::mem::take(&mut self.pending) }
    pub fn pending(&self) -> usize { self.pending.len() }
    pub fn is_empty(&self) -> bool { self.open.is_none() && self.pending.is_empty() }
}
fn unix_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) }
/// `rank.jsonl`, beside the state file.
pub fn path_beside(state: &Path) -> PathBuf { state.with_file_name("rank.jsonl") }
/// Append offers as JSON lines. A crash mid-line leaves the ones before it readable.
pub fn append(path: &Path, events: &[Event]) -> std::io::Result<()> {
    if events.is_empty() { return Ok(()) }
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    for e in events { if let Ok(l) = serde_json::to_string(e) { let _ = f.write_all(l.as_bytes()); let _ = f.write_all(b"\n"); } }
    Ok(())
}
/// Drop the log file. A missing file is fine.
pub fn remove(path: &Path) { let _ = std::fs::remove_file(path); }
/// The offers in `path`, and how many lines could not be read (a truncated one is normal after a crash).
pub fn load(path: &Path) -> (Vec<Event>, usize) {
    let Ok(s) = std::fs::read_to_string(path) else { return (vec![], 0) };
    let (mut out, mut bad) = (vec![], 0);
    for l in s.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str(l) { Ok(e) => out.push(e), Err(_) => bad += 1 }
    }
    (out, bad)
}
/// Cut the file to its last half when it is over `MAX_BYTES`: whole lines only, through a temp file.
pub fn trim(path: &Path) {
    let Ok(len) = std::fs::metadata(path).map(|m| m.len()) else { return };
    if len <= MAX_BYTES { return }
    let Ok(data) = std::fs::read(path) else { return };
    let half = data.len() / 2;
    let start = data[half..].iter().position(|&b| b == b'\n').map_or(data.len(), |i| half + i + 1);
    let tmp = path.with_extension("jsonl.tmp");
    if std::fs::write(&tmp, &data[start..]).is_ok() { let _ = std::fs::rename(&tmp, path); }
}

/// Counts of up to 4 things: `4+` is the last slot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Buckets([usize; 4]);
impl Buckets {
    fn add(&mut self, i: usize) { self.0[i.min(3)] += 1 }
    fn sum(&self) -> usize { self.0.iter().sum() }
    /// `1st 12 (40.0%), 2nd 3 (10.0%)`, empty slots left out; `none` when there is nothing to count.
    fn render(&self, labels: [&str; 4]) -> String {
        if self.sum() == 0 { return "none".into() }
        (0..4).filter(|&i| self.0[i] > 0)
            .map(|i| format!("{} {} ({:.1}%)", labels[i], self.0[i], 100.0 * self.0[i] as f64 / self.sum() as f64))
            .collect::<Vec<_>>().join(", ")
    }
}
const RANKS: [&str; 4] = ["1st", "2nd", "3rd", "4th+"];
const ROW_COUNTS: [&str; 4] = ["1", "2", "3", "4+"];
/// Per source: how often it was offered (once per event, however many rows it contributed), how often one of its rows
/// was picked, and the mean of the best score it offered in each event. Rows repeat their offer's score, so counting
/// per row would weight an offer by its row count.
#[derive(Debug, Default)]
struct SrcTally { offered: usize, picked: usize, conf_sum: f64, scored: usize }

/// What the report is built from.
#[derive(Debug, Default)]
struct Tally {
    offers: usize,
    picked: usize,
    rows: Buckets,
    rank: Buckets,
    /// The same, over the offers that showed more than one row: the ones a ranker could get wrong.
    contested: usize,
    contested_rank: Buckets,
    how: [usize; 5],
    /// Picks at rank 0, by key.
    first_how: [usize; 5],
    /// Reason tag -> (rows offered, rows picked).
    reason: BTreeMap<String, (usize, usize)>,
    /// Source tag -> counts. `None` in a Row (an old log) is keyed under "-".
    source: BTreeMap<String, SrcTally>,
    /// Offers a non-empty candidate set was kept off the bubble for, and the rows that cost them.
    suppressed: usize,
    suppressed_rows: usize,
}
impl Tally {
    fn add(&mut self, e: &Event) {
        if e.suppressed > 0 { self.suppressed += 1; self.suppressed_rows += e.suppressed as usize; return }
        self.offers += 1;
        self.rows.add(e.rows.len().saturating_sub(1));
        let contested = e.rows.len() > 1;
        if contested { self.contested += 1 }
        // one entry per source in this event: its best score is the confidence its offer was made with
        let mut per: BTreeMap<&str, (bool, Option<f32>)> = BTreeMap::new();
        for (i, r) in e.rows.iter().enumerate() {
            let c = self.reason.entry(r.r.clone().unwrap_or_else(|| "-".into())).or_default();
            c.0 += 1;
            if e.pick.as_ref().is_some_and(|p| p.i == i) { c.1 += 1 }
            let entry = per.entry(r.s.as_deref().unwrap_or("-")).or_default();
            if e.pick.as_ref().is_some_and(|p| p.i == i) { entry.0 = true }
            if let Some(conf) = r.c { entry.1 = Some(entry.1.map_or(conf, |best| best.max(conf))) }
        }
        for (tag, (picked, conf)) in per {
            let t = self.source.entry(tag.to_string()).or_default();
            t.offered += 1;
            if picked { t.picked += 1 }
            if let Some(c) = conf { t.conf_sum += c as f64; t.scored += 1 }
        }
        let Some(p) = &e.pick else { return };
        self.picked += 1;
        self.rank.add(p.i);
        if contested { self.contested_rank.add(p.i) }
        self.how[p.how.idx()] += 1;
        if p.i == 0 { self.first_how[p.how.idx()] += 1 }
    }
    /// The counters, as the report prints them.
    fn render(&self) -> String {
        let mut s = String::new();
        let pct = |n: usize, d: usize| 100.0 * n as f64 / d.max(1) as f64;
        s.push_str(&format!("{} offers, {} picked ({:.1}%)\n", self.offers, self.picked, pct(self.picked, self.offers)));
        s.push_str(&format!("rows on the bubble: {}\n", self.rows.render(ROW_COUNTS)));
        s.push_str(&format!("row picked:         {}\n", self.rank.render(RANKS)));
        if self.contested > 0 {
            s.push_str(&format!("  of the {} offers that showed 2 or more rows: {}\n", self.contested, self.contested_rank.render(RANKS)));
            s.push_str(&format!("  the first row was picked in {:.1}% of those\n", pct(self.contested_rank.0[0], self.contested)));
        }
        if self.first_how.iter().sum::<usize>() > 0 {
            s.push_str(&format!("picked with:        {}\n", counts(&self.how)));
            let reflex: usize = [How::Tab, How::Space].into_iter().map(|h| self.first_how[h.idx()]).sum();
            s.push_str(&format!("in first place, by key: {} — {:.1}% of them by a reflex key\n", counts(&self.first_how), pct(reflex, self.first_how.iter().sum())));
        }
        if self.suppressed > 0 {
            s.push_str(&format!("{} offer(s) suppressed by the trigger: {} row(s) kept off the bubble\n", self.suppressed, self.suppressed_rows));
        }
        if self.reason.is_empty() { return s }
        s.push_str("reason    offered   picked   rate\n");
        for (tag, (offered, picked)) in &self.reason {
            s.push_str(&format!("{tag:<10}{offered:>8}{picked:>9}{:>7.1}%\n", pct(*picked, *offered)));
        }
        if self.source.is_empty() { return s }
        s.push_str("source       offered   picked    rate  mean conf\n");
        for (tag, t) in &self.source {
            let mean = if t.scored > 0 { format!("{:.2}", t.conf_sum / t.scored as f64) } else { "-".into() };
            s.push_str(&format!("{tag:<12}{:>8}{:>9}{:>7.1}%{mean:>11}\n", t.offered, t.picked, pct(t.picked, t.offered)));
        }
        s
    }
}
/// `tab 12, digit 3`, empty counters left out.
fn counts(v: &[usize; 5]) -> String {
    (0..5).filter(|&i| v[i] > 0).map(|i| format!("{} {}", How::LABELS[i], v[i])).collect::<Vec<_>>().join(", ")
}
/// The offline measurement over the offers in `events`.
pub fn report(events: &[Event]) -> String {
    let mut t = Tally::default();
    for e in events { t.add(e) }
    t.render()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx(w: &str) -> Ctx { Ctx { line: 0, w: w.into(), prev: None, rev: false } }
    fn rows(w: &str) -> Vec<Row> {
        vec![Row { w: w.into(), r: Some("th".into()), c: None, s: None }, Row { w: "b".into(), r: Some("typo".into()), c: None, s: None }]
    }
    fn event(w: &str, pick: Option<(usize, How)>) -> Event {
        let mut l = RankLog::default();
        l.offer(ctx(w), rows(w));
        if let Some((i, how)) = pick { l.picked(i, how) }
        l.close();
        l.pending.pop().unwrap()
    }

    #[test]
    fn keystrokes_of_one_word_are_one_offer() {
        let mut l = RankLog::default();
        for _ in 0..3 { l.offer(ctx("l;y"), rows("สวัสดี")) }
        assert_eq!(l.pending(), 0, "nothing is written while the offer stands");
        l.offer(ctx("l;y"), vec![]);
        assert_eq!(l.pending(), 1, "the empty offer closes it once");
        assert_eq!(l.take_pending().len(), 1);
        assert_eq!(l.pending(), 0);
    }
    #[test]
    fn a_longer_buffer_is_the_same_offer() {
        let mut l = RankLog::default();
        l.offer(ctx("l;y"), rows("สวัสดี"));
        l.offer(Ctx { w: "l;ylfu".into(), ..ctx("l;y") }, rows("สวัสดี"));
        l.close();
        assert_eq!((l.pending(), l.pending[0].w.as_str()), (1, "l;ylfu"), "one word, and the word it finished as");
    }
    #[test]
    fn another_word_closes_the_offer_and_keeps_its_word() {
        let mut l = RankLog::default();
        l.offer(ctx("l;y"), rows("สวัสดี"));
        l.offer(Ctx { line: 1, ..ctx("ihbjk") }, rows("ครับ"));
        let closed = l.pending.pop().unwrap();
        assert_eq!((closed.w.as_str(), closed.rows.len()), ("l;y", 2));
        l.close();
        assert_eq!(l.pending.pop().unwrap().w, "ihbjk", "the second offer is still standing until it closes");
    }
    #[test]
    fn a_pick_is_written_with_its_rank_and_key() {
        let e = event("l;y", Some((1, How::Digit)));
        assert_eq!(e.pick, Some(Pick { i: 1, how: How::Digit }));
        assert_eq!((e.w.as_str(), e.rows.len()), ("l;y", 2));
    }
    #[test]
    fn the_rank_is_a_rank_of_the_rows_at_the_pick() {
        let mut l = RankLog::default();
        l.offer(ctx("l;y"), rows("สวัสดี"));
        l.picked(1, How::Tab);
        // the bubble is refreshed before it closes: the pick still refers to the rows the user saw
        l.offer(ctx("l;y"), vec![Row { w: "สวัสดีครับ".into(), r: None, c: None, s: None }]);
        l.close();
        let e = &l.pending[0];
        assert_eq!((e.rows.len(), e.pick.clone().unwrap().i), (2, 1));
    }
    #[test]
    fn nothing_is_written_without_an_offer() {
        let mut l = RankLog::default();
        l.picked(0, How::Tab);
        l.close();
        assert!(l.pending.is_empty(), "a key applied with no bubble standing is not an offer");
        assert!(l.is_empty());
    }
    #[test]
    fn an_offer_of_no_rows_is_not_an_event() {
        let mut l = RankLog::default();
        l.offer(ctx("l;y"), vec![]);
        l.close();
        assert_eq!(l.pending(), 0);
    }
    #[test]
    fn a_reset_throws_the_log_away() {
        let mut l = RankLog::default();
        l.offer(ctx("l;y"), rows("สวัสดี"));
        l.close();
        l.reset();
        assert_eq!((l.pending(), l.cleared, l.is_empty()), (0, true, true));
    }
    #[test]
    fn the_log_stays_bounded() {
        let mut l = RankLog::default();
        for i in 0..MAX_PENDING + 10 { l.offer(Ctx { line: i, ..ctx("w") }, rows("a")); l.close() }
        assert_eq!(l.pending(), MAX_PENDING, "the oldest offers go, so memory cannot run away");
    }

    #[test]
    fn the_report_separates_picks_with_a_choice_from_reflex_ones() {
        let one = |w: &str| { let mut e = event(w, Some((0, How::Tab))); e.rows.truncate(1); e };
        let events = [
            one("a"),
            event("b", Some((0, How::Tab))),
            event("c", Some((1, How::Digit))),
            event("d", Some((0, How::Arrow))),
            event("e", None),
        ];
        let r = report(&events);
        assert!(r.contains("5 offers, 4 picked (80.0%)"), "{r}");
        assert!(r.contains("rows on the bubble: 1 1 (20.0%), 2 4 (80.0%)"), "{r}");
        assert!(r.contains("row picked:         1st 3 (75.0%), 2nd 1 (25.0%)"), "{r}");
        assert!(r.contains("2 or more rows: 1st 2 (66.7%), 2nd 1 (33.3%)"), "{r}");
        assert!(r.contains("the first row was picked in 50.0% of those"), "{r}");
        assert!(r.contains("picked with:        tab 2, digit 1, arrow 1"), "{r}");
        assert!(r.contains("in first place, by key: tab 2, arrow 1 — 66.7% of them by a reflex key"), "{r}");
        assert!(r.contains("th               5        3   60.0%"), "{r}");
        assert!(r.contains("typo             4        1   25.0%"), "{r}");
    }
    #[test]
    fn the_report_of_an_empty_log_says_so_in_zeroes() {
        let r = report(&[]);
        assert!(r.contains("0 offers, 0 picked (0.0%)"), "{r}");
        assert!(r.contains("rows on the bubble: none"), "{r}");
    }

    #[test]
    fn the_log_round_trips_through_the_file() {
        let dir = std::env::temp_dir().join(format!("typethai-rank-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("rank.jsonl");
        let a = event("l;y", Some((1, How::Digit)));
        let b = event("ihbjk", None);
        append(&path, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(load(&path), (vec![a, b], 0));
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"w\":\"trunc").unwrap();
        let (got, bad) = load(&path);
        assert_eq!((got.len(), bad), (2, 1), "a half-written line is counted, not fatal");
        remove(&path);
        assert!(load(&path).0.is_empty(), "a removed log reads as empty, not as an error");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn trim_drops_whole_lines_from_the_front() {
        let dir = std::env::temp_dir().join(format!("typethai-rank-trim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("rank.jsonl");
        std::fs::create_dir_all(&dir).unwrap();
        let chunk = format!("{}\n", serde_json::to_string(&event("x", None)).unwrap());
        let n = MAX_BYTES as usize / chunk.len() + 10;
        std::fs::write(&path, chunk.repeat(n)).unwrap();
        trim(&path);
        let (events, bad) = load(&path);
        assert!(std::fs::metadata(&path).unwrap().len() < MAX_BYTES, "the file came down to its last half");
        assert_eq!(bad, 0, "the cut lands on a line boundary");
        assert!(events.len() > n / 3, "most of the last half is kept: {} of {n}", events.len());
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn old_lines_load_and_new_ones_round_trip() {
        // a line written before `c`, `s` and `suppressed` existed
        let old: Event = serde_json::from_str(r#"{"t":1,"app":"Editor","w":"hel","rev":false,"rows":[{"w":"help","r":"th"}],"pick":{"i":0,"how":"tab"},"ms":5}"#).unwrap();
        assert_eq!(old.rows[0].c, None);
        assert_eq!(old.rows[0].s, None);
        assert_eq!(old.suppressed, 0);
        // a tag this build does not know is still a string, so the line survives
        let future: Event = serde_json::from_str(r#"{"t":1,"w":"hel","rows":[{"w":"help","c":0.4,"s":"sentence"}],"suppressed":2}"#).unwrap();
        assert_eq!(future.rows[0].s.as_deref(), Some("sentence"));
        assert_eq!(future.suppressed, 2);
        let now = Event { t: 2, app: "Editor".into(), w: "hel".into(), prev: None, rev: false,
            rows: vec![Row { w: "help".into(), r: None, c: Some(0.5), s: Some("completion".into()) }], pick: None, ms: 3, suppressed: 0 };
        let line = serde_json::to_string(&now).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), now);
        assert!(!line.contains("suppressed"), "an ordinary offer keeps the old shape: {line}");
    }
    #[test]
    fn the_report_averages_confidence_per_offer_not_per_row() {
        let mut one = event("a", None);
        one.rows = vec![
            Row { w: "help".into(), r: None, c: Some(0.6), s: Some("completion".into()) },
            Row { w: "hello".into(), r: None, c: Some(0.6), s: Some("completion".into()) },
            Row { w: "helmet".into(), r: None, c: Some(0.6), s: Some("completion".into()) },
            Row { w: "can".into(), r: None, c: Some(0.2), s: Some("next_word".into()) },
        ];
        let mut two = event("b", Some((0, How::Tab)));
        two.rows = vec![
            Row { w: "help".into(), r: None, c: Some(0.7), s: Some("completion".into()) },
            Row { w: "hello".into(), r: None, c: Some(0.7), s: Some("completion".into()) },
        ];
        let r = report(&[one, two]);
        assert!(r.contains("source       offered   picked    rate  mean conf"), "{r}");
        assert!(r.contains("completion         2        1   50.0%       0.65"), "one offer per event, best score each: {r}");
        assert!(r.contains("next_word          1        0    0.0%       0.20"), "{r}");
    }
    #[test]
    fn a_suppressed_offer_is_recorded() {
        let mut l = RankLog::default();
        l.offer(ctx("hel"), rows("help"));
        l.suppressed(ctx("hello"), 3);
        assert_eq!((l.pending(), l.pending[0].w.as_str(), l.pending[0].rows.len()), (2, "hel", 2), "the offer that stood is closed as it was");
        let s = &l.pending[1];
        assert_eq!((s.w.as_str(), s.suppressed, s.rows.len()), ("hello", 3, 0));
        let r = report(&l.pending);
        assert!(r.contains("1 offer(s) suppressed by the trigger: 3 row(s) kept off the bubble"), "{r}");
    }
}
