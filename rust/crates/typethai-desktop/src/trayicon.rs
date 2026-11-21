//! The menu-bar / tray icon is a live input-language indicator: two cards, the white ท card in front while the input
//! source is Thai, the orange T card in front otherwise. Paused draws the same cards in greys. "Plain" (macOS only) is
//! the black template version, which macOS colours for the menu bar. On a change the cards swap through two
//! in-between frames (about 150 ms in all), unless motion is reduced, typeThai is paused, or the last swap was only a
//! moment ago (then the icon just changes). Pure: the platform tray sets the image this picks.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
use std::time::{Duration, Instant};

/// How long each in-between frame of the swap stays up: two frames, then the final image, about 150 ms in all.
pub const FRAME: Duration = Duration::from_millis(50);
/// A change this soon after the previous swap started is not animated (fast toggling would only flicker).
pub const QUICK: Duration = Duration::from_millis(600);

/// The icon style chosen in Settings (prefs.json `"menu_bar_icon"`): the coloured cards, or the black template.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style { #[default] Colour, Plain }
pub const STYLES: [(Style, &str); 2] = [(Style::Colour, "colour"), (Style::Plain, "plain")];

/// What the icon has to say: the input language, whether typeThai is paused, and the style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look { pub thai: bool, pub paused: bool, pub style: Style }

/// One image: a resting state, or an in-between frame of the Thai -> English swap (1 or 2; the English -> Thai swap
/// plays them backwards).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame { Rest(Look), Swap(u8, Style) }

/// Which platform's image set: the macOS menu bar (18 pt, at 18 or 36 px) or the Windows tray (16/20/24/32 px).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target { Mac, Windows }

/// The pixel size to use: macOS takes the @2x image on a Retina screen; Windows takes the size nearest the tray's
/// small-icon size for the current DPI (16 at 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %).
pub fn pixel_size(target: Target, wanted: f64) -> u32 {
    match target {
        Target::Mac => if wanted >= 1.5 { 36 } else { 18 },
        Target::Windows => *[16u32, 20, 24, 32].iter().min_by(|a, b| (**a as f64 - wanted).abs().total_cmp(&(**b as f64 - wanted).abs())).unwrap(),
    }
}

/// The file name (in `tray/`, without `.png`) of a frame at a pixel size. Windows has no plain style: its tray icons
/// are always in colour.
pub fn asset(target: Target, frame: Frame, px: u32) -> String {
    match (target, frame) {
        (Target::Mac, Frame::Rest(l)) => format!("mac-{}-{}{}-{px}", style_name(l.style), lang(l.thai), if l.paused { "-paused" } else { "" }),
        (Target::Mac, Frame::Swap(k, s)) => format!("mac-{}-swap{k}-{px}", style_name(s)),
        (Target::Windows, Frame::Rest(l)) => format!("win-{}{}-{px}", lang(l.thai), if l.paused { "-paused" } else { "" }),
        (Target::Windows, Frame::Swap(k, _)) => format!("win-swap{k}-{px}"),
    }
}
fn style_name(s: Style) -> &'static str { match s { Style::Colour => "colour", Style::Plain => "plain" } }
fn lang(thai: bool) -> &'static str { if thai { "thai" } else { "english" } }

