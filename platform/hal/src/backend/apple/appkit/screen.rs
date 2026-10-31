//! Keeping a window inside the attached displays: the policy of `platform/src/screen.rs`
//! (`fit_window_rect_to_screens`) on plain rectangles, for the windows the shell places. The
//! rectangles are Cocoa's global points, bottom-left origin.

/// x, y, w, h
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    fn center(&self) -> (f64, f64) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// The smallest window extent a fit ever produces.
pub const MIN_WINDOW: (f64, f64) = (200.0, 120.0);

/// One attached display.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub bounds: Rect,
    /// the extent left once the menu bar and Dock took theirs; windows are placed inside it
    pub work_area: Rect,
    pub is_primary: bool,
}

fn overlap_area(a: Rect, b: Rect) -> f64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0.0 || h <= 0.0 {
        0.0
    } else {
        w * h
    }
}

fn center_distance_sq(a: Rect, b: Rect) -> f64 {
    let (ac, bc) = (a.center(), b.center());
    let (dx, dy) = (ac.0 - bc.0, ac.1 - bc.1);
    dx * dx + dy * dy
}

/// The parts of `a` that `b` does not cover, as up to four rectangles.
fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    if overlap_area(a, b) <= 0.0 {
        return vec![a];
    }
    let (ax0, ay0, ax1, ay1) = (a.x, a.y, a.x + a.w, a.y + a.h);
    let bx0 = b.x.max(ax0);
    let by0 = b.y.max(ay0);
    let bx1 = (b.x + b.w).min(ax1);
    let by1 = (b.y + b.h).min(ay1);
    let mut out = Vec::new();
    let mut push = |x0: f64, y0: f64, x1: f64, y1: f64| {
        if x1 > x0 && y1 > y0 {
            out.push(Rect::new(x0, y0, x1 - x0, y1 - y0));
        }
    };
    push(ax0, ay0, ax1, by0);
    push(ax0, by1, ax1, ay1);
    push(ax0, by0, bx0, by1);
    push(bx1, by0, ax1, by1);
    out
}

fn is_usable(r: Rect) -> bool {
    r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite() && r.w > 0.0 && r.h > 0.0
}

/// Whether `r` lies entirely within the union of `areas`.
fn is_covered_by(areas: &[Rect], r: Rect) -> bool {
    if !is_usable(r) {
        return false;
    }
    let mut remaining = vec![r];
    for area in areas {
        let mut next = Vec::new();
        for piece in remaining.drain(..) {
            next.extend(subtract(piece, *area));
        }
        if next.is_empty() {
            return true;
        }
        remaining = next;
    }
    remaining.is_empty()
}

/// Fits a window's outer rectangle inside the work area of the display it belongs to. A window
/// wholly on the desktop (also one spanning adjacent displays) is returned untouched; anything
/// else moves onto the display it overlaps most (or whose centre is nearest), its size capped to
/// that work area and floored at [`MIN_WINDOW`], its position pulled in until it is all visible.
/// No screens: returned as it is.
pub fn fit_window_rect_to_screens(screens: &[Screen], window: Rect) -> Rect {
    let usable: Vec<Rect> = screens.iter().map(|s| s.work_area).filter(|r| is_usable(*r)).collect();
    let Some(&first) = usable.first() else { return window };
    let primary = screens.iter().find(|s| s.is_primary && is_usable(s.work_area)).map_or(first, |s| s.work_area);
    let mut want = window;
    if !want.w.is_finite() || !want.h.is_finite() {
        want.w = primary.w * 0.5;
        want.h = primary.h * 0.5;
    }
    if !want.x.is_finite() || !want.y.is_finite() {
        want.x = primary.x;
        want.y = primary.y;
    }
    if is_covered_by(&usable, want) {
        return want;
    }
    let area = usable
        .iter()
        .copied()
        .max_by(|a, b| {
            let (oa, ob) = (overlap_area(*a, want), overlap_area(*b, want));
            oa.total_cmp(&ob).then_with(|| center_distance_sq(*b, want).total_cmp(&center_distance_sq(*a, want)))
        })
        .unwrap_or(primary);
    let w = want.w.clamp(MIN_WINDOW.0.min(area.w), area.w);
    let h = want.h.clamp(MIN_WINDOW.1.min(area.h), area.h);
    let x = want.x.clamp(area.x, area.x + area.w - w);
    let y = want.y.clamp(area.y, area.y + area.h - h);
    Rect::new(x, y, w, h)
}

/// Clamps a point into the work area of the display nearest to it.
pub fn clamp_point_to_screens(screens: &[Screen], x: f64, y: f64) -> (f64, f64) {
    let r = fit_window_rect_to_screens(screens, Rect::new(x, y, 0.0, 0.0));
    (r.x, r.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(x: f64, y: f64, w: f64, h: f64, is_primary: bool) -> Screen {
        let bounds = Rect::new(x, y, w, h);
        Screen { bounds, work_area: bounds, is_primary }
    }

    #[test]
    fn a_window_already_inside_a_display_is_left_alone() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let want = Rect::new(100.0, 100.0, 800.0, 600.0);
        assert_eq!(fit_window_rect_to_screens(&screens, want), want);
    }

    #[test]
    fn no_screens_leaves_the_request_untouched() {
        let want = Rect::new(-32000.0, -32000.0, 0.0, 0.0);
        assert_eq!(fit_window_rect_to_screens(&[], want), want);
    }

    #[test]
    fn a_window_past_the_right_edge_is_pulled_back_in() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true)];
        let fitted = fit_window_rect_to_screens(&screens, Rect::new(1900.0, 50.0, 800.0, 600.0));
        assert_eq!(fitted, Rect::new(1120.0, 50.0, 800.0, 600.0));
    }

    #[test]
    fn a_window_larger_than_the_work_area_is_capped_to_it() {
        let screens = [Screen { bounds: Rect::new(0.0, 0.0, 1920.0, 1080.0), work_area: Rect::new(0.0, 0.0, 1920.0, 1040.0), is_primary: true }];
        let fitted = fit_window_rect_to_screens(&screens, Rect::new(-500.0, -500.0, 4000.0, 4000.0));
        assert_eq!(fitted, Rect::new(0.0, 0.0, 1920.0, 1040.0));
    }

    #[test]
    fn a_window_spanning_two_adjacent_displays_is_left_alone() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0, true), screen(1920.0, 0.0, 1920.0, 1080.0, false)];
        let want = Rect::new(1720.0, 100.0, 800.0, 600.0);
        assert_eq!(fit_window_rect_to_screens(&screens, want), want);
    }
}
