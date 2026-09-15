//! The packed layout: justified rows, the way a photo wall is hung.
//!
//! Every picture keeps its own proportions and no gap is left between
//! them: pictures fill a row left to right at one shared height, and when
//! the row is full it is scaled so it spans the width exactly — the row's
//! height then differs a little from the target, and the next row starts
//! under it. Ranks are never reordered, so the wall reads in library
//! order, and the same input always packs the same way.
//!
//! Pure: no widget, no `Cx`, no clock — a function from aspects to
//! rectangles, in world units where the target row height is 1.

/// One packed picture: its top-left corner and size, in the packer's
/// units (the target row height is 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The gap left around every picture, as a fraction of the target row
/// height. Small enough to read as a hairline, large enough that two
/// dark comics do not merge into one.
pub const GUTTER: f32 = 0.04;

/// One row of the packing: which ranks it holds and where it lies, so a
/// point can be mapped to a picture by one binary search over rows and
/// one short scan inside the row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackRow {
    pub first: usize,
    /// One past the last rank in the row.
    pub end: usize,
    pub y: f32,
    pub h: f32,
}

/// The whole packing: every picture's rectangle (rank order), the rows,
/// and the outer size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Packing {
    pub rects: Vec<PackRect>,
    pub rows: Vec<PackRow>,
    pub width: f32,
    pub height: f32,
}

/// The rows for `aspects` (width over height, one per picture, rank
/// order) across `width` units, aiming at rows `target_row_height` tall.
///
/// A row closes when adding the next picture at the target height would
/// overrun the width; it is then scaled to fill the width exactly, so a
/// row of a few wide panels comes out a little shorter than the target and
/// a row of tall strips a little taller. The last row is not stretched: a
/// trailing row of two pictures scaled to the full width would be a wall
/// of two giants — it keeps the target height, left-aligned. A single
/// picture wider than the whole width is scaled down to fit it.
pub fn pack(aspects: &[f32], width: f32, target_row_height: f32) -> Packing {
    let width = if width.is_finite() && width > 0.0 { width } else { 1.0 };
    let target = if target_row_height.is_finite() && target_row_height > 0.0 { target_row_height } else { 1.0 };
    let gutter = GUTTER * target;
    let mut rects: Vec<PackRect> = Vec::with_capacity(aspects.len());
    let mut rows: Vec<PackRow> = Vec::new();
    let mut y = 0.0f32;
    let mut row_start = 0usize;
    // The natural widths of the pictures in the open row at the target height.
    let mut row_widths: Vec<f32> = Vec::new();
    let mut row_natural = 0.0f32;

    let close_row = |rects: &mut Vec<PackRect>, rows: &mut Vec<PackRow>, y: &mut f32, row_start: usize, widths: &[f32], natural: f32, last: bool| {
        if widths.is_empty() {
            return;
        }
        let gutters = gutter * widths.len() as f32;
        let available = (width - gutters).max(gutter);
        // A full row is scaled to span the width; the last row keeps the
        // target height unless it would overrun.
        let scale = if !last || natural > available { available / natural.max(1e-6) } else { 1.0 };
        // A cell is the picture plus one gutter, half on each side; the
        // cells of a full row sum to the width exactly.
        let h = target * scale;
        let mut x = 0.0f32;
        for natural_w in widths {
            let w = natural_w * scale;
            rects.push(PackRect { x: x + gutter * 0.5, y: *y + gutter * 0.5, w, h });
            x += w + gutter;
        }
        rows.push(PackRow { first: row_start, end: row_start + widths.len(), y: *y, h: h + gutter });
        *y += h + gutter;
    };

    for &aspect in aspects {
        let a = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(0.05, 20.0) } else { 1.0 };
        let natural_w = a * target;
        let gutters = gutter * (row_widths.len() as f32 + 1.0);
        if !row_widths.is_empty() && row_natural + natural_w + gutters > width {
            close_row(&mut rects, &mut rows, &mut y, row_start, &row_widths, row_natural, false);
            row_start = rects.len();
            row_widths.clear();
            row_natural = 0.0;
        }
        row_widths.push(natural_w);
        row_natural += natural_w;
    }
    close_row(&mut rects, &mut rows, &mut y, row_start, &row_widths, row_natural, true);
    let height = y;
    Packing { rects, rows, width, height }
}

