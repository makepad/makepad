//! Plain geometry and the xdg_positioner solver.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect { pub x: i32, pub y: i32, pub w: i32, pub h: i32 }
impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self { Self { x, y, w, h } }
    pub fn right(&self) -> i32 { self.x + self.w }
    pub fn bottom(&self) -> i32 { self.y + self.h }
    pub fn contains_rect(&self, r: &Rect) -> bool { r.x >= self.x && r.y >= self.y && r.right() <= self.right() && r.bottom() <= self.bottom() }
    pub fn union(&self, o: &Rect) -> Rect {
        if self.w <= 0 || self.h <= 0 { return *o; }
        if o.w <= 0 || o.h <= 0 { return *self; }
        let (x, y) = (self.x.min(o.x), self.y.min(o.y));
        Rect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
}

pub const ANCHOR_NONE: u32 = 0;
pub const ANCHOR_TOP: u32 = 1;
pub const ANCHOR_BOTTOM: u32 = 2;
pub const ANCHOR_LEFT: u32 = 3;
pub const ANCHOR_RIGHT: u32 = 4;
pub const ANCHOR_TOP_LEFT: u32 = 5;
pub const ANCHOR_BOTTOM_LEFT: u32 = 6;
pub const ANCHOR_TOP_RIGHT: u32 = 7;
pub const ANCHOR_BOTTOM_RIGHT: u32 = 8;
pub const ADJ_SLIDE_X: u32 = 1;
pub const ADJ_SLIDE_Y: u32 = 2;
pub const ADJ_FLIP_X: u32 = 4;
pub const ADJ_FLIP_Y: u32 = 8;
pub const ADJ_RESIZE_X: u32 = 16;
pub const ADJ_RESIZE_Y: u32 = 32;

#[derive(Clone, Debug, Default)]
pub struct Positioner {
    pub size: (i32, i32),
    pub anchor_rect: Rect,
    pub anchor: u32,
    pub gravity: u32,
    pub adjust: u32,
    pub offset: (i32, i32),
    pub reactive: bool,
    pub parent_size: Option<(i32, i32)>,
}
fn has_left(a: u32) -> bool { matches!(a, ANCHOR_LEFT | ANCHOR_TOP_LEFT | ANCHOR_BOTTOM_LEFT) }
fn has_right(a: u32) -> bool { matches!(a, ANCHOR_RIGHT | ANCHOR_TOP_RIGHT | ANCHOR_BOTTOM_RIGHT) }
fn has_top(a: u32) -> bool { matches!(a, ANCHOR_TOP | ANCHOR_TOP_LEFT | ANCHOR_TOP_RIGHT) }
fn has_bottom(a: u32) -> bool { matches!(a, ANCHOR_BOTTOM | ANCHOR_BOTTOM_LEFT | ANCHOR_BOTTOM_RIGHT) }
fn flip_x(a: u32) -> u32 {
    match a { ANCHOR_LEFT => ANCHOR_RIGHT, ANCHOR_RIGHT => ANCHOR_LEFT, ANCHOR_TOP_LEFT => ANCHOR_TOP_RIGHT, ANCHOR_TOP_RIGHT => ANCHOR_TOP_LEFT,
        ANCHOR_BOTTOM_LEFT => ANCHOR_BOTTOM_RIGHT, ANCHOR_BOTTOM_RIGHT => ANCHOR_BOTTOM_LEFT, o => o }
}
fn flip_y(a: u32) -> u32 {
    match a { ANCHOR_TOP => ANCHOR_BOTTOM, ANCHOR_BOTTOM => ANCHOR_TOP, ANCHOR_TOP_LEFT => ANCHOR_BOTTOM_LEFT, ANCHOR_BOTTOM_LEFT => ANCHOR_TOP_LEFT,
        ANCHOR_TOP_RIGHT => ANCHOR_BOTTOM_RIGHT, ANCHOR_BOTTOM_RIGHT => ANCHOR_TOP_RIGHT, o => o }
}

