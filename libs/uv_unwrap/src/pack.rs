//! Raster atlas packing. Each chart becomes a mask of the texels it touches at
//! the trial scale, and masks are nested by first fit against one occupancy
//! bitmap, so a chart can sit in a neighbour's concavity. Padding is exact, in
//! texels. Every chart shares one scale, which is what keeps texel density
//! consistent; a binary search on that scale is what fills the atlas.

use crate::{AtlasTarget, Checkpoint, PackOptions, UnwrapError};

/// Stop refining the scale once the bracket is this tight.
const SCALE_PRECISION: f64 = 1.01;
/// The scale search runs on an atlas this many times smaller first, where a
/// trial costs a fraction, and only checks and refines its answer at full size.
const COARSE_FACTOR: u32 = 4;
/// Atlases below this go straight to full size.
const COARSE_MIN: u32 = 256;

/// The texels of one chart, as sorted solid spans per row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    row_start: Vec<u32>,
    spans: Vec<(u32, u32)>,
}

impl Mask {
    /// Spans of `row`, each `(first, last)` inclusive.
    pub fn row(&self, row: u32) -> &[(u32, u32)] {
        let r = row as usize;
        &self.spans[self.row_start[r] as usize..self.row_start[r + 1] as usize]
    }

    /// Texels set.
    pub fn count(&self) -> u64 {
        self.spans.iter().map(|s| (s.1 - s.0 + 1) as u64).sum()
    }

    fn from_triangles(triangles: impl Iterator<Item = [[f64; 2]; 3]>, width: u32, height: u32) -> Self {
        let mut raw: Vec<(u32, u32, u32)> = Vec::new();
        for t in triangles {
            let low = t[0][1].min(t[1][1]).min(t[2][1]);
            let high = t[0][1].max(t[1][1]).max(t[2][1]);
            let first = (low.floor().max(0.0) as u32).min(height - 1);
            let last = ((high.ceil() as i64 - 1).max(first as i64) as u32).min(height - 1);
            for row in first..=last {
                let (y0, y1) = (row as f64, row as f64 + 1.0);
                let mut lo = f64::INFINITY;
                let mut hi = f64::NEG_INFINITY;
                for k in 0..3 {
                    // The edge's part inside this row.
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let (a, b) = if a[1] <= b[1] { (a, b) } else { (b, a) };
                    if a[1] > y1 || b[1] < y0 {
                        continue;
                    }
                    let dy = b[1] - a[1];
                    let (mut xa, mut xb) = (a[0], b[0]);
                    if dy > 0.0 {
                        let slope = (b[0] - a[0]) / dy;
                        if a[1] < y0 {
                            xa = a[0] + slope * (y0 - a[1]);
                        }
                        if b[1] > y1 {
                            xb = a[0] + slope * (y1 - a[1]);
                        }
                    }
                    lo = lo.min(xa.min(xb));
                    hi = hi.max(xa.max(xb));
                }
                if lo > hi {
                    continue;
                }
                let x0 = (lo.floor().max(0.0) as u32).min(width - 1);
                let x1 = ((hi.ceil() as i64 - 1).max(x0 as i64) as u32).min(width - 1);
                raw.push((row, x0, x1));
            }
        }
        if raw.is_empty() {
            raw.push((0, 0, 0));
        }
        raw.sort_unstable();
        let mut row_start = Vec::with_capacity(height as usize + 1);
        let mut spans: Vec<(u32, u32)> = Vec::new();
        let mut at = 0;
        for row in 0..height {
            row_start.push(spans.len() as u32);
            let first = spans.len();
            while at < raw.len() && raw[at].0 == row {
                let (x0, x1) = (raw[at].1, raw[at].2);
                match spans[first..].last_mut() {
                    // Touching spans merge, so a span is always solid.
                    Some(last) if x0 <= last.1 + 1 => last.1 = last.1.max(x1),
                    _ => spans.push((x0, x1)),
                }
                at += 1;
            }
        }
        row_start.push(spans.len() as u32);
        Self { width, height, row_start, spans }
    }
}

