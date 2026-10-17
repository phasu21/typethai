//! The input-language badge: the language now typed, over the caret for a moment after the keyboard input source
//! changes (by the user, or by typeThai after a conversion), and for as long as focus sits in a text field nothing has
//! been typed in yet. Pure logic; the platform layers feed it.
use crate::engine::InputLang;
use crate::placement::Anchor;
use std::time::{Duration, Instant};

/// How long the badge stays up.
pub const SHOW_FOR: Duration = Duration::from_millis(1200);

/// Spots the input language changing under the same owner: Windows gives each foreground window's thread a layout of
/// its own, so a new window is not a switch; macOS has one source (owner 0). The first look is not a change either.
#[derive(Default)]
pub struct Watch { last: Option<(u64, bool)> }
impl Watch {
    pub const fn new() -> Watch { Watch { last: None } }
    pub fn see(&mut self, owner: u64, thai: bool) -> Option<InputLang> {
        let was = self.last.replace((owner, thai));
        (was.is_some_and(|(o, t)| o == owner && t != thai)).then_some(if thai { InputLang::Thai } else { InputLang::Latin })
    }
}

/// Where the badge may go: the caret, else a short field, never the mouse (a badge there is noise). None: don't show,
/// also with the setting off, while the bubble is up, or when focus is `blocked`: a password field, or not a text field
/// (the text-field gate). `blocked` and `anchor` are AX / UIA reads, made only when needed.
pub fn anchor_for(on: bool, bubble: bool, blocked: impl FnOnce() -> bool, anchor: impl FnOnce() -> Anchor) -> Option<Anchor> {
    if !on || bubble || blocked() { return None }
    let a = Anchor { mouse: None, ..anchor() };
    crate::placement::source_of(&a, &crate::placement::Opts::default()).is_some().then_some(a)
}

