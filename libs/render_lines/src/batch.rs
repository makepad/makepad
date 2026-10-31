//! Line and point batches: plain CPU data, no `Cx`.
//!
//! A [`LineBatch`] is a list of capsule segments (two end points, a width
//! and a colour at each end, the distance along the line at each end and a
//! birth time at each end) drawn with one [`LineStyle`]. A [`PointBatch`]
//! is a list of sprites drawn with one [`PointStyle`]. Both pack straight
//! into the draw shaders' instance layout, so kernels, eval code and
//! physics can fill them without a conversion step.

/// Floats per packed segment instance: a (3) + width a (1), b (3) + width b
/// (1), colour a (4), colour b (4), dist a, dist b, birth a, birth b.
pub const SEGMENT_FLOATS: usize = 20;

/// Floats per packed point instance: pos (3) + size (1), colour (4),
/// rotation, birth, 2 spare.
pub const POINT_FLOATS: usize = 12;

/// Where segment and point positions live.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Space {
    /// World units, projected by the view's `view_proj`.
    #[default]
    World,
    /// Logical pixels of the target, x right, y down, origin top left
    /// (2D documents, HUD, diagrams). `z` is ignored.
    Screen,
}

/// How widths (and point sizes) are measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WidthUnit {
    /// Logical pixels: the same on-screen thickness at every distance, and
    /// the same ink at every output scale (a 1 px line at 4K is 2 physical
    /// pixels wide, sharper, not thinner).
    #[default]
    Pixels,
    /// World units (thinner with distance). Only for [`Space::World`].
    World,
}

/// How a batch combines with what is under it. Colours are linear and
/// premultiplied, and may exceed 1 (HDR: glows bloom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Blend {
    /// Premultiplied over.
    #[default]
    Over,
    /// Additive light (the target's alpha is left as it is).
    Add,
    /// Per-channel max with the target: overlapping strokes and glows keep
    /// the brightest value instead of summing (dense line work, isolines).
    Max,
}

/// The per-batch style of a [`LineBatch`]. Distances (`dash`, `gap`,
/// `trim_*`, `tail`) are in the units the batch's `dist` values were
/// written in: world length for world lines, logical pixels for screen
/// lines, or whatever a kernel wrote.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineStyle {
    pub space: Space,
    pub width_unit: WidthUnit,
    pub blend: Blend,
    /// Colour multiplier (>1 glows under bloom).
    pub intensity: f32,
    /// Dash and gap lengths (dash 0 = solid), scrolled by `dash_offset`.
    pub dash: f32,
    pub gap: f32,
    pub dash_offset: f32,
    /// The drawn window along the line: from `trim_start` to `trim_end`
    /// (distances; `trim_end` < 0 = to the end).
    pub trim_start: f32,
    pub trim_end: f32,
    /// A comet fade behind the head (`trim_end`): over this distance the
    /// line ramps from transparent to full (0 = none).
    pub tail: f32,
    /// Fade with camera distance between these (world units); far <= near
    /// = none. World lines only.
    pub fade: (f32, f32),
    /// Birth reveal: a fragment whose (interpolated) birth time is after
    /// `time` is not drawn; it fades in over `birth_fade` seconds and, when
    /// `life` > 0, out again over the last half of its life.
    pub time: f32,
    pub birth_fade: f32,
    pub life: f32,
    /// Thinnest drawn width in physical pixels: a thinner line is drawn at
    /// this width with its alpha scaled by width / floor, so its ink (the
    /// coverage summed across it) stays exact instead of aliasing away.
    pub min_px: f32,
    /// Round end caps (false: butt ends; joints between segments of one
    /// polyline are round either way where caps overlap).
    pub round_caps: bool,
    /// How the stroke's edge is antialiased.
    pub edge: LineEdge,
}