/// One bit per texel, with enough per-row summary to skip rows that cannot
/// hold a mask.
struct Occupancy {
    width: u32,
    height: u32,
    words: usize,
    bits: Vec<u64>,
    /// Longest run of free texels per row.
    longest: Vec<u32>,
    /// Lowest possibly free column per row.
    first: Vec<u32>,
}

impl Occupancy {
    fn new(width: u32, height: u32, border: u32) -> Self {
        let words = (width as usize).div_ceil(64);
        let mut this = Self {
            width,
            height,
            words,
            bits: vec![0; words * height as usize],
            longest: vec![0; height as usize],
            first: vec![0; height as usize],
        };
        let end = words as u32 * 64;
        for y in 0..height {
            if width < end {
                // Bits past the atlas read as taken, so runs end at its edge.
                this.set_range(y, width, end - 1);
            }
            if y < border || y + border >= height {
                this.set_range(y, 0, width - 1);
            } else if border > 0 {
                this.set_range(y, 0, border - 1);
                this.set_range(y, width - border, width - 1);
            }
            this.summarize(y);
        }
        this
    }

    fn set_range(&mut self, y: u32, x0: u32, x1: u32) {
        let row = y as usize * self.words;
        let (w0, w1) = ((x0 / 64) as usize, (x1 / 64) as usize);
        let head = !0u64 << (x0 % 64);
        let tail = !0u64 >> (63 - x1 % 64);
        if w0 == w1 {
            self.bits[row + w0] |= head & tail;
        } else {
            self.bits[row + w0] |= head;
            self.bits[row + w0 + 1..row + w1].fill(!0);
            self.bits[row + w1] |= tail;
        }
    }

    /// The highest occupied column in `x0..=x1`.
    #[inline]
    fn last_set(&self, y: u32, x0: u32, x1: u32) -> Option<u32> {
        let row = y as usize * self.words;
        let (w0, w1) = ((x0 / 64) as usize, (x1 / 64) as usize);
        let tail = !0u64 >> (63 - x1 % 64);
        let head = !0u64 << (x0 % 64);
        let mut w = w1;
        loop {
            let mut word = self.bits[row + w];
            if w == w1 {
                word &= tail;
            }
            if w == w0 {
                word &= head;
            }
            if word != 0 {
                return Some(w as u32 * 64 + 63 - word.leading_zeros());
            }
            if w == w0 {
                return None;
            }
            w -= 1;
        }
    }

    fn summarize(&mut self, y: u32) {
        let row = &self.bits[y as usize * self.words..(y as usize + 1) * self.words];
        let mut longest = 0u32;
        let mut run = 0u32;
        let mut first = u32::MAX;
        for (w, &word) in row.iter().enumerate() {
            let base = w as u32 * 64;
            let mut at = 0u32;
            while at < 64 {
                let rest = word >> at;
                if rest == 0 {
                    if first == u32::MAX {
                        first = base + at;
                    }
                    run += 64 - at;
                    break;
                }
                let free = rest.trailing_zeros();
                if free > 0 && first == u32::MAX {
                    first = base + at;
                }
                longest = longest.max(run + free);
                run = 0;
                at += free;
                at += (!(word >> at)).trailing_zeros();
            }
        }
        self.longest[y as usize] = longest.max(run);
        self.first[y as usize] = first.min(self.width);
    }