// The pill (logical px): [border][pad][flag][gap][label][pad][border], in a transparent margin for its shadow.
pub const MARGIN: f64 = crate::bridge::SHADOW_M;
pub const PILL_H: f64 = 26.0;
pub const PILL_PX: f64 = 8.0;
pub const FLAG: f64 = 16.0;
pub const GAP: f64 = 6.0;
pub const LABEL_PX: f64 = 13.0;
/// The window's x from the caret's: the flag's centre sits over the caret.
pub const CARET_DX: f64 = -(MARGIN + crate::bridge::BORDER + PILL_PX + FLAG / 2.0);
/// The window for a label `label_w` wide: the pill and the margin around it.
pub fn size(label_w: f64) -> (f64, f64) { (2.0 * (MARGIN + crate::bridge::BORDER + PILL_PX) + FLAG + GAP + label_w, 2.0 * MARGIN + PILL_H) }
/// The window's rect over the caret line (under it only when there is no room above), for a label `label_w` wide:
/// macOS draws its own input-source indicator under the caret.
pub fn place(a: &Anchor, label_w: f64, mons: &[crate::placement::Monitor]) -> Option<crate::placement::Placement> {
    // lower_fraction 0: always "low on the screen", so above whenever it fits
    let o = crate::placement::Opts { gap: 4.0, caret_dx: CARET_DX, inset: MARGIN, lower_fraction: 0.0, ..Default::default() };
    crate::placement::place(&Anchor { mouse: None, ..*a }, size(label_w), mons, &o)
}
/// What the hold indicator says next to its icon: a held Tab opens the ⌃⌥R rows.
pub const HOLD_LABEL: &str = "Rewrite";
/// The hold indicator's progress bar along the pill's bottom, inside its padding.
pub const BAR_H: f64 = 2.0;
pub const BAR_BOTTOM: f64 = 3.0;
/// What the badge says next to the flag.
pub fn label(lang: InputLang) -> &'static str { match lang { InputLang::Thai => "ไทย", InputLang::Latin => "EN" } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd { Show(InputLang), Hold, Hide }
/// UI side: what the badge window shows. The bubble wins: nothing while it shows, and it takes the window down. A
/// held Tab's indicator (`Cmd::Hold`, no bubble up) replaces the language badge until Tab lets go. A *pinned* badge
/// (`pinned`, the language of a text field just focused with nothing typed in it) stays until a key, focus leaving,
/// the bubble or a hold; one shown on an input-source change (`changed`) goes after `SHOW_FOR`.
#[derive(Default)]
pub struct Shown { until: Option<Instant>, pin: bool, bubble: bool, hold: bool }
impl Shown {
    pub fn changed(&mut self, lang: InputLang, now: Instant) -> Option<Cmd> {
        if self.bubble || self.hold { return None }
        self.until = Some(now + SHOW_FOR); Some(Cmd::Show(lang))
    }
    /// Focus landed in a text field with nothing typed yet: show and stay.
    pub fn pinned(&mut self, lang: InputLang) -> Option<Cmd> {
        if self.bubble || self.hold || self.pin { return None }
        self.pin = true; self.until = None; Some(Cmd::Show(lang))
    }
    /// A key was typed, or focus left the text field: a pinned badge goes (a badge on a switch keeps its moment).
    pub fn unpin(&mut self) -> Option<Cmd> { std::mem::take(&mut self.pin).then_some(Cmd::Hide) }
    pub fn bubble(&mut self, visible: bool) -> Option<Cmd> { self.bubble = visible; if visible { self.clicked() } else { None } }
    pub fn tick(&mut self, now: Instant) -> Option<Cmd> { if self.pin { return None } self.until.take_if(|t| now >= *t).map(|_| Cmd::Hide) }
    /// Tab held (`on`) or let go.
    pub fn hold(&mut self, on: bool) -> Option<Cmd> {
        if on && self.bubble { return None }
        if on { self.until = None; self.pin = false; self.hold = true; return Some(Cmd::Hold) }
        std::mem::take(&mut self.hold).then_some(Cmd::Hide)
    }
    /// A click on it (Windows; on macOS clicks pass through).
    pub fn clicked(&mut self) -> Option<Cmd> { let up = self.up(); (self.until, self.pin, self.hold) = (None, false, false); up.then_some(Cmd::Hide) }
    /// Something is up.
    pub fn up(&self) -> bool { self.until.is_some() || self.pin || self.hold }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::placement::Rect;
    use InputLang::{Latin, Thai};
    const CARET: Anchor = Anchor { caret: Some(Rect::new(100.0, 200.0, 1.0, 18.0)), field: Some(Rect::new(60.0, 194.0, 300.0, 30.0)), mouse: Some((5.0, 5.0)) };
    #[test] fn a_switch_under_the_same_owner_is_a_change() {
        let mut w = Watch::default();
        assert_eq!(w.see(0, false), None, "the first look");
        assert_eq!(w.see(0, false), None, "unchanged");
        assert_eq!(w.see(0, true), Some(Thai));
        assert_eq!(w.see(0, true), None);
        assert_eq!(w.see(0, false), Some(Latin));
    }
    #[test] fn a_new_window_with_its_own_layout_is_not_a_switch() {
        let mut w = Watch::default();
        w.see(1, false);
        assert_eq!(w.see(2, true), None, "another window, already Thai");
        assert_eq!(w.see(2, false), Some(Latin), "then switched in it");
        assert_eq!(w.see(1, false), None, "back to the first");
    }
    fn at(on: bool, bubble: bool, blocked: bool, a: Anchor) -> Option<Anchor> { anchor_for(on, bubble, || blocked, || a) }
    #[test] fn the_badge_goes_at_the_caret_or_a_field_never_the_mouse() {
        assert_eq!(at(true, false, false, CARET), Some(Anchor { mouse: None, ..CARET }));
        let field = Anchor { caret: None, ..CARET };
        assert_eq!(at(true, false, false, field), Some(Anchor { mouse: None, ..field }));
        assert_eq!(at(true, false, false, Anchor { caret: None, field: None, mouse: Some((5.0, 5.0)) }), None, "only the mouse");
        let tall = Anchor { caret: None, field: Some(Rect::new(0.0, 0.0, 800.0, 600.0)), mouse: Some((5.0, 5.0)) };
        assert_eq!(at(true, false, false, tall), None, "a document's frame says nothing of the line");
    }
    #[test] fn no_badge_when_off_over_the_bubble_or_outside_a_text_field() {
        assert_eq!(at(false, false, false, CARET), None, "setting off");
        assert_eq!(at(true, true, false, CARET), None, "the bubble is up");
        assert_eq!(at(true, false, true, CARET), None, "a password field, or not a text field");
    }
    #[test] fn off_or_under_the_bubble_nothing_is_looked_up() {
        let never = || -> bool { panic!("looked up") };
        assert_eq!(anchor_for(false, false, never, || -> Anchor { panic!("looked up") }), None);
        assert_eq!(anchor_for(true, true, never, || -> Anchor { panic!("looked up") }), None);
        assert_eq!(anchor_for(true, false, || true, || -> Anchor { panic!("looked up") }), None, "blocked: no caret lookup");
    }
    #[test] fn the_window_fits_the_pill_and_its_shadow() {
        // [margin 10][border 1][pad 8][flag 16][gap 6][label][pad 8][border 1][margin 10], 26 tall + margins
        assert_eq!(size(30.0), (90.0, 46.0));
        assert_eq!(CARET_DX, -27.0, "the flag's centre over the caret");
    }
    #[test] fn the_badge_sits_over_the_caret_clear_of_the_system_indicator() {
        use crate::placement::Monitor;
        let v = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let mons = [Monitor { frame: v, visible: v, size_scale: 1.0, pixel: 2.0 }];
        let high = Anchor { caret: Some(Rect::new(300.0, 120.0, 1.0, 18.0)), field: None, mouse: None };
        let p = place(&high, 30.0, &mons).unwrap();
        assert!(p.above, "macOS draws its indicator under the caret: ours goes over it, even high on the screen");
        // the pill 4 px over the caret, its flag's centre on the caret's x
        assert_eq!((p.rect.bottom() - MARGIN, p.rect.x - CARET_DX), (120.0 - 4.0, 300.0));
        assert_eq!((p.rect.w, p.rect.h), size(30.0));
        let top = Anchor { caret: Some(Rect::new(300.0, 10.0, 1.0, 18.0)), ..high };
        assert!(!place(&top, 30.0, &mons).unwrap().above, "no room above");
        assert_eq!(place(&Anchor { caret: None, field: None, mouse: Some((5.0, 5.0)) }, 30.0, &mons), None, "never at the mouse");
    }
    #[test] fn a_held_tab_replaces_the_language_badge() {
        let (mut s, t) = (Shown::default(), Instant::now());
        s.changed(Thai, t);
        assert_eq!(s.hold(true), Some(Cmd::Hold), "the hold indicator takes the window");
        assert_eq!(s.changed(Latin, t), None, "no language badge over it");
        assert_eq!(s.tick(t + SHOW_FOR), None, "the badge's time no longer runs");
        assert!(s.up());
        assert_eq!(s.hold(false), Some(Cmd::Hide));
        assert_eq!(s.hold(false), None, "already down");
        assert!(!s.up());
        assert_eq!(s.changed(Latin, t), Some(Cmd::Show(Latin)), "badges show again");
    }
    #[test] fn the_bubble_or_a_click_takes_the_hold_indicator_down() {
        let mut s = Shown::default();
        s.hold(true); assert_eq!(s.bubble(true), Some(Cmd::Hide));
        assert_eq!(s.hold(true), None, "the bubble shows its own on the Tab key");
        s.bubble(false); s.hold(true); assert_eq!(s.clicked(), Some(Cmd::Hide)); assert_eq!(s.hold(false), None);
    }
    #[test] fn a_click_takes_the_badge_down() {
        let (mut s, t) = (Shown::default(), Instant::now());
        assert_eq!(s.clicked(), None, "nothing up");
        s.changed(Thai, t);
        assert_eq!(s.clicked(), Some(Cmd::Hide));
        assert_eq!(s.tick(t + SHOW_FOR), None);
    }
    #[test] fn a_pinned_badge_stays_until_a_key_or_focus_leaving() {
        let (mut s, t) = (Shown::default(), Instant::now());
        assert_eq!(s.pinned(Thai), Some(Cmd::Show(Thai)));
        assert_eq!(s.pinned(Thai), None, "already up: no second show");
        assert!(s.up());
        assert_eq!(s.tick(t + SHOW_FOR * 10), None, "a pinned badge has no time of its own");
        assert_eq!(s.unpin(), Some(Cmd::Hide));
        assert_eq!(s.unpin(), None, "already down");
        assert!(!s.up());
    }
    #[test] fn a_switch_while_pinned_shows_the_new_language_and_keeps_it_up() {
        let (mut s, t) = (Shown::default(), Instant::now());
        s.pinned(Thai);
        assert_eq!(s.changed(Latin, t), Some(Cmd::Show(Latin)));
        assert_eq!(s.tick(t + SHOW_FOR), None, "pinned: the switch's own time does not take it down");
        assert_eq!(s.unpin(), Some(Cmd::Hide));
    }
    #[test] fn a_key_leaves_a_switch_badge_its_moment() {
        let (mut s, t) = (Shown::default(), Instant::now());
        s.changed(Thai, t);
        assert_eq!(s.unpin(), None);
        assert_eq!(s.tick(t + SHOW_FOR), Some(Cmd::Hide));
    }
    #[test] fn the_bubble_a_hold_or_a_click_take_a_pinned_badge_down() {
        let mut s = Shown::default();
        s.pinned(Thai);
        assert_eq!(s.bubble(true), Some(Cmd::Hide));
        assert_eq!(s.unpin(), None, "the bubble already took it down");
        s.bubble(false);
        assert_eq!(s.pinned(Thai), Some(Cmd::Show(Thai)), "and it shows again once the bubble goes");
        assert_eq!(s.hold(true), Some(Cmd::Hold));
        assert_eq!(s.pinned(Thai), None, "no pinned badge over the hold indicator");
        assert_eq!(s.unpin(), None);
        s.hold(false);
        s.pinned(Thai);
        assert_eq!(s.clicked(), Some(Cmd::Hide));
        assert_eq!(s.unpin(), None);
    }
    #[test] fn the_badge_shows_at_once_and_hides_after_its_time() {
        let (mut s, t) = (Shown::default(), Instant::now());
        assert_eq!(s.changed(Thai, t), Some(Cmd::Show(Thai)));
        assert_eq!(s.tick(t + SHOW_FOR - Duration::from_millis(1)), None);
        assert_eq!(s.tick(t + SHOW_FOR), Some(Cmd::Hide));
        assert_eq!(s.tick(t + SHOW_FOR * 2), None, "hidden once");
    }
    #[test] fn another_switch_restarts_the_time() {
        let (mut s, t) = (Shown::default(), Instant::now());
        s.changed(Thai, t);
        let t2 = t + Duration::from_millis(800);
        assert_eq!(s.changed(Latin, t2), Some(Cmd::Show(Latin)));
        assert_eq!(s.tick(t + SHOW_FOR), None, "counted from the second switch");
        assert_eq!(s.tick(t2 + SHOW_FOR), Some(Cmd::Hide));
    }
    #[test] fn the_bubble_takes_the_badge_down_and_keeps_it_off() {
        let (mut s, t) = (Shown::default(), Instant::now());
        s.changed(Thai, t);
        assert_eq!(s.bubble(true), Some(Cmd::Hide));
        assert_eq!(s.changed(Latin, t), None, "no badge over the bubble");
        assert_eq!(s.tick(t + SHOW_FOR), None);
        assert_eq!(s.bubble(false), None, "the bubble going does not bring it back");
        assert_eq!(s.changed(Latin, t), Some(Cmd::Show(Latin)));
        assert_eq!(Shown::default().bubble(true), None, "nothing to hide");
    }
}
