//! Pure bubble-placement logic (no OS calls), shared by the macOS and Windows UI layers.
//!
//! Coordinates are "global units": a single top-left-origin space spanning all monitors. On macOS these are AppKit points
//! (y flipped to top-left, primary display at the origin); on Windows they are physical pixels of the virtual screen.
//! The bubble size is given in *logical* pixels and converted per monitor with `Monitor::size_scale` (1.0 on macOS,
//! DPI/96 on Windows), so mixed-DPI setups get the right size on the monitor that holds the caret. `Monitor::pixel`
//! (device pixels per global unit, e.g. 2.0 on Retina) is used to snap the final rect to the device-pixel grid.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect { pub x: f64, pub y: f64, pub w: f64, pub h: f64 }
impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self { Rect { x, y, w, h } }
    pub fn right(&self) -> f64 { self.x + self.w }
    pub fn bottom(&self) -> f64 { self.y + self.h }
    pub fn contains(&self, px: f64, py: f64) -> bool { px >= self.x && px < self.right() && py >= self.y && py < self.bottom() }
    pub fn intersects(&self, o: &Rect) -> bool { self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom() }
    pub fn is_valid(&self) -> bool { self.w.is_finite() && self.h.is_finite() && self.x.is_finite() && self.y.is_finite() && self.w >= 0.0 && self.h >= 0.0 && !(self.x == 0.0 && self.y == 0.0 && self.w == 0.0 && self.h == 0.0) }
    fn dist2(&self, px: f64, py: f64) -> f64 { let dx = (self.x - px).max(0.0).max(px - self.right()); let dy = (self.y - py).max(0.0).max(py - self.bottom()); dx * dx + dy * dy }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    /// Full monitor frame.
    pub frame: Rect,
    /// Usable area (without menu bar / Dock / taskbar).
    pub visible: Rect,
    /// Global units per logical pixel for sizing the bubble (macOS 1.0; Windows DPI/96).
    pub size_scale: f64,
    /// Device pixels per global unit, for snapping (macOS backingScaleFactor; Windows 1.0).
    pub pixel: f64,
}