    /// Lowest row, then lowest column, at which `mask` fits, from row `from`.
    fn find(&self, mask: &Mask, from: u32) -> Option<(u32, u32)> {
        if mask.height > self.height || mask.width > self.width {
            return None;
        }
        let row_width = |r: u32| -> u32 {
            mask.row(r).iter().map(|s| s.1 - s.0 + 1).max().unwrap_or(0)
        };
        let widths: Vec<u32> = (0..mask.height).map(row_width).collect();
        let lefts: Vec<u32> = (0..mask.height)
            .map(|r| mask.row(r).first().map_or(u32::MAX, |s| s.0))
            .collect();
        // Widest rows first: they collide soonest, and jump furthest when they do.
        let mut order: Vec<u32> = (0..mask.height).collect();
        order.sort_unstable_by_key(|&r| (std::cmp::Reverse(widths[r as usize]), r));
        'rows: for y in from..=self.height - mask.height {
            for r in 0..mask.height {
                if self.longest[(y + r) as usize] < widths[r as usize] {
                    continue 'rows;
                }
            }
            // No column left of this can place every row's leftmost texel.
            let mut x = 0u32;
            for r in 0..mask.height {
                let left = lefts[r as usize];
                if left != u32::MAX {
                    x = x.max(self.first[(y + r) as usize].saturating_sub(left));
                }
            }
            let limit = self.width - mask.width;
            'columns: while x <= limit {
                for &r in &order {
                    for &(s0, s1) in mask.row(r) {
                        if let Some(hit) = self.last_set(y + r, x + s0, x + s1) {
                            // Every placement up to this one still covers the
                            // occupied column with this solid span.
                            x = hit - s0 + 1;
                            continue 'columns;
                        }
                    }
                }
                return Some((x, y));
            }
        }
        None
    }

    /// Marks `mask` at `(x, y)` grown by `padding` in every direction.
    fn place(&mut self, mask: &Mask, x: u32, y: u32, padding: u32) {
        let top = y.saturating_sub(padding);
        let bottom = (y + mask.height - 1 + padding).min(self.height - 1);
        for r in 0..mask.height {
            let y0 = (y + r).saturating_sub(padding);
            let y1 = (y + r + padding).min(self.height - 1);
            for i in mask.row_start[r as usize]..mask.row_start[r as usize + 1] {
                let (s0, s1) = mask.spans[i as usize];
                let x0 = (x + s0).saturating_sub(padding);
                let x1 = (x + s1 + padding).min(self.width - 1);
                for row in y0..=y1 {
                    self.set_range(row, x0, x1);
                }
            }
        }
        for row in top..=bottom {
            self.summarize(row);
        }
    }

    /// Marks triangles given in full-size texels as taken.
    fn block(&mut self, triangles: &[[[f64; 2]; 3]], divide: f64, padding: u32) {
        if triangles.is_empty() {
            return;
        }
        let scaled = triangles.iter().map(|t| t.map(|p| [p[0] / divide, p[1] / divide]));
        let mask = Mask::from_triangles(scaled, self.width, self.height);
        self.place(&mask, 0, 0, padding);
    }
}

/// A chart in atlas space. `map` takes chart-space points to texels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placement {
    pub x: u32,
    pub y: u32,
    /// The texels the chart touches, relative to `(x, y)`, padding excluded.
    pub mask: Mask,
    linear: [[f64; 2]; 2],
    offset: [f64; 2],
}

