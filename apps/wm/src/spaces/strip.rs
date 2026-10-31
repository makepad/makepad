//! The space strip: SUPER+Up lays the spaces of the display out as
//! thumbnails along its top, with a "+" tile after them. Click a thumbnail to
//! go there, drag one to reorder, drop a window on one to move the window
//! there, "x" removes a desktop (or ends a fullscreen), SUPER+Up or Esc
//! closes it. The geometry is pure (unit-tested); `desk.rs` draws it from the
//! cached captures and `lib.rs` runs the gestures.
use crate::layout::LRect;
use crate::motion::Ease;

/// Seconds the strip takes to slide in or out.
pub const SECONDS: f64 = 0.28;
/// Thumbnail height as a share of the display height, and its limits (px).
const THUMB_SHARE: f64 = 0.16;
const THUMB_MIN: f64 = 96.0;
const THUMB_MAX: f64 = 190.0;
const PAD: f64 = 12.0;
const GAP: f64 = 12.0;
/// Room under a thumbnail for its label.
const LABEL: f64 = 22.0;
/// The "x" hit square.
pub const CLOSE: f64 = 22.0;

/// What the person is dragging in the strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub index: usize,
    pub start: (f64, f64),
    /// The pointer moved far enough that this is a drag, not a click.
    pub moved: bool,
    pub at: (f64, f64),
}

pub struct Strip {
    /// 0 hidden, 1 fully in.
    pub ease: Ease,
    pub open: bool,
    /// The display it is on.
    pub screen: usize,
    pub drag: Option<Drag>,
    pub hover: Option<Hit>,
}

impl Default for Strip {
    fn default() -> Self {
        Strip { ease: Ease::at(0.0, SECONDS), open: false, screen: 0, drag: None, hover: None }
    }
}

impl Strip {
    pub fn show(&mut self, screen: usize) {
        self.open = true;
        self.screen = screen;
        self.ease.retarget(1.0);
    }

    pub fn hide(&mut self) {
        self.open = false;
        self.drag = None;
        self.hover = None;
        self.ease.retarget(0.0);
    }

    pub fn toggle(&mut self, screen: usize) {
        if self.open {
            self.hide();
        } else {
            self.show(screen);
        }
    }

    /// Visible at all (in, or still sliding out).
    pub fn visible(&self) -> bool {
        self.open || self.ease.value() > 0.001
    }

    pub fn t(&self) -> f64 {
        self.ease.value()
    }