impl Positioner {
    /// Position without constraint adjustment, for the given anchor/gravity.
    fn place(&self, anchor: u32, gravity: u32, size: (i32, i32)) -> (i32, i32) {
        let r = &self.anchor_rect;
        let ax = if has_left(anchor) { r.x } else if has_right(anchor) { r.right() } else { r.x + r.w / 2 };
        let ay = if has_top(anchor) { r.y } else if has_bottom(anchor) { r.bottom() } else { r.y + r.h / 2 };
        let x = ax + self.offset.0 - if has_left(gravity) { size.0 } else if has_right(gravity) { 0 } else { size.0 / 2 };
        let y = ay + self.offset.1 - if has_top(gravity) { size.1 } else if has_bottom(gravity) { 0 } else { size.1 / 2 };
        (x, y)
    }
    /// Solves the popup rectangle in the parent's window-geometry coordinates, inside `bounds`.
    /// Order per axis: flip, then slide, then resize, as the protocol describes.
    pub fn solve(&self, bounds: Rect) -> Rect {
        let (anchor, gravity) = (self.anchor, self.gravity);
        let mut size = self.size;
        let (x0, y0) = self.place(anchor, gravity, size);
        let (mut x, mut y) = (x0, y0);
        let off_x = |x: i32, w: i32| x < bounds.x || x + w > bounds.right();
        let off_y = |y: i32, h: i32| y < bounds.y || y + h > bounds.bottom();
        if off_x(x, size.0) && self.adjust & ADJ_FLIP_X != 0 {
            let (fx, _) = self.place(flip_x(anchor), flip_x(gravity), size);
            if !off_x(fx, size.0) { x = fx; }
        }
        if off_y(y, size.1) && self.adjust & ADJ_FLIP_Y != 0 {
            let (_, fy) = self.place(flip_y(anchor), flip_y(gravity), size);
            if !off_y(fy, size.1) { y = fy; }
        }
        if off_x(x, size.0) && self.adjust & ADJ_SLIDE_X != 0 {
            if x + size.0 > bounds.right() { x = bounds.right() - size.0; }
            if x < bounds.x { x = bounds.x; }
        }
        if off_y(y, size.1) && self.adjust & ADJ_SLIDE_Y != 0 {
            if y + size.1 > bounds.bottom() { y = bounds.bottom() - size.1; }
            if y < bounds.y { y = bounds.y; }
        }
        if self.adjust & ADJ_RESIZE_X != 0 && off_x(x, size.0) {
            let nx = x.max(bounds.x);
            let right = (x + size.0).min(bounds.right());
            if right > nx { size.0 = right - nx; x = nx; }
        }
        if self.adjust & ADJ_RESIZE_Y != 0 && off_y(y, size.1) {
            let ny = y.max(bounds.y);
            let bottom = (y + size.1).min(bounds.bottom());
            if bottom > ny { size.1 = bottom - ny; y = ny; }
        }
        Rect { x, y, w: size.0, h: size.1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn menu() -> Positioner {
        Positioner { size: (100, 50), anchor_rect: Rect::new(10, 10, 40, 20), anchor: ANCHOR_BOTTOM_LEFT, gravity: ANCHOR_BOTTOM_RIGHT, ..Default::default() }
    }
    #[test]
    fn plain_placement_and_gravity() {
        let b = Rect::new(0, 0, 1000, 1000);
        assert_eq!(menu().solve(b), Rect::new(10, 30, 100, 50));
        let mut p = menu();
        p.anchor = ANCHOR_NONE; p.gravity = ANCHOR_NONE; // centred on the anchor rect centre
        assert_eq!(p.solve(b), Rect::new(30 - 50, 20 - 25, 100, 50));
        p.offset = (5, 7);
        assert_eq!(p.solve(b), Rect::new(-15, 2, 100, 50));
    }
    #[test]
    fn flip_slide_resize() {
        let bounds = Rect::new(0, 0, 200, 100);
        // below does not fit: flip y puts it above the anchor rect
        let mut p = Positioner { size: (60, 40), anchor_rect: Rect::new(20, 70, 30, 20), anchor: ANCHOR_BOTTOM_LEFT, gravity: ANCHOR_BOTTOM_RIGHT, adjust: ADJ_FLIP_Y, ..Default::default() };
        assert_eq!(p.solve(bounds), Rect::new(20, 30, 60, 40));
        // without flip, slide keeps it inside
        p.adjust = ADJ_SLIDE_Y;
        assert_eq!(p.solve(bounds), Rect::new(20, 60, 60, 40));
        // x flip: right edge overflows, flipping to the left side of the anchor
        let p = Positioner { size: (80, 20), anchor_rect: Rect::new(150, 10, 40, 20), anchor: ANCHOR_TOP_RIGHT, gravity: ANCHOR_BOTTOM_RIGHT, adjust: ADJ_FLIP_X, ..Default::default() };
        assert_eq!(p.solve(bounds), Rect::new(70, 10, 80, 20));
        // resize shrinks to fit
        let p = Positioner { size: (80, 20), anchor_rect: Rect::new(150, 10, 40, 20), anchor: ANCHOR_TOP_RIGHT, gravity: ANCHOR_BOTTOM_RIGHT, adjust: ADJ_RESIZE_X, ..Default::default() };
        assert_eq!(p.solve(bounds), Rect::new(190, 10, 10, 20));
        // no adjustment: unchanged even though outside
        let p = Positioner { size: (80, 20), anchor_rect: Rect::new(150, 10, 40, 20), anchor: ANCHOR_TOP_RIGHT, gravity: ANCHOR_BOTTOM_RIGHT, ..Default::default() };
        assert_eq!(p.solve(bounds), Rect::new(190, 10, 80, 20));
    }
}