impl Placement {
    #[inline]
    pub fn map(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.linear[0][0] * p[0] + self.linear[0][1] * p[1] + self.offset[0],
            self.linear[1][0] * p[0] + self.linear[1][1] * p[1] + self.offset[1],
        ]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Atlas {
    pub width: u32,
    pub height: u32,
    pub texels_per_unit: f64,
    pub placements: Vec<Placement>,
    /// Chart area over atlas area, from the true triangle areas in uv.
    pub fill: f64,
    /// Texels touched by a chart over atlas area, the raster coverage.
    pub coverage: f64,
}

/// An item's chart space turned to its tightest bounding box, long side on x.
struct Shape {
    triangles: Vec<[[f64; 2]; 3]>,
    /// Rotation into the box frame, and the box's low corner in that frame.
    rotation: [f64; 2],
    low: [f64; 2],
    size: [f64; 2],
    area: f64,
}

impl Shape {
    fn new(triangles: Vec<[[f64; 2]; 3]>) -> Self {
        let mut points: Vec<[f64; 2]> = triangles.iter().flatten().copied().collect();
        let area: f64 = triangles
            .iter()
            .map(|t| {
                ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0])).abs() * 0.5
            })
            .sum();
        let hull = convex_hull(&mut points);
        let frame = |c: f64, s: f64| {
            let mut low = [f64::INFINITY; 2];
            let mut high = [f64::NEG_INFINITY; 2];
            for p in &hull {
                let q = [c * p[0] + s * p[1], c * p[1] - s * p[0]];
                for d in 0..2 {
                    low[d] = low[d].min(q[d]);
                    high[d] = high[d].max(q[d]);
                }
            }
            (low, [high[0] - low[0], high[1] - low[1]])
        };
        // The tightest box has a side on a hull edge. Long hulls are sampled:
        // their nearly parallel neighbouring edges give the same box.
        let mut best = (f64::INFINITY, 1.0, 0.0);
        let step = (hull.len() / 64).max(1);
        for i in (0..hull.len()).step_by(step) {
            let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
            if !(l > 0.0) {
                continue;
            }
            let (c, s) = (d[0] / l, d[1] / l);
            let (_, size) = frame(c, s);
            if size[0] * size[1] < best.0 {
                best = (size[0] * size[1], c, s);
            }
        }
        let (mut c, mut s) = (best.1, best.2);
        let (_, size) = frame(c, s);
        if size[1] > size[0] {
            // A quarter turn lays the long side along x.
            (c, s) = (-s, c);
        }
        let (low, size) = frame(c, s);
        Self { triangles, rotation: [c, s], low, size, area }
    }

    /// Point in the box frame scaled to texels, optionally turned a quarter.
    #[inline]
    fn to_texels(&self, p: [f64; 2], scale: f64, turned: bool) -> [f64; 2] {
        let [c, s] = self.rotation;
        let x = (c * p[0] + s * p[1] - self.low[0]) * scale;
        let y = (c * p[1] - s * p[0] - self.low[1]) * scale;
        if turned {
            [self.size[1] * scale - y, x]
        } else {
            [x, y]
        }
    }

    fn mask(&self, scale: f64, turned: bool) -> Mask {
        let extent = |v: f64| ((v * scale).ceil() as u32).max(1);
        let (w, h) = if turned {
            (extent(self.size[1]), extent(self.size[0]))
        } else {
            (extent(self.size[0]), extent(self.size[1]))
        };
        Mask::from_triangles(
            self.triangles.iter().map(|t| t.map(|p| self.to_texels(p, scale, turned))),
            w,
            h,
        )
    }

    fn placement(&self, mask: Mask, x: u32, y: u32, scale: f64, turned: bool) -> Placement {
        // The texel map is affine: read it off three points.
        let origin = self.to_texels([0.0, 0.0], scale, turned);
        let ex = self.to_texels([1.0, 0.0], scale, turned);
        let ey = self.to_texels([0.0, 1.0], scale, turned);
        Placement {
            x,
            y,
            mask,
            linear: [
                [ex[0] - origin[0], ey[0] - origin[0]],
                [ex[1] - origin[1], ey[1] - origin[1]],
            ],
            offset: [origin[0] + x as f64, origin[1] + y as f64],
        }
    }
}

