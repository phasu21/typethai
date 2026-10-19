//! Caret re-reads after a keystroke. The key hook (the macOS event tap, the Windows WH_KEYBOARD_LL) runs before the
//! focused app processes the key, so the caret it reports is the one from before the glyph was laid out: a bubble or
//! preview that follows the caret reads it again a moment later, and twice more for an app that scrolls or wraps the
//! line on the key. Shared by both hooks, so the two platforms follow the caret the same way.
use std::time::{Duration, Instant};

/// First re-read after the key: long enough for the app to lay the glyph out, short enough to read as following along.
const AFTER: Duration = Duration::from_millis(30);
/// Re-reads after the first: an app that scrolls on the key moves the caret again after it.
const EVERY: Duration = Duration::from_millis(70);
/// Re-reads per arming: the first, then `EVERY` apart.
const SAMPLES: u8 = 3;
/// Never re-read faster than this: keys arriving while a sample is still being read arm the follow again, they do not
/// pile queries on the hook thread.
const SPACING: Duration = Duration::from_millis(50);

#[derive(Default)]
pub struct Follow { due: Option<Instant>, left: u8, last: Option<Instant> }

impl Follow {
    /// A key or a click moved the caret, and something is up to be moved with it. Does not restart a run already
    /// armed: that one is still waiting for the app to lay the caret out.
    pub fn arm(&mut self, now: Instant) { if self.due.is_none() { self.left = SAMPLES; self.due = Some(now + AFTER) } }

    /// Time for a re-read now, while `up` (the bubble or the preview is shown). One arming is `SAMPLES` reads; once
    /// nothing is up, the run is spent.
    pub fn take(&mut self, now: Instant, up: bool) -> bool {
        if !up { self.due = None; self.left = 0; return false }
        if !self.due.is_some_and(|t| now >= t) || self.last.is_some_and(|t| now.duration_since(t) < SPACING) { return false }
        self.last = Some(now);
        if self.left > 1 { self.left -= 1; self.due = Some(now + EVERY) } else { self.due = None; self.left = 0 }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `t0` is this run's origin: an Instant plus a millisecond offset.
    fn at(t0: Instant, ms: u64) -> Instant { t0 + Duration::from_millis(ms) }

    #[test] fn one_arming_reads_three_times_after_the_key() {
        let t0 = Instant::now();
        let mut f = Follow::default();
        f.arm(at(t0, 0));
        assert!(!f.take(at(t0, 0), true), "nothing yet: the app has not processed the key");
        assert!(f.take(at(t0, 30), true), "the first read lands on the app's own caret");
        assert!(!f.take(at(t0, 40), true), "samples are spaced out");
        assert!(f.take(at(t0, 100), true));
        assert!(f.take(at(t0, 170), true));
        assert!(!f.take(at(t0, 240), true), "three reads and the run is spent");
    }
    #[test] fn nothing_up_spends_the_run_and_a_later_key_arms_again() {
        let t0 = Instant::now();
        let mut f = Follow::default();
        f.arm(at(t0, 0)); f.arm(at(t0, 5));
        assert!(f.take(at(t0, 30), true));
        assert!(!f.take(at(t0, 100), false), "the bubble went: no re-read");
        assert!(!f.take(at(t0, 200), true), "and the rest of that run went with it");
        f.arm(at(t0, 300));
        assert!(f.take(at(t0, 330), true), "a new key starts a new run");
    }
    #[test] fn keys_do_not_restart_a_run_or_crowd_it() {
        let t0 = Instant::now();
        let mut f = Follow::default();
        f.arm(at(t0, 0)); f.arm(at(t0, 20));
        assert!(f.take(at(t0, 30), true), "the second key did not push the first read out");
        assert!(!f.take(at(t0, 40), true), "not due yet");
        assert!(f.take(at(t0, 100), true));
        assert!(f.take(at(t0, 170), true), "three reads and the run is spent");
        f.arm(at(t0, 175));
        assert!(!f.take(at(t0, 205), true), "a new key right after a read waits for the spacing");
        assert!(f.take(at(t0, 220), true));
    }
}