/// Where the text being typed is. All in global units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Anchor {
    /// Caret (insertion point) rect, if the app exposes it.
    pub caret: Option<Rect>,
    /// Frame of the focused text field/element.
    pub field: Option<Rect>,
    /// Mouse position, used when there is no caret.
    pub mouse: Option<(f64, f64)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Gap between the text and the bubble (logical px).
    pub gap: f64,
    /// Minimum distance from the visible-frame edge (logical px).
    pub margin: f64,
    /// If the caret is below this fraction of the visible height (e.g. chat boxes at the bottom), prefer above.
    pub lower_fraction: f64,
    /// Field edges are used for vertical placement only if the field is at most this tall (logical px); taller
    /// fields (documents, editors) use the caret line instead so the bubble stays next to the text.
    pub max_field_h: f64,
    /// Bubble x relative to caret x (logical px). -17 = -(PAD 6 + BORDER 1 + ROW_PX 10): the first row's text starts
    /// exactly at the caret's x. The bubble passes its own layout's `text_x` (a strip's first label sits further in).
    pub caret_dx: f64,
    /// The bubble window's transparent margin (logical px), around what it draws: it may cover the gap to the caret.
    pub inset: f64,
    /// Assumed pointer height when anchoring at the mouse (logical px).
    pub mouse_h: f64,
}
impl Default for Opts { fn default() -> Self { Opts { gap: 6.0, margin: 4.0, lower_fraction: 0.72, max_field_h: 120.0, caret_dx: -17.0, inset: 0.0, mouse_h: 22.0 } } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source { Caret, Field, Mouse }
impl Source { pub fn name(self) -> &'static str { match self { Source::Caret => "caret", Source::Field => "field", Source::Mouse => "mouse" } } }

/// AppKit (bottom-left origin, y up, primary screen at the origin) -> global top-left units. `primary_h` is the primary
/// screen's height. Accessibility (AX) rects are already top-left global and need no conversion.
pub fn ns_to_global(r: Rect, primary_h: f64) -> Rect { Rect::new(r.x, primary_h - (r.y + r.h), r.w, r.h) }

/// Caret top from a zero-length `AXBoundsForRange` (`zero_y`) and the neighboring glyphs' tops.
///
/// AppKit reports the empty range one line above the glyphs, so the nearest glyph that sits lower is the line.
/// A glyph above the empty range (the newline at the end of the previous line) is ignored. WebKit reports the
/// empty range on the glyphs already, and nothing is lower, so `zero_y` is kept.
pub fn caret_top(zero_y: f64, glyph_ys: &[f64]) -> f64 {
    glyph_ys.iter().copied().filter(|y| *y > zero_y + 1.0).reduce(f64::min).unwrap_or(zero_y)
}
/// Inverse of `ns_to_global` (for NSWindow setFrame:).
pub fn global_to_ns(r: Rect, primary_h: f64) -> Rect { Rect::new(r.x, primary_h - r.y - r.h, r.w, r.h) }

/// Sanity filter for an AX caret rect: some apps return (0,0)-based, element-relative or off-screen garbage. Keep it
/// only if it lies on a monitor, is not absurdly tall, and (when the field frame is known) sits inside the field.
pub fn plausible_caret(c: Rect, field: Option<Rect>, monitors: &[Monitor]) -> bool {
    if !c.is_valid() || c.h <= 0.0 || c.h > 400.0 { return false }
    let (cx, cy) = (c.x, c.y + c.h / 2.0);
    if !monitors.is_empty() && !monitors.iter().any(|m| m.frame.contains(cx, cy)) { return false }
    match field.filter(|f| f.is_valid() && f.w > 1.0 && f.h > 1.0) {
        Some(f) => cx >= f.x - 40.0 && cx <= f.right() + 40.0 && cy >= f.y - 40.0 && cy <= f.bottom() + 40.0,
        None => true,
    }
}
/// Which source `place` would use for this anchor (logging).
pub fn source_of(a: &Anchor, o: &Opts) -> Option<Source> {
    if a.caret.is_some_and(|r| r.is_valid()) { Some(Source::Caret) }
    else if field_line(a, o).is_some() { Some(Source::Field) }
    else if a.mouse.is_some() { Some(Source::Mouse) } else { None }
}
/// Fallback "line" from the focused element frame: its left edge, full height, only for single-line-ish fields.
fn field_line(a: &Anchor, o: &Opts) -> Option<Rect> {
    a.field.filter(|f| f.is_valid() && f.w > 1.0 && f.h > 1.0 && f.h <= o.max_field_h).map(|f| Rect::new(f.x + 4.0, f.y, 1.0, f.h))
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement { pub rect: Rect, pub monitor: usize, pub above: bool, pub source: Source }

/// Index of the monitor containing (x, y), else the nearest one.
pub fn monitor_for(monitors: &[Monitor], x: f64, y: f64) -> Option<usize> {
    if let Some(i) = monitors.iter().position(|m| m.frame.contains(x, y)) { return Some(i) }
    (0..monitors.len()).min_by(|a, b| monitors[*a].frame.dist2(x, y).partial_cmp(&monitors[*b].frame.dist2(x, y)).unwrap())
}

fn snap(v: f64, pixel: f64) -> f64 { if pixel > 0.0 { (v * pixel).round() / pixel } else { v } }

/// Compute the bubble rect. `size` is the bubble's logical size (width, height).
pub fn place(anchor: &Anchor, size: (f64, f64), monitors: &[Monitor], o: &Opts) -> Option<Placement> {
    let caret = anchor.caret.filter(|r| r.is_valid());
    let (line, source) = match (caret, field_line(anchor, o), anchor.mouse) {
        (Some(c), _, _) => (c, Source::Caret),
        (None, Some(f), _) => (f, Source::Field),
        (None, None, Some((mx, my))) => (Rect::new(mx, my, 0.0, 0.0), Source::Mouse),
        (None, None, None) => return None,
    };
    let (cx, cy) = (line.x, line.y + line.h / 2.0);
    let mi = monitor_for(monitors, cx, cy)?;
    let m = monitors[mi]; let s = m.size_scale.max(0.01); let v = m.visible;
    let (w, h) = (size.0 * s, size.1 * s); let gap = (o.gap - o.inset) * s; let margin = o.margin * s;
    let line = if source == Source::Mouse { Rect::new(line.x, line.y, 0.0, o.mouse_h * s) } else { line };
    // Sit on the caret line itself. A short text field is taller than the glyphs; stretching to the field
    // frame drops the bubble below the caret.
    let (top, bottom) = (line.y, line.bottom());
    let below_y = bottom + gap; let above_y = top - gap - h;
    let fits_below = below_y + h <= v.bottom() - margin; let fits_above = above_y >= v.y + margin;
    let in_lower_part = cy > v.y + o.lower_fraction * v.h;
    let above = match (fits_below, fits_above) {
        (true, true) => in_lower_part,
        (false, true) => true,
        (true, false) => false,
        (false, false) => (top - v.y) > (v.bottom() - bottom), // neither fits: take the roomier side, then clamp
    };
    let mut y = if above { above_y } else { below_y };
    y = y.max(v.y + margin).min(v.bottom() - margin - h);
    let mut x = line.x + o.caret_dx * s;
    let (lo, hi) = (v.x + margin, v.right() - margin - w);
    x = if hi < lo { v.x + (v.w - w) / 2.0 } else { x.max(lo).min(hi) };
    Some(Placement { rect: Rect::new(snap(x, m.pixel), snap(y, m.pixel), w, h), monitor: mi, above, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    // a 1440x900 Retina MacBook (menu bar 25pt at the top, no Dock at bottom for simplicity)
    const MAC: Monitor = Monitor { frame: Rect::new(0.0, 0.0, 1440.0, 900.0), visible: Rect::new(0.0, 25.0, 1440.0, 875.0), size_scale: 1.0, pixel: 2.0 };
    const SZ: (f64, f64) = (300.0, 90.0);
    fn caret(x: f64, y: f64) -> Anchor { Anchor { caret: Some(Rect::new(x, y, 2.0, 18.0)), field: None, mouse: None } }
    fn p(a: Anchor, ms: &[Monitor]) -> Placement { place(&a, SZ, ms, &Opts::default()).unwrap() }

    #[test] fn below_caret_by_default() {
        let r = p(caret(400.0, 200.0), &[MAC]);
        assert!(!r.above); assert_eq!(r.rect.y, 200.0 + 18.0 + 6.0); assert_eq!(r.rect.x, 383.0); assert_eq!(r.source, Source::Caret);
    }
    #[test] fn a_transparent_margin_may_cover_the_gap_but_what_is_drawn_keeps_it() {
        let o = Opts { inset: 10.0, ..Opts::default() };
        let below = place(&caret(400.0, 200.0), SZ, &[MAC], &o).unwrap();
        assert_eq!(below.rect.y + 10.0, 200.0 + 18.0 + 6.0, "the drawn top is 6 below the caret");
        let above = place(&caret(400.0, 840.0), SZ, &[MAC], &o).unwrap();
        assert_eq!(above.rect.bottom() - 10.0, 840.0 - 6.0, "and its bottom 6 above it");
    }
    #[test] fn short_field_still_uses_the_caret_line() {
        // a 36pt field around an 18pt caret must not push the bubble down to the field's bottom edge
        let a = Anchor { caret: Some(Rect::new(400.0, 205.0, 2.0, 18.0)), field: Some(Rect::new(380.0, 196.0, 500.0, 36.0)), mouse: None };
        let r = p(a, &[MAC]); assert!(!r.above); assert_eq!(r.rect.y, 205.0 + 18.0 + 6.0);
    }
    #[test] fn caret_top_uses_the_glyph_line_when_the_empty_range_is_high() {
        assert_eq!(caret_top(212.0, &[228.8]), 228.8); // AppKit: empty range one line above the character
        assert_eq!(caret_top(212.0, &[195.0, 228.8]), 228.8); // newline on the previous line is ignored
        assert_eq!(caret_top(222.0, &[222.0]), 222.0); // WebKit: glyph and caret agree
        assert_eq!(caret_top(222.0, &[]), 222.0);
    }
    #[test] fn tall_field_uses_caret_line() {
        let a = Anchor { caret: Some(Rect::new(400.0, 300.0, 2.0, 18.0)), field: Some(Rect::new(0.0, 25.0, 1440.0, 875.0)), mouse: None };
        let r = p(a, &[MAC]); assert_eq!(r.rect.y, 318.0 + 6.0);
    }
    #[test] fn flips_above_when_no_room_below() {
        let r = p(caret(400.0, 840.0), &[MAC]);
        assert!(r.above); assert_eq!(r.rect.bottom(), 840.0 - 6.0); assert!(!r.rect.intersects(&Rect::new(400.0, 840.0, 2.0, 18.0)));
    }
    #[test] fn chat_box_in_lower_part_prefers_above_even_with_room() {
        // caret at y=700 (> 72% of the visible height) inside a bottom chat field: room below exists, still go above
        let a = Anchor { caret: Some(Rect::new(300.0, 700.0, 2.0, 18.0)), field: Some(Rect::new(200.0, 690.0, 800.0, 40.0)), mouse: None };
        let r = p(a, &[MAC]); assert!(r.above); assert_eq!(r.rect.bottom(), 700.0 - 6.0);
    }
    #[test] fn top_edge_below_menu_bar() {
        let r = p(caret(400.0, 26.0), &[MAC]); assert!(!r.above); assert!(r.rect.y >= 25.0 + 4.0);
    }
    #[test] fn clamps_right_and_left_edges() {
        let r = p(caret(1430.0, 200.0), &[MAC]); assert_eq!(r.rect.right(), 1440.0 - 4.0);
        let r = p(caret(2.0, 200.0), &[MAC]); assert_eq!(r.rect.x, 4.0);
    }
    #[test] fn neither_side_fits_is_clamped_inside() {
        let tiny = Monitor { frame: Rect::new(0.0, 0.0, 800.0, 150.0), visible: Rect::new(0.0, 0.0, 800.0, 150.0), size_scale: 1.0, pixel: 1.0 };
        let r = p(caret(100.0, 60.0), &[tiny]); assert!(r.rect.y >= 4.0 && r.rect.bottom() <= 146.0);
    }
    #[test] fn bubble_wider_than_monitor_is_centered() {
        let narrow = Monitor { frame: Rect::new(0.0, 0.0, 200.0, 900.0), visible: Rect::new(0.0, 0.0, 200.0, 900.0), size_scale: 1.0, pixel: 1.0 };
        let r = p(caret(10.0, 100.0), &[narrow]); assert_eq!(r.rect.x, (200.0 - 300.0) / 2.0);
    }
    #[test] fn multi_monitor_left_external_display_negative_coords() {
        // external 1920x1080 display to the LEFT of the laptop (negative x), Dock-free, menu bar on primary only
        let ext = Monitor { frame: Rect::new(-1920.0, -180.0, 1920.0, 1080.0), visible: Rect::new(-1920.0, -180.0, 1920.0, 1080.0), size_scale: 1.0, pixel: 1.0 };
        let ms = [MAC, ext];
        let r = p(caret(-10.0, 400.0), &ms); assert_eq!(r.monitor, 1);
        assert!(r.rect.right() <= -4.0, "must stay on the external display, got {:?}", r.rect); // clamped, not spilling onto the laptop
        let r = p(caret(-500.0, 860.0), &ms); assert_eq!(r.monitor, 1); assert!(r.above); // near the bottom of the external display
    }
    #[test] fn multi_monitor_stacked_vertically() {
        let above_display = Monitor { frame: Rect::new(0.0, -1080.0, 1920.0, 1080.0), visible: Rect::new(0.0, -1080.0, 1920.0, 1055.0), size_scale: 1.0, pixel: 1.0 };
        let ms = [MAC, above_display];
        // caret near the bottom of the upper display: must not drop onto the laptop below
        let r = p(caret(500.0, -60.0), &ms); assert_eq!(r.monitor, 1); assert!(r.above); assert!(r.rect.bottom() <= -25.0 - 4.0 + 1e-9);
    }
    #[test] fn windows_mixed_dpi_sizes_and_positions_in_physical_pixels() {
        // primary 1920x1080 @100%, secondary 2560x1440 @150% to the right; taskbar 48 px at the bottom
        let a = Monitor { frame: Rect::new(0.0, 0.0, 1920.0, 1080.0), visible: Rect::new(0.0, 0.0, 1920.0, 1032.0), size_scale: 1.0, pixel: 1.0 };
        let b = Monitor { frame: Rect::new(1920.0, 0.0, 2560.0, 1440.0), visible: Rect::new(1920.0, 0.0, 2560.0, 1368.0), size_scale: 1.5, pixel: 1.0 };
        let r = p(caret(2100.0, 300.0), &[a, b]);
        assert_eq!(r.monitor, 1); assert_eq!((r.rect.w, r.rect.h), (450.0, 135.0)); assert_eq!(r.rect.y, 318.0 + 9.0);
        let r = p(caret(1900.0, 1000.0), &[a, b]); assert_eq!(r.monitor, 0); assert!(r.above); assert_eq!(r.rect.w, 300.0);
        assert!(r.rect.right() <= 1916.0);
    }
    #[test] fn retina_snaps_to_half_points() {
        let r = p(caret(400.3, 200.26), &[MAC]);
        assert_eq!((r.rect.x * 2.0).fract(), 0.0); assert_eq!((r.rect.y * 2.0).fract(), 0.0);
    }
    #[test] fn mouse_fallback_uses_same_flip_rules() {
        let r = p(Anchor { caret: None, field: None, mouse: Some((600.0, 300.0)) }, &[MAC]);
        assert_eq!(r.source, Source::Mouse); assert!(!r.above); assert_eq!(r.rect.y, 300.0 + 22.0 + 6.0);
        let r = p(Anchor { caret: None, field: None, mouse: Some((600.0, 880.0)) }, &[MAC]); assert!(r.above);
        // a zero caret rect (AX returned nothing useful) also falls back to the mouse
        let r = p(Anchor { caret: Some(Rect::new(0.0, 0.0, 0.0, 0.0)), field: None, mouse: Some((600.0, 300.0)) }, &[MAC]); assert_eq!(r.source, Source::Mouse);
    }
    #[test] fn caret_off_every_monitor_uses_nearest() {
        let r = p(caret(1500.0, 200.0), &[MAC]); assert_eq!(r.monitor, 0); assert!(r.rect.right() <= 1436.0);
        assert!(place(&Anchor::default(), SZ, &[MAC], &Opts::default()).is_none());
        assert!(place(&caret(1.0, 1.0), SZ, &[], &Opts::default()).is_none());
    }
    #[test] fn ns_global_roundtrip_multi_monitor() {
        // primary 1440x900; external 1920x1080 above-left: AppKit frame origin (-1920, 900) (bottom-left, y up)
        let ext_ns = Rect::new(-1920.0, 900.0, 1920.0, 1080.0);
        let g = ns_to_global(ext_ns, 900.0); assert_eq!(g, Rect::new(-1920.0, -1080.0, 1920.0, 1080.0));
        assert_eq!(global_to_ns(g, 900.0), ext_ns);
        // a window 100 tall whose top-left is at global (10, 20) sits at AppKit y = 900-20-100 = 780
        assert_eq!(global_to_ns(Rect::new(10.0, 20.0, 50.0, 100.0), 900.0).y, 780.0);
        // display below the primary: AppKit y negative -> global y >= 900
        assert_eq!(ns_to_global(Rect::new(0.0, -1080.0, 1920.0, 1080.0), 900.0).y, 900.0);
    }
    #[test] fn caret_on_external_display_above_flips_there() {
        let ext = Monitor { frame: ns_to_global(Rect::new(-1920.0, 900.0, 1920.0, 1080.0), 900.0), visible: ns_to_global(Rect::new(-1920.0, 900.0, 1920.0, 1055.0), 900.0), size_scale: 1.0, pixel: 1.0 };
        let ms = [MAC, ext];
        // AX caret near the bottom of the external display (global y -60) -> above the caret, on that display
        let c = Rect::new(-800.0, -60.0, 1.0, 18.0); assert!(plausible_caret(c, None, &ms));
        let r = p(Anchor { caret: Some(c), field: None, mouse: None }, &ms);
        assert_eq!(r.monitor, 1); assert!(r.above); assert_eq!(r.rect.bottom(), -60.0 - 6.0); assert_eq!(r.rect.x, -817.0);
        let r = p(caret(-800.0, -900.0), &ms); assert!(!r.above); assert_eq!(r.rect.y, -900.0 + 18.0 + 6.0);
    }
    #[test] fn first_row_text_lines_up_with_caret_x() {
        let r = p(caret(500.0, 300.0), &[MAC]);
        assert_eq!(r.rect.x + crate::bridge::PAD + crate::bridge::BORDER + crate::bridge::ROW_PX, 500.0);
    }
    #[test] fn implausible_carets_rejected() {
        let f = Some(Rect::new(100.0, 100.0, 400.0, 30.0));
        assert!(plausible_caret(Rect::new(120.0, 105.0, 1.0, 18.0), f, &[MAC]));
        assert!(!plausible_caret(Rect::new(5.0, 5.0, 1.0, 18.0), f, &[MAC]), "element-relative rect");
        assert!(!plausible_caret(Rect::new(3000.0, 105.0, 1.0, 18.0), None, &[MAC]), "off screen");
        assert!(!plausible_caret(Rect::new(0.0, 0.0, 0.0, 0.0), None, &[MAC]));
    }
    #[test] fn field_fallback_then_mouse() {
        let a = Anchor { caret: None, field: Some(Rect::new(200.0, 400.0, 600.0, 30.0)), mouse: Some((10.0, 10.0)) };
        let r = p(a, &[MAC]); assert_eq!(r.source, Source::Field); assert_eq!(r.rect.y, 430.0 + 6.0); assert_eq!(r.rect.x, 187.0);
        assert_eq!(source_of(&a, &Opts::default()), Some(Source::Field));
        // tall field (document) is not a useful anchor -> mouse
        let a = Anchor { caret: None, field: Some(Rect::new(0.0, 25.0, 1440.0, 875.0)), mouse: Some((600.0, 300.0)) };
        assert_eq!(p(a, &[MAC]).source, Source::Mouse);
    }
}