/// The icon over time: the look it rests on, and a swap in progress.
#[derive(Debug, Default)]
pub struct Live {
    rest: Option<Look>,
    /// A swap that started at this instant, towards English (true) or towards Thai.
    swap: Option<(Instant, bool)>,
    last_swap: Option<Instant>,
    /// The frame last handed out, so the caller only changes the image when it changes.
    shown: Option<Frame>,
}
impl Live {
    pub fn new() -> Live { Live::default() }
    /// The frame to show at `now` for `look`, or None when the image does not need to change. `reduce_motion` is asked
    /// only when a swap would start (on macOS and Windows it is a system call).
    pub fn update(&mut self, now: Instant, look: Look, reduce_motion: impl FnOnce() -> bool) -> Option<Frame> {
        if self.rest != Some(look) {
            let prev = self.rest.replace(look);
            // the first image counts as a swap just made: the platform's first reading of the input source, a moment
            // after launch, changes it without a swap
            if prev.is_none() { self.last_swap = Some(now) }
            // only a change of language animates: not the first image, not pausing or a new style
            let swaps = prev.is_some_and(|p| p.thai != look.thai && p.paused == look.paused && p.style == look.style);
            let quick = self.last_swap.is_some_and(|t| now.duration_since(t) < QUICK);
            self.swap = (swaps && !look.paused && !quick && !reduce_motion()).then_some((now, !look.thai));
            if swaps { self.last_swap = Some(now) }
        }
        let frame = match self.swap {
            Some((t0, to_english)) => {
                let k = (now.duration_since(t0).as_millis() / FRAME.as_millis()) as u8;
                match (k, to_english) {
                    (0, true) | (1, false) => Frame::Swap(1, look.style),
                    (1, true) | (0, false) => Frame::Swap(2, look.style),
                    _ => { self.swap = None; Frame::Rest(look) }
                }
            }
            None => Frame::Rest(look),
        };
        (self.shown != Some(frame)).then(|| { self.shown = Some(frame); frame })
    }
    /// Whether a swap is still playing (the caller keeps ticking it).
    pub fn animating(&self) -> bool { self.swap.is_some() }
    /// The frame on screen, to draw again at another size (a screen with another scale came or went).
    pub fn shown(&self) -> Option<Frame> { self.shown }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TH: Look = Look { thai: true, paused: false, style: Style::Colour };
    const EN: Look = Look { thai: false, paused: false, style: Style::Colour };
    fn ms(t0: Instant, n: u64) -> Instant { t0 + Duration::from_millis(n) }

