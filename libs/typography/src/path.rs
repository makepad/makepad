//! Text on a projected 3D path: each glyph of a layout rides a 3D polyline
//! by arc length (its centre at `offset + x`), and is projected through a
//! camera to a screen position, a screen angle along the path, a scale and
//! a depth, so flat type can follow a 3D curve (a fuse, a trail, a route)
//! and stay sharp as 2D text.

use super::layout::TextLayout;

/// Where a glyph sits on the path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathGlyph {
    /// The glyph's centre on the path, world units.
    pub world: [f32; 3],
    /// Unit tangent of the path there.
    pub tangent: [f32; 3],
    /// Screen position in pixels (x right, y down, origin top left).
    pub screen: [f32; 2],
    /// Screen angle of the path's direction (radians, y down: clockwise).
    pub angle: f32,
    /// Pixels per world unit at the glyph (the text's size in world
    /// units times this is its on-screen size).
    pub scale: f32,
    /// Clip-space depth (w).
    pub depth: f32,
    /// In front of the camera and on the path (glyphs past its ends are
    /// not visible).
    pub visible: bool,
}

/// An arc-length table over a 3D polyline.
#[derive(Clone, Debug)]
pub struct Path3 {
    points: Vec<[f32; 3]>,
    cum: Vec<f32>,
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

impl Path3 {
    pub fn new(points: &[[f32; 3]]) -> Self {
        let mut cum = Vec::with_capacity(points.len());
        let mut s = 0.0;
        for (i, p) in points.iter().enumerate() {
            if i > 0 {
                s += len(sub(*p, points[i - 1]));
            }
            cum.push(s);
        }
        Self { points: points.to_vec(), cum }
    }

    pub fn length(&self) -> f32 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    /// Point and unit tangent at arc length `s` (clamped to the path).
    pub fn at(&self, s: f32) -> ([f32; 3], [f32; 3]) {
        let n = self.points.len();
        if n == 0 {
            return ([0.0; 3], [1.0, 0.0, 0.0]);
        }
        if n == 1 {
            return (self.points[0], [1.0, 0.0, 0.0]);
        }
        let s = s.clamp(0.0, self.length());
        let i = self.cum.partition_point(|c| *c <= s).clamp(1, n - 1);
        let (a, b) = (self.points[i - 1], self.points[i]);
        let d = sub(b, a);
        let l = len(d).max(1e-12);
        let t = ((s - self.cum[i - 1]) / l).clamp(0.0, 1.0);
        ([a[0] + d[0] * t, a[1] + d[1] * t, a[2] + d[2] * t], [d[0] / l, d[1] / l, d[2] / l])
    }
}

/// Projects `p` with a column-major `view_proj` to (screen px, w).
pub fn project(view_proj: &[f32; 16], viewport: [f32; 2], p: [f32; 3]) -> ([f32; 2], f32) {
    let m = view_proj;
    let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
    let y = m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13];
    let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
    let iw = if w.abs() > 1e-9 { 1.0 / w } else { 0.0 };
    ([(x * iw * 0.5 + 0.5) * viewport[0], (0.5 - y * iw * 0.5) * viewport[1]], w)
}

/// Places every glyph of `layout` (world units: its `size` is the em in
/// world units) on `path`, starting at arc length `offset`; `lift` moves
/// each glyph off the path along `up` (e.g. to sit above a line).
pub fn place_on_path(layout: &TextLayout, path: &Path3, offset: f32, up: [f32; 3], lift: f32, view_proj: &[f32; 16], viewport: [f32; 2]) -> Vec<PathGlyph> {
    let total = path.length();
    layout
        .glyphs
        .iter()
        .map(|g| {
            let s = offset + g.x + g.advance * 0.5;
            let (p, t) = path.at(s);
            let world = [p[0] + up[0] * lift, p[1] + up[1] * lift, p[2] + up[2] * lift];
            let (screen, depth) = project(view_proj, viewport, world);
            let step = 0.01 * layout.size.max(1e-3);
            let (ahead, _) = project(view_proj, viewport, [world[0] + t[0] * step, world[1] + t[1] * step, world[2] + t[2] * step]);
            let (dx, dy) = (ahead[0] - screen[0], ahead[1] - screen[1]);
            let scale = (dx * dx + dy * dy).sqrt() / step;
            PathGlyph { world, tangent: t, screen, angle: dy.atan2(dx), scale, depth, visible: depth > 0.0 && (0.0..=total).contains(&s) }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::font::test_font;
    use super::super::layout::{layout, TextStyle};
    use super::*;

    #[test]
    fn glyphs_ride_the_path_and_project() {
        let f = test_font("IBMPlexSans-Text.ttf");
        let l = layout(&f, "fuse", &TextStyle { size: 1.0, ..TextStyle::default() });
        // A path going right, then up; an orthographic camera mapping
        // x, y in -5..5 to the screen.
        let path = Path3::new(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 3.0, 0.0]]);
        let mut vp = [0.0f32; 16];
        vp[0] = 0.2;
        vp[5] = 0.2;
        vp[10] = 1.0;
        vp[15] = 1.0;
        let g = place_on_path(&l, &path, 0.0, [0.0, 0.0, 1.0], 0.0, &vp, [1000.0, 1000.0]);
        assert_eq!(g.len(), 4);
        assert!(g[0].angle.abs() < 1e-4, "along +x on screen");
        let last = g[3];
        assert!(last.world[0] == 1.0 && last.world[1] > 0.0, "the last glyph turned the corner: {:?}", last.world);
        assert!((last.angle + std::f32::consts::FRAC_PI_2).abs() < 1e-3, "going up the screen");
        assert!((g[0].scale - 100.0).abs() < 1e-2, "100 px per unit");
        assert!(g.iter().all(|p| p.visible));
        let beyond = place_on_path(&l, &path, 3.9, [0.0, 0.0, 1.0], 0.0, &vp, [1000.0, 1000.0]);
        assert!(!beyond[3].visible);
    }
}