/// How a stroke's edge is antialiased.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineEdge {
    /// The exact box-filter coverage of `ink.rs`: the ink a pixel holds is
    /// the stroke's area in it, so a hairline keeps its weight anywhere.
    #[default]
    Exact,
    /// Within the stroke's own outline only (at least a pixel wide, round
    /// caps, hard trim and dash ends): crisper hairlines that snap to the
    /// pixel rows they cross.
    Inner,
}

impl Default for LineStyle {
    fn default() -> Self {
        Self {
            space: Space::World,
            width_unit: WidthUnit::Pixels,
            blend: Blend::Over,
            intensity: 1.0,
            dash: 0.0,
            gap: 0.0,
            dash_offset: 0.0,
            trim_start: 0.0,
            trim_end: -1.0,
            tail: 0.0,
            fade: (0.0, 0.0),
            time: f32::MAX,
            birth_fade: 0.0,
            life: 0.0,
            min_px: crate::ink::MIN_PX,
            round_caps: true,
            edge: LineEdge::Exact,
        }
    }
}

/// One segment, as the builders see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub width_a: f32,
    pub width_b: f32,
    /// Linear, straight (not premultiplied) RGBA.
    pub color_a: [f32; 4],
    pub color_b: [f32; 4],
    pub dist_a: f32,
    pub dist_b: f32,
    pub birth_a: f32,
    pub birth_b: f32,
}

impl Segment {
    pub fn new(a: [f32; 3], b: [f32; 3], width: f32, color: [f32; 4]) -> Self {
        let d = len3(sub3(b, a));
        Self { a, b, width_a: width, width_b: width, color_a: color, color_b: color, dist_a: 0.0, dist_b: d, birth_a: f32::MIN, birth_b: f32::MIN }
    }

    pub fn pack(&self, out: &mut Vec<f32>) {
        let (a, b, ca, cb) = (self.a, self.b, self.color_a, self.color_b);
        out.extend_from_slice(&[
            a[0], a[1], a[2], self.width_a, b[0], b[1], b[2], self.width_b, ca[0], ca[1], ca[2], ca[3], cb[0], cb[1], cb[2], cb[3], self.dist_a, self.dist_b, self.birth_a, self.birth_b,
        ]);
    }

    pub fn unpack(s: &[f32]) -> Self {
        Self {
            a: [s[0], s[1], s[2]],
            width_a: s[3],
            b: [s[4], s[5], s[6]],
            width_b: s[7],
            color_a: [s[8], s[9], s[10], s[11]],
            color_b: [s[12], s[13], s[14], s[15]],
            dist_a: s[16],
            dist_b: s[17],
            birth_a: s[18],
            birth_b: s[19],
        }
    }
}

/// A point along a polyline, as [`LineBatch::push_polyline`] takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePoint {
    pub pos: [f32; 3],
    pub width: f32,
    pub color: [f32; 4],
    pub birth: f32,
}

impl LinePoint {
    pub fn new(pos: [f32; 3], width: f32, color: [f32; 4]) -> Self {
        Self { pos, width, color, birth: f32::MIN }
    }
}

/// Segments drawn with one style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineBatch {
    pub style: LineStyle,
    /// Packed segments, [`SEGMENT_FLOATS`] each.
    pub data: Vec<f32>,
    /// The largest `dist` written (a trim of `-1` ends here).
    pub total: f32,
}

impl LineBatch {
    pub fn new(style: LineStyle) -> Self {
        Self { style, data: Vec::new(), total: 0.0 }
    }

    pub fn clear(&mut self) {
        self.data.clear();
        self.total = 0.0;
    }

