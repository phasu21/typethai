//! Messages and shared state between the key-capture thread (CGEventTap / low-level hooks, owns the Engine) and the
//! gpui UI thread (bubble window + tray).
use crate::placement::{Anchor, Rect};
use crate::prefs::{SettingsCmd, SettingsSnap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub enum UiMsg {
    /// Highlight this row (Shift+Up/Down).
    Select(usize),
    Show { items: Vec<String>, anchor: Anchor },
    /// The caret moved while the bubble is shown: re-place it (same size; skipped if the rect doesn't change).
    Move(Anchor),
    Hide,
    Paused(bool),
    /// Capture thread saved state and stopped; UI should quit.
    Stopped,
    /// Current don't-translate list and learned words, for the Settings window.
    Settings(SettingsSnap),
}

/// Bubble layout constants (logical px) shared by the view and the click hit-test.
pub const PAD: f64 = 6.0;
pub const ROW_H: f64 = 30.0;
/// Bubble chrome metrics (logical units). One row = [px ROW_PX][label column (wraps)][gap][Tab keycap][px ROW_PX], at least ROW_H tall.
pub const BORDER: f64 = 1.0;
/// Panel corner radius; also the native layer clip radius on macOS.
pub const RADIUS: f64 = 8.0;
pub const ROW_PX: f64 = 10.0;
pub const FONT_PX: f64 = 15.0;
/// Space between the suggestion and the Tab `Kbd`. The hint is only drawn on the selected row; the slot is always reserved.
pub const BADGE_GAP: f64 = 8.0;
/// Width reserved for the Tab `Kbd` (`text_xs`, horizontal padding, "Tab").
pub const BADGE_W: f64 = 40.0;
/// Added to every measured single-line width: Thai upper/lower marks and tone marks on the last cluster can paint a
/// little past the advance width, and layout rounds to device pixels; the slack keeps them inside the column.
pub const MEASURE_SLACK: f64 = 3.0;
pub const MIN_W: f64 = 200.0;
/// Margin kept from the monitor's visible edges when computing the maximum bubble width.
pub const SCREEN_MARGIN: f64 = 24.0;
/// Width used by everything except the label column (padding, border, and the Tab keycap slot).
pub const CHROME_W: f64 = 2.0 * (PAD + BORDER) + 2.0 * ROW_PX + BADGE_GAP + BADGE_W;

/// Bubble layout: width and the shared label column width; `h` is only the first guess (rows wrap — the real height
/// comes from gpui's laid-out panel, see `Ui::reconcile`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout { pub w: f64, pub h: f64, pub label_w: f64 }
/// Layout from the measured one-line label widths: the label column is the widest label, clamped so the bubble is in
/// [MIN_W, max_w] (max_w = the monitor's visible width minus margins). Longer labels wrap; nothing is truncated.
pub fn layout(label_widths: &[f64], max_w: f64) -> Layout {
    let max_w = max_w.max(MIN_W);
    let widest = label_widths.iter().copied().fold(0.0, f64::max).ceil();
    let label_w = widest.clamp(MIN_W - CHROME_W, max_w - CHROME_W);
    let lines: f64 = label_widths.iter().map(|w| (w / label_w).ceil().max(1.0)).sum();
    Layout { w: CHROME_W + label_w, h: 2.0 * (PAD + BORDER) + ROW_H * lines, label_w }
}
pub fn max_width_for(visible_w: f64) -> f64 { visible_w - 2.0 * SCREEN_MARGIN }
/// Estimated label width when no text system is available (tests / fallback): 8.6 units per spacing character.
pub fn estimate_width(s: &str) -> f64 { s.chars().filter(|c| !is_combining(*c)).count() as f64 * 8.6 }
pub fn bubble_size(items: &[String], max_w: f64) -> (f64, f64) {
    let l = layout(&items.iter().map(|s| estimate_width(s)).collect::<Vec<_>>(), max_w); (l.w, l.h)
}
/// Thai above/below vowels and tone marks take no horizontal space.
fn is_combining(c: char) -> bool { matches!(c as u32, 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E) }

/// On-screen bubble geometry in global units (set by the UI, read by the capture thread for click-to-accept).
/// `rows` = (top, height) of each row in logical units relative to the bubble's top (rows have different heights
/// once they wrap); `scale` converts logical units to the global units of `frame` (Windows DPI; 1.0 on macOS).
#[derive(Clone, Debug, PartialEq)]
pub struct Geom { pub frame: Rect, pub rows: Vec<(f64, f64)>, pub scale: f64 }
impl Geom {
    /// Uniform single-line rows (before gpui has laid the bubble out).
    pub fn uniform(frame: Rect, n: usize, scale: f64) -> Geom { Geom { frame, scale, rows: (0..n).map(|i| (PAD + BORDER + ROW_H * i as f64, ROW_H)).collect() } }
    /// Row index under a global point, if the point is on the bubble (Some(None) = on the bubble but not on a row).
    pub fn hit(&self, x: f64, y: f64) -> Option<Option<usize>> {
        if !self.frame.contains(x, y) { return None }
        let ly = (y - self.frame.y) / self.scale;
        Some(self.rows.iter().position(|(t, h)| ly >= *t && ly < t + h))
    }
}
pub static GEOM: Mutex<Option<Geom>> = Mutex::new(None);
pub fn geom() -> Option<Geom> { GEOM.lock().unwrap().clone() }
pub fn set_geom(g: Option<Geom>) { *GEOM.lock().unwrap() = g; }

/// Tray -> capture thread requests (polled by a timer on the capture thread).
pub static REQ_TOGGLE_PAUSE: AtomicBool = AtomicBool::new(false);
pub static REQ_QUIT: AtomicBool = AtomicBool::new(false);
/// Settings window wants a fresh snapshot. Commands, if any, are applied first.
pub static REQ_SETTINGS: AtomicBool = AtomicBool::new(false);
static SETTINGS_CMDS: Mutex<Vec<SettingsCmd>> = Mutex::new(Vec::new());
pub fn take(flag: &AtomicBool) -> bool { flag.swap(false, Ordering::SeqCst) }
pub fn push_settings(cmd: SettingsCmd) {
    SETTINGS_CMDS.lock().unwrap().push(cmd);
    REQ_SETTINGS.store(true, Ordering::SeqCst);
}
pub fn request_settings() { REQ_SETTINGS.store(true, Ordering::SeqCst); }
pub fn take_settings() -> Vec<SettingsCmd> { std::mem::take(&mut SETTINGS_CMDS.lock().unwrap()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn hit_rows() {
        let g = Geom::uniform(Rect::new(100.0, 100.0, 300.0, bubble_size(&["a".into(), "b".into()], 1000.0).1), 2, 1.0);
        assert_eq!(g.hit(50.0, 50.0), None);
        assert_eq!(g.hit(150.0, 100.0 + PAD + 1.0 + 5.0), Some(Some(0)));
        assert_eq!(g.hit(150.0, 100.0 + PAD + 1.0 + ROW_H + 5.0), Some(Some(1)));
        assert_eq!(g.hit(150.0, 101.0), Some(None));
        let g2 = Geom::uniform(Rect::new(0.0, 0.0, 450.0, 120.0), 2, 1.5);
        assert_eq!(g2.hit(10.0, (PAD + 1.0 + ROW_H + 2.0) * 1.5), Some(Some(1)));
        // wrapped rows: row 0 is two lines tall
        let g3 = Geom { frame: Rect::new(0.0, 0.0, 300.0, 120.0), scale: 1.0, rows: vec![(7.0, 52.0), (59.0, 30.0)] };
        assert_eq!(g3.hit(10.0, 50.0), Some(Some(0))); assert_eq!(g3.hit(10.0, 70.0), Some(Some(1)));
    }
    #[test] fn size_ignores_thai_marks() {
        let (w1, h) = bubble_size(&["สวัสดี".into()], 1000.0); let (w2, _) = bubble_size(&["สวสด".into()], 1000.0);
        assert_eq!(w1, w2); assert_eq!(h, PAD * 2.0 + ROW_H + 2.0);
    }
    #[test] fn layout_widens_to_screen_then_wraps() {
        let l = layout(&[20.0, 30.0], 1400.0);
        assert_eq!(l.w, MIN_W); assert_eq!(l.label_w, MIN_W - CHROME_W); assert_eq!(l.h, 2.0 * (PAD + BORDER) + 2.0 * ROW_H);
        let l = layout(&[100.0, 180.0, 312.4], 1400.0);
        assert_eq!(l.label_w, 313.0); assert_eq!(l.w, CHROME_W + 313.0);
        // much wider than the screen: the bubble takes the max width, the row wraps (no truncation) -> taller guess
        let mw = max_width_for(1440.0); let l = layout(&[3000.0, 50.0], mw);
        assert_eq!(l.w, mw); assert_eq!(l.label_w, mw - CHROME_W);
        assert_eq!(l.h, 2.0 * (PAD + BORDER) + ROW_H * ((3000.0 / (mw - CHROME_W)).ceil() + 1.0));
        assert_eq!(layout(&[], 1400.0).h, 2.0 * (PAD + BORDER));
    }
}