    #[test] fn each_state_names_its_image() {
        assert_eq!(asset(Target::Mac, Frame::Rest(TH), 36), "mac-colour-thai-36");
        assert_eq!(asset(Target::Mac, Frame::Rest(EN), 18), "mac-colour-english-18");
        assert_eq!(asset(Target::Mac, Frame::Rest(Look { paused: true, ..TH }), 36), "mac-colour-thai-paused-36");
        assert_eq!(asset(Target::Mac, Frame::Rest(Look { style: Style::Plain, ..EN }), 36), "mac-plain-english-36");
        assert_eq!(asset(Target::Mac, Frame::Rest(Look { style: Style::Plain, paused: true, ..TH }), 18), "mac-plain-thai-paused-18");
        assert_eq!(asset(Target::Mac, Frame::Swap(2, Style::Plain), 36), "mac-plain-swap2-36");
        assert_eq!(asset(Target::Windows, Frame::Rest(Look { paused: true, ..EN }), 24), "win-english-paused-24");
        // Windows has no template: plain still shows the colour icons there
        assert_eq!(asset(Target::Windows, Frame::Rest(Look { style: Style::Plain, ..TH }), 16), "win-thai-16");
        assert_eq!(asset(Target::Windows, Frame::Swap(1, Style::Colour), 32), "win-swap1-32");
    }
    #[test] fn sizes_follow_the_screen() {
        assert_eq!(pixel_size(Target::Mac, 1.0), 18); assert_eq!(pixel_size(Target::Mac, 2.0), 36);
        assert_eq!(pixel_size(Target::Windows, 16.0), 16); assert_eq!(pixel_size(Target::Windows, 20.0), 20);
        assert_eq!(pixel_size(Target::Windows, 24.0), 24); assert_eq!(pixel_size(Target::Windows, 32.0), 32);
        assert_eq!(pixel_size(Target::Windows, 28.0), 24, "175 % takes the nearer size");
        assert_eq!(pixel_size(Target::Windows, 40.0), 32);
    }
    #[test] fn startup_shows_the_current_language_at_once() {
        let mut l = Live::new(); let t0 = Instant::now();
        assert_eq!(l.update(t0, EN, || false), Some(Frame::Rest(EN)));
        assert!(!l.animating());
        assert_eq!(l.update(ms(t0, 20), EN, || false), None, "no change, no new image");
    }
    #[test] fn the_first_reading_after_launch_does_not_swap() {
        let mut l = Live::new(); let t0 = Instant::now();
        l.update(t0, EN, || false);
        assert_eq!(l.update(ms(t0, 40), TH, || false), Some(Frame::Rest(TH)), "the platform's first reading of the source");
        assert_eq!(l.shown(), Some(Frame::Rest(TH)));
    }
    #[test] fn a_switch_plays_the_swap_then_rests() {
        let mut l = Live::new(); let t0 = Instant::now();
        l.update(t0, TH, || false);
        let t1 = ms(t0, 5000);
        assert_eq!(l.update(t1, EN, || false), Some(Frame::Swap(1, Style::Colour)));
        assert_eq!(l.update(ms(t1, 20), EN, || false), None);
        assert_eq!(l.update(ms(t1, 55), EN, || false), Some(Frame::Swap(2, Style::Colour)));
        assert_eq!(l.update(ms(t1, 105), EN, || false), Some(Frame::Rest(EN)));
        assert!(!l.animating());
        // back to Thai: the same frames backwards
        let t2 = ms(t1, 5000);
        assert_eq!(l.update(t2, TH, || false), Some(Frame::Swap(2, Style::Colour)));
        assert_eq!(l.update(ms(t2, 50), TH, || false), Some(Frame::Swap(1, Style::Colour)));
        assert_eq!(l.update(ms(t2, 100), TH, || false), Some(Frame::Rest(TH)));
    }
    #[test] fn reduce_motion_quick_switches_and_pause_skip_the_swap() {
        let mut l = Live::new(); let t0 = Instant::now();
        l.update(t0, TH, || false);
        assert_eq!(l.update(ms(t0, 5000), EN, || true), Some(Frame::Rest(EN)), "reduce motion");
        // a second switch 300 ms later: straight to the new image
        assert_eq!(l.update(ms(t0, 5300), TH, || false), Some(Frame::Rest(TH)), "quick succession");
        let p = Look { paused: true, ..TH };
        assert_eq!(l.update(ms(t0, 9000), p, || false), Some(Frame::Rest(p)), "pausing does not swap");
        let pe = Look { paused: true, ..EN };
        assert_eq!(l.update(ms(t0, 12000), pe, || false), Some(Frame::Rest(pe)), "paused: the grey icon follows the language without a swap");
        let plain = Look { style: Style::Plain, ..EN };
        assert_eq!(l.update(ms(t0, 15000), plain, || false), Some(Frame::Rest(plain)), "a new style is not a swap");
    }
    #[test] fn a_switch_during_a_swap_ends_on_the_latest_language() {
        let mut l = Live::new(); let t0 = Instant::now();
        l.update(t0, TH, || false);
        assert_eq!(l.update(ms(t0, 5000), EN, || false), Some(Frame::Swap(1, Style::Colour)));
        assert_eq!(l.update(ms(t0, 5030), TH, || false), Some(Frame::Rest(TH)), "a quick switch back cancels the swap");
        assert!(!l.animating());
    }
    #[test] fn every_image_the_tray_can_ask_for_is_shipped() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tray");
        let mut names = vec![];
        for thai in [true, false] { for paused in [true, false] { for style in [Style::Colour, Style::Plain] {
            let l = Look { thai, paused, style };
            for px in [18, 36] { names.push(asset(Target::Mac, Frame::Rest(l), px)) }
            for px in [16, 20, 24, 32] { names.push(asset(Target::Windows, Frame::Rest(l), px)) }
        } } }
        for k in [1, 2] { for style in [Style::Colour, Style::Plain] {
            for px in [18, 36] { names.push(asset(Target::Mac, Frame::Swap(k, style), px)) }
            for px in [16, 20, 24, 32] { names.push(asset(Target::Windows, Frame::Swap(k, style), px)) }
        } }
        for n in names { assert!(dir.join(format!("{n}.png")).is_file(), "missing tray/{n}.png") }
    }
    #[test] fn reduce_motion_is_asked_only_when_a_swap_would_start() {
        let mut l = Live::new(); let t0 = Instant::now(); let mut asked = 0;
        l.update(t0, TH, || { asked += 1; false });
        l.update(ms(t0, 10), TH, || { asked += 1; false });
        l.update(ms(t0, 20), Look { paused: true, ..TH }, || { asked += 1; false });
        assert_eq!(asked, 0);
        l.update(ms(t0, 5000), Look { paused: true, ..EN }, || { asked += 1; false });
        assert_eq!(asked, 0, "paused never animates");
        l.update(ms(t0, 9000), EN, || { asked += 1; false });
        l.update(ms(t0, 15000), TH, || { asked += 1; false });
        assert_eq!(asked, 1);
    }
}