    pub fn len(&self) -> usize {
        self.data.len() / SEGMENT_FLOATS
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn segment(&self, i: usize) -> Segment {
        Segment::unpack(&self.data[i * SEGMENT_FLOATS..(i + 1) * SEGMENT_FLOATS])
    }

    pub fn push(&mut self, s: Segment) {
        self.total = self.total.max(s.dist_a).max(s.dist_b);
        s.pack(&mut self.data);
    }

    /// Straight segments through `points`, with `dist` running on from
    /// `start_dist` (so several polylines can share one dash and trim
    /// window, or each restart at 0). Returns the distance at the end.
    /// Zero-length steps are skipped (they carry no direction).
    pub fn push_polyline(&mut self, points: &[LinePoint], start_dist: f32) -> f32 {
        let mut d = start_dist;
        self.data.reserve(points.len().saturating_sub(1) * SEGMENT_FLOATS);
        for w in points.windows(2) {
            let (p, q) = (w[0], w[1]);
            let l = len3(sub3(q.pos, p.pos));
            if !(l > 0.0) {
                continue;
            }
            self.push(Segment { a: p.pos, b: q.pos, width_a: p.width, width_b: q.width, color_a: p.color, color_b: q.color, dist_a: d, dist_b: d + l, birth_a: p.birth, birth_b: q.birth });
            d += l;
        }
        d
    }

    /// A polyline of positions with one width and colour.
    pub fn push_path(&mut self, points: &[[f32; 3]], width: f32, color: [f32; 4], closed: bool) -> f32 {
        let mut pts: Vec<LinePoint> = points.iter().map(|&p| LinePoint::new(p, width, color)).collect();
        if closed && points.len() > 2 {
            pts.push(pts[0]);
        }
        self.push_polyline(&pts, 0.0)
    }

    /// Segments written by a kernel as `Lines.Segment` records
    /// ([`crate::layouts`]), or points as `Lines.Point` records forming
    /// one polyline. Records with non-finite positions are skipped.
    pub fn push_segment_records(&mut self, words: &[f32]) {
        for r in words.chunks_exact(crate::layouts::SEGMENT_RECORD) {
            let (a, b) = ([r[0], r[1], r[2]], [r[3], r[4], r[5]]);
            if !a.iter().chain(b.iter()).all(|x| x.is_finite()) {
                continue;
            }
            let c = [r[7], r[8], r[9], r[10]];
            let l = len3(sub3(b, a));
            let d0 = self.total;
            self.push(Segment { a, b, width_a: r[6], width_b: r[6], color_a: c, color_b: c, dist_a: d0, dist_b: d0 + l, birth_a: r[11], birth_b: r[11] });
        }
    }

    pub fn push_point_records(&mut self, words: &[f32]) -> f32 {
        let pts: Vec<LinePoint> = words
            .chunks_exact(crate::layouts::POINT_RECORD)
            .filter(|r| r[..3].iter().all(|x| x.is_finite()))
            .map(|r| LinePoint { pos: [r[0], r[1], r[2]], width: r[3], color: [r[4], r[5], r[6], r[7]], birth: r[8] })
            .collect();
        let start = self.total;
        self.push_polyline(&pts, start)
    }
}

/// Sprite shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpriteShape {
    /// A hard disc with an anti-aliased rim.
    #[default]
    Disc,
    /// A soft round falloff (glows, dust).
    Soft,
    Square,
    /// A four-point spark.
    Spark,
    Ring,
}