impl Packing {
    /// The rank of the picture under `(x, y)`, with `slop` units of
    /// tolerance around each picture.
    pub fn hit(&self, x: f32, y: f32, slop: f32) -> Option<usize> {
        if self.rows.is_empty() {
            return None;
        }
        // The row whose band holds y (rows are stacked in order).
        let row_index = self.rows.partition_point(|r| r.y + r.h + slop < y);
        for r in self.rows.iter().skip(row_index.saturating_sub(1)).take(2) {
            if y < r.y - slop || y > r.y + r.h + slop {
                continue;
            }
            for rank in r.first..r.end {
                let rect = self.rects[rank];
                if x >= rect.x - slop && x <= rect.x + rect.w + slop && y >= rect.y - slop && y <= rect.y + rect.h + slop {
                    return Some(rank);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlaps(a: &PackRect, b: &PackRect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn every_picture_keeps_its_aspect_and_rank() {
        let aspects = [0.5, 2.0, 1.0, 0.33, 3.0, 1.2, 0.8, 1.0, 0.6, 2.5];
        let p = pack(&aspects, 6.0, 1.0);
        assert_eq!(p.rects.len(), aspects.len());
        for (rank, rect) in p.rects.iter().enumerate() {
            let got = rect.w / rect.h;
            assert!((got - aspects[rank]).abs() / aspects[rank] < 1e-4, "rank {rank}: {got} vs {}", aspects[rank]);
        }
        // Rank order is reading order: x grows within a row, y between rows.
        for r in &p.rows {
            for rank in r.first + 1..r.end {
                assert!(p.rects[rank].x > p.rects[rank - 1].x, "row {:?}", r);
            }
        }
        for w in p.rows.windows(2) {
            assert!(w[1].y > w[0].y);
        }
    }

    #[test]
    fn full_rows_span_the_width_and_nothing_overlaps() {
        let aspects: Vec<f32> = (0..40).map(|i| 0.4 + (i % 7) as f32 * 0.5).collect();
        let p = pack(&aspects, 8.0, 1.0);
        let last = p.rows.len() - 1;
        for (i, r) in p.rows.iter().enumerate() {
            let right = p.rects[r.end - 1].x + p.rects[r.end - 1].w;
            if i != last {
                assert!((right - (8.0 - GUTTER * 0.5)).abs() < 0.01, "row {i} ends at {right}");
            } else {
                assert!(right <= 8.0 + 0.01);
            }
        }
        for a in 0..p.rects.len() {
            for b in a + 1..p.rects.len() {
                assert!(!overlaps(&p.rects[a], &p.rects[b]), "{a} overlaps {b}");
            }
        }
        assert!(p.height > 0.0);
        assert_eq!(p.width, 8.0);
    }

    #[test]
    fn rows_stay_near_the_target_height() {
        let aspects: Vec<f32> = (0..60).map(|i| if i % 3 == 0 { 0.5 } else { 1.5 }).collect();
        let p = pack(&aspects, 10.0, 1.0);
        for r in &p.rows[..p.rows.len() - 1] {
            assert!(r.h > 0.6 && r.h < 1.6, "row height {}", r.h);
        }
    }

    #[test]
    fn the_last_row_is_not_stretched_and_a_giant_is_shrunk() {
        let p = pack(&[1.0, 1.0], 10.0, 1.0);
        assert_eq!(p.rows.len(), 1);
        assert!((p.rects[0].h - 1.0).abs() < 1e-6, "the last row keeps the target height");
        let giant = pack(&[20.0], 4.0, 1.0);
        assert!(giant.rects[0].w <= 4.0);
        assert_eq!(pack(&[], 5.0, 1.0).rects.len(), 0);
    }

    #[test]
    fn packing_is_deterministic_and_hit_testing_finds_ranks() {
        let aspects: Vec<f32> = (0..25).map(|i| 0.5 + (i % 5) as f32 * 0.4).collect();
        let a = pack(&aspects, 7.0, 1.0);
        let b = pack(&aspects, 7.0, 1.0);
        assert_eq!(a, b);
        for (rank, rect) in a.rects.iter().enumerate() {
            let (cx, cy) = (rect.x + rect.w * 0.5, rect.y + rect.h * 0.5);
            assert_eq!(a.hit(cx, cy, 0.0), Some(rank), "centre of {rank}");
        }
        assert_eq!(a.hit(-1.0, -1.0, 0.0), None);
        assert_eq!(a.hit(0.5, a.height + 5.0, 0.0), None);
    }
}