    /// Advance; true while moving.
    pub fn step(&mut self, dt: f64) -> bool {
        self.ease.step(dt)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Thumb(usize),
    Close(usize),
    Plus,
    Band,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub index: usize,
    pub thumb: LRect,
    pub label: LRect,
    /// The "x" square: on every desktop (when there is more than one) and on
    /// every fullscreen space.
    pub close: Option<LRect>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// The whole band, shifted by the slide.
    pub band: LRect,
    pub cells: Vec<Cell>,
    pub plus: Option<LRect>,
}

/// The strip's cells on `screen` for `n` spaces at slide position `t`.
/// `closable[i]` says space `i` gets an "x"; `can_add` shows the "+" tile.
pub fn layout(screen: LRect, n: usize, closable: &[bool], can_add: bool, t: f64) -> Layout {
    let th = (screen.h * THUMB_SHARE).clamp(THUMB_MIN, THUMB_MAX);
    let tw = th * screen.w / screen.h.max(1.0);
    let tiles = n + can_add as usize;
    let mut tw_eff = tw;
    let avail = screen.w - 2.0 * PAD;
    let need = tiles as f64 * tw + (tiles.max(1) - 1) as f64 * GAP;
    if need > avail {
        tw_eff = ((avail - (tiles.max(1) - 1) as f64 * GAP) / tiles.max(1) as f64).max(40.0);
    }
    let th_eff = tw_eff * th / tw;
    let band_h = th_eff + LABEL + 2.0 * PAD;
    let y_off = -(band_h + 4.0) * (1.0 - t.clamp(0.0, 1.0));
    let band = LRect::new(screen.x, screen.y + y_off, screen.w, band_h);
    let total = tiles as f64 * tw_eff + (tiles.max(1) - 1) as f64 * GAP;
    let x0 = screen.x + (screen.w - total) * 0.5;
    let y = band.y + PAD;
    let mut cells = Vec::with_capacity(n);
    for i in 0..n {
        let x = x0 + i as f64 * (tw_eff + GAP);
        let thumb = LRect::new(x, y, tw_eff, th_eff);
        let label = LRect::new(x, y + th_eff + 2.0, tw_eff, LABEL - 2.0);
        let close = closable.get(i).copied().unwrap_or(false).then(|| {
            LRect::new(thumb.x + thumb.w - CLOSE - 4.0, thumb.y + 4.0, CLOSE, CLOSE)
        });
        cells.push(Cell { index: i, thumb, label, close });
    }
    let plus = can_add.then(|| LRect::new(x0 + n as f64 * (tw_eff + GAP), y, tw_eff, th_eff));
    Layout { band, cells, plus }
}

fn inside(r: LRect, p: (f64, f64)) -> bool {
    p.0 >= r.x && p.0 < r.x + r.w && p.1 >= r.y && p.1 < r.y + r.h
}

impl Layout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        if !inside(self.band, p) {
            return None;
        }
        for c in &self.cells {
            if c.close.is_some_and(|r| inside(r, p)) {
                return Some(Hit::Close(c.index));
            }
            if inside(c.thumb, p) {
                return Some(Hit::Thumb(c.index));
            }
        }
        if self.plus.is_some_and(|r| inside(r, p)) {
            return Some(Hit::Plus);
        }
        Some(Hit::Band)
    }

    /// The index a thumbnail dragged to `x` lands at: the number of other
    /// thumbnails whose centre lies left of it.
    pub fn drop_index(&self, from: usize, x: f64) -> usize {
        let before = self
            .cells
            .iter()
            .filter(|c| c.index != from && c.thumb.x + c.thumb.w * 0.5 < x)
            .count();
        before.min(self.cells.len().saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: LRect = LRect { x: 0.0, y: 0.0, w: 2560.0, h: 1600.0 };

    #[test]
    fn thumbnails_keep_the_display_shape_and_sit_centred() {
        let l = layout(SCREEN, 3, &[false, true, true], true, 1.0);
        assert_eq!(l.cells.len(), 3);
        let c = &l.cells[0];
        assert!((c.thumb.w / c.thumb.h - 1.6).abs() < 1e-9);
        // 3 thumbnails and the plus tile are centred as one row.
        let first = l.cells[0].thumb.x;
        let last = l.plus.unwrap();
        assert!(((first - 0.0) - (2560.0 - (last.x + last.w))).abs() < 1e-6);
        assert!(l.cells[0].close.is_none() && l.cells[1].close.is_some());
        assert!(l.band.y == 0.0 && l.band.h > c.thumb.h);
    }

    #[test]
    fn the_band_slides_in_from_above() {
        let hidden = layout(SCREEN, 2, &[true, true], true, 0.0);
        let shown = layout(SCREEN, 2, &[true, true], true, 1.0);
        assert!(hidden.band.y + hidden.band.h < 0.0);
        assert_eq!(shown.band.y, 0.0);
        let half = layout(SCREEN, 2, &[true, true], true, 0.5);
        assert!(half.band.y > hidden.band.y && half.band.y < 0.0);
    }

    #[test]
    fn a_crowded_strip_shrinks_to_fit() {
        let l = layout(LRect::new(0.0, 0.0, 800.0, 500.0), 12, &[true; 12], true, 1.0);
        let last = l.plus.unwrap();
        assert!(l.cells[0].thumb.x >= 0.0 && last.x + last.w <= 800.0 + 1e-6);
    }

    #[test]
    fn hits_tell_thumb_close_plus_and_band_apart() {
        let l = layout(SCREEN, 2, &[true, true], true, 1.0);
        let c1 = &l.cells[1];
        let mid = (c1.thumb.x + c1.thumb.w * 0.5, c1.thumb.y + c1.thumb.h * 0.5);
        assert_eq!(l.hit(mid), Some(Hit::Thumb(1)));
        let x = c1.close.unwrap();
        assert_eq!(l.hit((x.x + 2.0, x.y + 2.0)), Some(Hit::Close(1)));
        let p = l.plus.unwrap();
        assert_eq!(l.hit((p.x + 5.0, p.y + 5.0)), Some(Hit::Plus));
        assert_eq!(l.hit((5.0, 5.0)), Some(Hit::Band));
        assert_eq!(l.hit((5.0, 900.0)), None);
    }

    #[test]
    fn a_dragged_thumbnail_lands_between_the_others() {
        let l = layout(SCREEN, 4, &[true; 4], false, 1.0);
        let centre = |i: usize| l.cells[i].thumb.x + l.cells[i].thumb.w * 0.5;
        assert_eq!(l.drop_index(0, centre(0)), 0);
        assert_eq!(l.drop_index(0, centre(2) + 1.0), 2);
        assert_eq!(l.drop_index(0, 5000.0), 3);
        assert_eq!(l.drop_index(3, -10.0), 0);
        assert_eq!(l.drop_index(3, centre(1) + 1.0), 2);
    }

    #[test]
    fn showing_and_hiding_ease_and_retarget() {
        let mut s = Strip::default();
        assert!(!s.visible());
        s.show(0);
        assert!(s.visible());
        for _ in 0..8 {
            s.step(1.0 / 60.0);
        }
        let mid = s.t();
        assert!(mid > 0.0 && mid < 1.0);
        s.toggle(0);
        assert!((s.t() - mid).abs() < 1e-12, "no jump on reversal");
        while s.step(1.0 / 60.0) {}
        assert!(!s.visible());
    }
}