impl SpriteShape {
    pub fn code(self) -> f32 {
        match self {
            SpriteShape::Disc => 0.0,
            SpriteShape::Soft => 1.0,
            SpriteShape::Square => 2.0,
            SpriteShape::Spark => 3.0,
            SpriteShape::Ring => 4.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointStyle {
    pub space: Space,
    /// `Pixels`: size is a diameter in logical pixels; `World`: world units.
    pub size_unit: WidthUnit,
    pub blend: Blend,
    pub shape: SpriteShape,
    pub intensity: f32,
    pub fade: (f32, f32),
    pub time: f32,
    pub birth_fade: f32,
    pub life: f32,
    /// Smallest drawn diameter in physical pixels (smaller sprites keep
    /// their ink as alpha, as lines do).
    pub min_px: f32,
}

impl Default for PointStyle {
    fn default() -> Self {
        Self { space: Space::World, size_unit: WidthUnit::Pixels, blend: Blend::Add, shape: SpriteShape::Soft, intensity: 1.0, fade: (0.0, 0.0), time: f32::MAX, birth_fade: 0.0, life: 0.0, min_px: 1.0 }
    }
}

/// One point sprite (see [`POINT_FLOATS`]): `size` is a diameter, `color`
/// straight linear RGBA, `birth` the time it appears (`f32::MIN`: always).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprite {
    pub pos: [f32; 3],
    pub size: f32,
    pub color: [f32; 4],
    pub rotation: f32,
    pub birth: f32,
}

impl Sprite {
    pub fn new(pos: [f32; 3], size: f32, color: [f32; 4]) -> Self {
        Self { pos, size, color, rotation: 0.0, birth: f32::MIN }
    }

    pub fn pack(&self, out: &mut Vec<f32>) {
        let (p, c) = (self.pos, self.color);
        out.extend_from_slice(&[p[0], p[1], p[2], self.size, c[0], c[1], c[2], c[3], self.rotation, self.birth, 0.0, 0.0]);
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointBatch {
    pub style: PointStyle,
    /// Packed points, [`POINT_FLOATS`] each.
    pub data: Vec<f32>,
}

impl PointBatch {
    pub fn new(style: PointStyle) -> Self {
        Self { style, data: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.data.len() / POINT_FLOATS
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn clear(&mut self) {
        self.data.clear();
    }

    pub fn push(&mut self, s: Sprite) {
        s.pack(&mut self.data);
    }

    /// Sprites written by a kernel as `Sprite` records (pos, size, colour:
    /// 8 words), the layout motion3d kernels already write.
    pub fn push_sprite_records(&mut self, words: &[f32]) {
        for r in words.chunks_exact(8) {
            if r[..4].iter().all(|x| x.is_finite()) {
                self.push(Sprite::new([r[0], r[1], r[2]], r[3], [r[4], r[5], r[6], r[7]]));
            }
        }
    }
}

pub(crate) fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn len3(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polylines_carry_running_distance_and_skip_zero_steps() {
        let mut b = LineBatch::new(LineStyle::default());
        let pts = [[0.0, 0.0, 0.0], [3.0, 4.0, 0.0], [3.0, 4.0, 0.0], [3.0, 4.0, 2.0]];
        let end = b.push_path(&pts, 1.0, [1.0; 4], false);
        assert_eq!(b.len(), 2, "the repeated point makes no segment");
        assert_eq!(end, 7.0);
        let s = b.segment(1);
        assert_eq!((s.dist_a, s.dist_b), (5.0, 7.0));
        assert_eq!(b.total, 7.0);
        assert_eq!(Segment::unpack(&b.data[SEGMENT_FLOATS..]), s);
    }

    #[test]
    fn closed_paths_return_to_the_start() {
        let mut b = LineBatch::new(LineStyle::default());
        let end = b.push_path(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]], 1.0, [1.0; 4], true);
        assert_eq!(b.len(), 3);
        assert!((end - (2.0 + 2f32.sqrt())).abs() < 1e-5);
    }

    #[test]
    fn kernel_records_become_segments_and_bad_records_are_dropped() {
        let mut b = LineBatch::new(LineStyle::default());
        let mut w = vec![0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 1.5, 1.0, 0.5, 0.25, 1.0, 3.0];
        w.extend_from_slice(&[f32::NAN, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0]);
        b.push_segment_records(&w);
        assert_eq!(b.len(), 1);
        let s = b.segment(0);
        assert_eq!((s.width_a, s.birth_b, s.dist_b), (1.5, 3.0, 2.0));
        let mut p = LineBatch::new(LineStyle::default());
        let pts = [0.0, 0.0, 0.0, 2.0, 1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 3.0, 1.0, 1.0, 1.0, 1.0, 0.5];
        assert_eq!(p.push_point_records(&pts), 1.0);
        let s = p.segment(0);
        assert_eq!((s.width_a, s.width_b, s.birth_a, s.birth_b), (2.0, 3.0, 0.0, 0.5));
    }
}