fn convex_hull(points: &mut [[f64; 2]]) -> Vec<[f64; 2]> {
    points.sort_unstable_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };
    let mut hull: Vec<[f64; 2]> = Vec::new();
    for &p in points.iter() {
        while hull.len() >= 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    let lower = hull.len() + 1;
    for &p in points.iter().rev().skip(1) {
        while hull.len() >= lower && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    if hull.len() > 1 {
        hull.pop();
    }
    hull
}

/// First rows not worth rechecking, by mask size class. Masks arrive largest
/// first, so where a mask of a class last landed, the next of that class or
/// larger has no earlier home. Small masks start from their own class's row.
struct Hints([[u32; 16]; 16]);

impl Hints {
    fn class(v: u32) -> usize {
        (32 - v.leading_zeros()).min(15) as usize
    }
    fn from(&self, mask: &Mask) -> u32 {
        self.0[Self::class(mask.width)][Self::class(mask.height)]
    }
    fn found(&mut self, mask: &Mask, y: u32) {
        let (w, h) = (Self::class(mask.width), Self::class(mask.height));
        for row in &mut self.0[w..] {
            for slot in &mut row[h..] {
                *slot = (*slot).max(y);
            }
        }
    }
}

struct Attempt {
    width: u32,
    height: u32,
    placements: Vec<Placement>,
}

fn attempt(
    shapes: &[Shape],
    order: &[u32],
    scale: f64,
    width: u32,
    height: u32,
    padding: u32,
    obstacles: &[[[f64; 2]; 3]],
    obstacle_divide: f64,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Option<Attempt>, UnwrapError> {
    let border = padding;
    if width <= border * 2 || height <= border * 2 {
        return Ok(None);
    }
    let mut occupancy = Occupancy::new(width, height, border);
    occupancy.block(obstacles, obstacle_divide, padding);
    let mut hints = Hints([[0; 16]; 16]);
    let mut placements = vec![Placement::default(); shapes.len()];
    let (mut used_w, mut used_h) = (0u32, 0u32);
    for &index in order {
        let shape = &shapes[index as usize];
        if !checkpoint(shape.triangles.len() as u64 + 1) {
            return Err(UnwrapError::Cancelled);
        }
        let upright = shape.mask(scale, false);
        let upright_at = occupancy.find(&upright, hints.from(&upright));
        // A quarter turn only changes the answer for a mask that is not square,
        // and only matters for one large enough to shape the layout, or one with
        // nowhere else to go.
        let large = upright.width.max(upright.height) * 16 >= width;
        let try_turn = upright.width != upright.height && (large || upright_at.is_none());
        let turned = try_turn.then(|| shape.mask(scale, true));
        let turned_at = turned
            .as_ref()
            .and_then(|m| occupancy.find(m, hints.from(m)));
        let top = |m: &Mask, at: (u32, u32)| (at.1 + m.height, at.0);
        let use_turned = match (upright_at, &turned, turned_at) {
            (Some(a), Some(m), Some(b)) => top(m, b) < top(&upright, a),
            (None, Some(_), Some(_)) => true,
            _ => false,
        };
        let (mask, at, is_turned) = if use_turned {
            (turned.unwrap(), turned_at.unwrap(), true)
        } else {
            let Some(at) = upright_at else {
                return Ok(None);
            };
            (upright, at, false)
        };
        hints.found(&mask, at.1);
        occupancy.place(&mask, at.0, at.1, padding);
        used_w = used_w.max(at.0 + mask.width);
        used_h = used_h.max(at.1 + mask.height);
        placements[index as usize] = shape.placement(mask, at.0, at.1, scale, is_turned);
    }
    Ok(Some(Attempt { width: used_w + border, height: used_h + border, placements }))
}

/// Packs groups of chart-space triangles into one atlas at one shared scale.
/// Every item keeps its shape; only a rotation, the scale and a move apply.
pub fn pack_items(
    items: Vec<Vec<[[f64; 2]; 3]>>,
    options: &PackOptions<'_>,
    checkpoint: &mut Checkpoint<'_>,
) -> Result<Atlas, UnwrapError> {
    let padding = options.padding;
    if items.is_empty() {
        return Ok(Atlas { width: 1, height: 1, texels_per_unit: 1.0, ..Atlas::default() });
    }
    let shapes: Vec<Shape> = items.into_iter().map(Shape::new).collect();
    let total: f64 = shapes.iter().map(|s| s.area).sum();
    // Largest box first. Ties keep item order, so the result is deterministic.
    let mut order: Vec<u32> = (0..shapes.len() as u32).collect();
    let box_area = |s: &Shape| s.size[0] * s.size[1];
    order.sort_by(|&a, &b| {
        box_area(&shapes[b as usize])
            .total_cmp(&box_area(&shapes[a as usize]))
            .then(a.cmp(&b))
    });
    let finish = |result: Attempt, scale: f64, width: u32, height: u32| {
        let touched: u64 = result.placements.iter().map(|p| p.mask.count()).sum();
        let texels = width as f64 * height as f64;
        Atlas {
            width,
            height,
            texels_per_unit: scale,
            placements: result.placements,
            fill: total * scale * scale / texels,
            coverage: touched as f64 / texels,
        }
    };
    let fit = |width: u32, height: u32, cap: f64, crop: bool, checkpoint: &mut Checkpoint<'_>| -> Result<Atlas, UnwrapError> {
        if width == 0 || height == 0 {
            return Err(UnwrapError::InvalidInput("the atlas has no texels".into()));
        }
        let room = (width.saturating_sub(padding * 2)) as f64 * (height.saturating_sub(padding * 2)) as f64;
        let longest = shapes.iter().map(|s| s.size[0].max(s.size[1])).fold(0.0f64, f64::max);
        // The scale at which every chart is down to a single texel. Below
        // it nothing shrinks further, so failing there is final.
        let smallest = if longest > 0.0 { 1.0 / longest } else { 1.0 };
        let largest = if total > 0.0 { (room / total).sqrt().max(smallest) } else { smallest };
        let largest = largest.min(cap).max(smallest.min(cap));
        let smallest = smallest.min(largest);
        let mut search = |width: u32, height: u32, padding: u32, divide: f64, start: f64, step: f64, precision: f64| {
            let (bottom, top) = (smallest / divide, largest / divide);
            let mut best: Option<(Attempt, f64)> = None;
            let (mut low, mut high) = (None::<f64>, None::<f64>);
            let mut trial = start.clamp(bottom, top);
            loop {
                match attempt(&shapes, &order, trial, width, height, padding, options.obstacles, divide, checkpoint)? {
                    Some(result) => {
                        low = Some(trial);
                        best = Some((result, trial));
                    }
                    None => high = Some(trial),
                }
                trial = match (low, high) {
                    (Some(l), Some(h)) if h / l <= precision => break,
                    (Some(l), Some(h)) => (l * h).sqrt(),
                    (Some(l), None) if l >= top => break,
                    (Some(l), None) => (l * step).min(top),
                    (None, Some(h)) if h <= bottom => break,
                    (None, Some(h)) => (h / step).max(bottom),
                    (None, None) => unreachable!(),
                };
            }
            Ok::<_, UnwrapError>(best)
        };
        let mut start = largest * 0.85;
        let mut step = 1.25;
        if width.min(height) >= COARSE_MIN {
            let f = COARSE_FACTOR;
            let coarse = search(width / f, height / f, padding.div_ceil(f), f as f64, start / f as f64, 1.25, 1.03)?;
            if let Some((_, scale)) = coarse {
                start = scale * f as f64;
                step = 1.03;
            }
        }
        match search(width, height, padding, 1.0, start, step, SCALE_PRECISION)? {
            Some((result, scale)) => {
                let (w, h) = if crop { (result.width.max(1), result.height.max(1)) } else { (width, height) };
                Ok(finish(result, scale, w, h))
            }
            None => Err(UnwrapError::AtlasTooSmall {
                width,
                height,
                charts: shapes.len(),
                padding,
                needed: ((shapes.len() as f64).sqrt().ceil() as u32 + 1) * (padding + 1) + padding * 2,
            }),
        }
    };
    let density = |texels_per_unit: f64, max_size: u32, checkpoint: &mut Checkpoint<'_>| -> Result<Atlas, UnwrapError> {
        if !(texels_per_unit > 0.0) {
            return Err(UnwrapError::InvalidInput("texel density must be positive".into()));
        }
        let longest = shapes
            .iter()
            .map(|s| ((s.size[0].max(s.size[1]) * texels_per_unit).ceil() as u32).max(1))
            .max()
            .unwrap_or(1);
        let snug = (total * texels_per_unit * texels_per_unit / 0.8).sqrt().ceil() as u32;
        let mut side = snug.max(longest + padding * 2 + 1).min(max_size.max(1));
        loop {
            if let Some(result) = attempt(&shapes, &order, texels_per_unit, side, side, padding, options.obstacles, 1.0, checkpoint)? {
                let (w, h) = (result.width.max(1), result.height.max(1));
                return Ok(finish(result, texels_per_unit, w, h));
            }
            if side >= max_size {
                return Err(UnwrapError::AtlasTooSmall {
                    width: max_size,
                    height: max_size,
                    charts: shapes.len(),
                    padding,
                    needed: (total * texels_per_unit * texels_per_unit / 0.6).sqrt().ceil() as u32,
                });
            }
            side = ((side as f64 * 1.1).ceil() as u32).min(max_size);
        }
    };
    match options.target {
        AtlasTarget::Fit { width, height } => fit(width, height, f64::INFINITY, false, checkpoint),
        AtlasTarget::Density { texels_per_unit, max_size } => density(texels_per_unit, max_size, checkpoint),
        AtlasTarget::UpTo { texels_per_unit, max_size } => match density(texels_per_unit, max_size, checkpoint) {
            Err(UnwrapError::AtlasTooSmall { .. }) => fit(max_size, max_size, texels_per_unit, true, checkpoint),
            other => other,
        },
    }
}
