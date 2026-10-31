use super::bands::{build_bands, BandMode, BandOptions, BandScratch};
use super::{Tessellator, VVertex};

/// Which side of the path receives the Gaussian coverage transition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FeatherSide {
    #[default]
    Both,
    Inner,
    Outer,
}

impl FeatherSide {
    fn as_float(self) -> f32 {
        match self {
            Self::Both => 0.0,
            Self::Inner => -1.0,
            Self::Outer => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct FeatherOptions {
    /// All distances are path-local units, including the one-pixel AA pad.
    pub sigma: f32,
    pub side: FeatherSide,
    pub half_width: Option<f32>,
    pub aa: f32,
}

/// Append a flattened shape to the session's slug textures and return its hull.
/// Each shape starts with three RGBA band texels, also read by Feather in
/// glyph_coverage.splash:
///   0: curve offset (curves), curve count, band offset (texels), band count
///   1: min x, min y, max x, max y (expanded hull/band bounds in path units)
///   2: sigma, side (-1 inner / 0 both / 1 outer), nonzero flag, half width
/// A negative half width means fill; a nonnegative one means stroke. Band
/// indices are absolute curve indices. Every line occupies two curve texels:
/// p0.xy, midpoint.xy | p2.xy, winding_only, 0. The second texel's .z is 1
/// for pieces with the same fill state on both sides: they still contribute
/// winding, but not distance. It is 0 for boundaries and all stroke segments.
/// Geometry carries the header texel in stroke_dist; UVs span the hull from
/// 0 to 1, so extending vertex shaders need no new varying. Gradient/effect
/// parameters remain available to get_color().
pub(crate) fn append_feather_shape(
    tess: &Tessellator,
    options: FeatherOptions,
    curves: &mut Vec<f32>,
    bands: &mut Vec<f32>,
    scratch: &mut BandScratch,
) -> Option<[VVertex; 4]> {
    let curve_offset = curves.len() / 8;
    let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (a, b) in tess.flattened_segments(options.half_width.is_none()) {
        append_segment(curves, a, b, false);
        bounds[0] = bounds[0].min(a[0]).min(b[0]);
        bounds[1] = bounds[1].min(a[1]).min(b[1]);
        bounds[2] = bounds[2].max(a[0]).max(b[0]);
        bounds[3] = bounds[3].max(a[1]).max(b[1]);
    }
    if curves.len() / 8 == curve_offset {
        return None;
    }
    let nonzero = tess.uses_nonzero_fill();
    if options.half_width.is_none() && nonzero && tess.fill_contour_count() > 1 {
        split_fill_boundaries(curve_offset, curves, scratch);
    }
    let curve_count = curves.len() / 8 - curve_offset;
    let sigma = options.sigma.max(0.0);
    let half_width = options.half_width.map(|width| width.max(0.0));
    let aa = if options.aa > 0.0 { options.aa } else { 1.0 };
    let reach = 3.0 * sigma + half_width.unwrap_or(0.0) + aa;
    bounds[0] -= reach;
    bounds[1] -= reach;
    bounds[2] += reach;
    bounds[3] += reach;
    let header = bands.len() / 4;
    bands.extend_from_slice(&[
        curve_offset as f32,
        curve_count as f32,
        0.0,
        0.0,
        bounds[0],
        bounds[1],
        bounds[2],
        bounds[3],
        sigma,
        options.side.as_float(),
        if nonzero { 1.0 } else { 0.0 },
        half_width.unwrap_or(-1.0),
    ]);
    let (band_offset, band_count) = build_bands(
        curve_offset,
        &curves[curve_offset * 8..],
        bands,
        BandOptions {
            bounds,
            count: (curve_count / 8).clamp(1, 64),
            reach,
            mode: BandMode::Distance,
        },
        scratch,
    );
    bands[header * 4 + 2] = band_offset as f32;
    bands[header * 4 + 3] = band_count as f32;
    let [x0, y0, x1, y1] = bounds;
    Some(
        [
            (x0, y0, 0.0, 0.0),
            (x1, y0, 1.0, 0.0),
            (x1, y1, 1.0, 1.0),
            (x0, y1, 0.0, 1.0),
        ]
        .map(|(x, y, u, v)| VVertex {
            x,
            y,
            u,
            v,
            stroke_dist: header as f32,
            clip_radius: 0.0,
        }),
    )
}

fn append_segment(curves: &mut Vec<f32>, a: [f32; 2], b: [f32; 2], winding_only: bool) {
    curves.extend_from_slice(&[
        a[0],
        a[1],
        (a[0] + b[0]) * 0.5,
        (a[1] + b[1]) * 0.5,
        b[0],
        b[1],
        if winding_only { 1.0 } else { 0.0 },
        0.0,
    ]);
}

fn segment(curves: &[f32], index: usize) -> ([f64; 2], [f64; 2]) {
    let curve = &curves[index * 8..index * 8 + 8];
    (
        [curve[0] as f64, curve[1] as f64],
        [curve[4] as f64, curve[5] as f64],
    )
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn add_split(splits: &mut Vec<(usize, f64)>, index: usize, t: f64) {
    if t > 0.0 && t < 1.0 {
        splits.push((index, t));
    }
}

fn split_fill_boundaries(curve_offset: usize, curves: &mut Vec<f32>, scratch: &mut BandScratch) {
    let start = curve_offset * 8;
    let end = curves.len();
    let source = &curves[start..end];
    let count = source.len() / 8;
    scratch.segment_order.clear();
    scratch.segment_order.extend(0..count);
    scratch.segment_order.sort_unstable_by(|a, b| {
        let a = &source[*a * 8..];
        let b = &source[*b * 8..];
        a[1].min(a[5]).total_cmp(&b[1].min(b[5]))
    });
    scratch.segment_splits.clear();
    scratch.segment_splits.extend((0..count).map(|i| (i, 1.0)));

    // Sweep minimum y: stop at disjoint y intervals and reject disjoint x
    // bounds before intersecting. Only overlapping boxes pay for a crossing
    // calculation, including crossings within a contour. f64 keeps short
    // pieces and nearly parallel crossings stable at f32 path coordinates.
    for (position, &i) in scratch.segment_order.iter().enumerate() {
        let (a, b) = segment(source, i);
        let ab = [b[0] - a[0], b[1] - a[1]];
        for &j in &scratch.segment_order[position + 1..] {
            let (c, d) = segment(source, j);
            if c[1].min(d[1]) > a[1].max(b[1]) {
                break;
            }
            if c[0].min(d[0]) > a[0].max(b[0]) || c[0].max(d[0]) < a[0].min(b[0]) {
                continue;
            }
            let cd = [d[0] - c[0], d[1] - c[1]];
            let ac = [c[0] - a[0], c[1] - a[1]];
            let denominator = cross(ab, cd);
            if denominator != 0.0 {
                let t = cross(ac, cd) / denominator;
                let u = cross(ac, ab) / denominator;
                if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                    add_split(&mut scratch.segment_splits, i, t);
                    add_split(&mut scratch.segment_splits, j, u);
                }
            } else if cross(ac, ab) == 0.0 {
                // Collinear overlap endpoints can also change whether a
                // piece is a boundary (shared edges or cancelled slivers).
                let axis = if ab[0].abs() > ab[1].abs() { 0 } else { 1 };
                if ab[axis] != 0.0 && cd[axis] != 0.0 {
                    for point in [c, d] {
                        add_split(
                            &mut scratch.segment_splits,
                            i,
                            (point[axis] - a[axis]) / ab[axis],
                        );
                    }
                    for point in [a, b] {
                        add_split(
                            &mut scratch.segment_splits,
                            j,
                            (point[axis] - c[axis]) / cd[axis],
                        );
                    }
                }
            }
        }
    }
    scratch
        .segment_splits
        .sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    scratch.segment_splits.dedup();

    // Keep the original segments for every winding probe. Append the pieces
    // in the same reusable texture buffer, then compact this shape's range;
    // earlier shapes' absolute curve indices never move.
    let mut split = 0;
    for i in 0..count {
        let (a, b) = segment(&curves[start..end], i);
        let mut from = 0.0;
        let mut p0 = a.map(|v| v as f32);
        while split < scratch.segment_splits.len() && scratch.segment_splits[split].0 == i {
            let to = scratch.segment_splits[split].1;
            let p2 = if to == 1.0 {
                b.map(|v| v as f32)
            } else {
                [
                    (a[0] + (b[0] - a[0]) * to) as f32,
                    (a[1] + (b[1] - a[1]) * to) as f32,
                ]
            };
            if p0 != p2 {
                let winding_only = piece_is_winding_only(&curves[start..end], a, b, from, to);
                append_segment(curves, p0, p2, winding_only);
            }
            p0 = p2;
            from = to;
            split += 1;
        }
    }
    let length = curves.len() - end;
    curves.copy_within(end.., start);
    curves.truncate(start + length);
}

fn piece_is_winding_only(curves: &[f32], a: [f64; 2], b: [f64; 2], from: f64, to: f64) -> bool {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let length = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
    let t = (from + to) * 0.5;
    let midpoint = [a[0] + delta[0] * t, a[1] + delta[1] * t];
    let mut epsilon = length * (to - from) * 0.0001;
    // Probe close to this piece, without stepping over a nearby edge. Lines
    // coincident with this one must participate in winding on both sides,
    // but do not limit the normal offset. Most boxes miss this tiny probe.
    for i in 0..curves.len() / 8 {
        let (c, d) = segment(curves, i);
        if midpoint[0] + epsilon < c[0].min(d[0])
            || midpoint[0] - epsilon > c[0].max(d[0])
            || midpoint[1] + epsilon < c[1].min(d[1])
            || midpoint[1] - epsilon > c[1].max(d[1])
        {
            continue;
        }
        let cd = [d[0] - c[0], d[1] - c[1]];
        if cross(delta, cd) == 0.0 && cross(delta, [c[0] - a[0], c[1] - a[1]]) == 0.0 {
            continue;
        }
        let relative = [midpoint[0] - c[0], midpoint[1] - c[1]];
        let squared = cd[0] * cd[0] + cd[1] * cd[1];
        let t = if squared > 0.0 {
            ((relative[0] * cd[0] + relative[1] * cd[1]) / squared).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let nearest = [relative[0] - cd[0] * t, relative[1] - cd[1] * t];
        epsilon = epsilon.min((nearest[0] * nearest[0] + nearest[1] * nearest[1]).sqrt() * 0.25);
    }
    // Offsets must remain representable even after a large translation.
    epsilon = epsilon.max(f64::EPSILON * midpoint[0].abs().max(midpoint[1].abs()).max(length) * 4.0);
    let normal = [-delta[1] / length * epsilon, delta[0] / length * epsilon];
    let samples = [
        [midpoint[0] + normal[0], midpoint[1] + normal[1]],
        [midpoint[0] - normal[0], midpoint[1] - normal[1]],
    ];
    let mut winding = [0_i32; 2];
    for i in 0..curves.len() / 8 {
        let (c, d) = segment(curves, i);
        for (side, sample) in samples.iter().enumerate() {
            // Same half-open non-zero rule as Tessellator's fill probes and
            // the band walk; every original segment contributes, unflagged.
            if c[1] <= sample[1] && d[1] > sample[1] {
                if cross(
                    [d[0] - c[0], d[1] - c[1]],
                    [sample[0] - c[0], sample[1] - c[1]],
                ) > 0.0
                {
                    winding[side] += 1;
                }
            } else if d[1] <= sample[1] && c[1] > sample[1] {
                if cross(
                    [d[0] - c[0], d[1] - c[1]],
                    [sample[0] - c[0], sample[1] - c[1]],
                ) < 0.0
                {
                    winding[side] -= 1;
                }
            }
        }
    }
    (winding[0] != 0) == (winding[1] != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{FillRule, VectorPath, Winding};

    // CPU reference of Feather.band_signed_distance in glyph_coverage.splash.
    // It reads the production builder's packed textures, not a second outline.
    fn signed_distance(
        sample: [f32; 2],
        header: usize,
        curves: &[f32],
        bands: &[f32],
    ) -> (f32, i32) {
        let shape = &bands[header * 4..header * 4 + 12];
        let count = shape[3] as usize;
        let band = (((sample[1] - shape[5]) / (shape[7] - shape[5]).max(0.000001) * count as f32)
            .floor()
            .max(0.0) as usize)
            .min(count - 1);
        let list = (shape[2] as usize + band) * 4;
        let start = bands[list] as usize * 4;
        let mut dist_squared = 1e30_f32;
        let mut winding = 0;
        for j in 0..bands[list + 1] as usize {
            let curve = bands[start + j] as usize * 8;
            let p0 = [curves[curve], curves[curve + 1]];
            let p2 = [curves[curve + 4], curves[curve + 5]];
            let delta = [p2[0] - p0[0], p2[1] - p0[1]];
            if curves[curve + 6] < 0.5 {
                let relative = [sample[0] - p0[0], sample[1] - p0[1]];
                let t = ((relative[0] * delta[0] + relative[1] * delta[1])
                    / (delta[0] * delta[0] + delta[1] * delta[1]).max(0.000000000001))
                .clamp(0.0, 1.0);
                let nearest = [relative[0] - delta[0] * t, relative[1] - delta[1] * t];
                dist_squared = dist_squared.min(nearest[0] * nearest[0] + nearest[1] * nearest[1]);
            }
            if p0[1] <= sample[1] && p2[1] > sample[1] {
                if p0[0] + (sample[1] - p0[1]) * delta[0] / delta[1] > sample[0] {
                    winding += 1;
                }
            } else if p2[1] <= sample[1] && p0[1] > sample[1] {
                if p0[0] + (sample[1] - p0[1]) * delta[0] / delta[1] > sample[0] {
                    winding -= 1;
                }
            }
        }
        let dist = dist_squared.sqrt();
        if shape[11] >= 0.0 {
            return (shape[11] - dist, winding);
        }
        let inside = if shape[10] >= 0.5 {
            winding != 0
        } else {
            winding % 2 != 0
        };
        (if inside { dist } else { -dist }, winding)
    }

    // CPU reference of Feather.feather_cover, including its zero-sigma AA ramp.
    fn cover(sd: f32, sigma: f32, side: FeatherSide) -> f32 {
        if sigma <= 0.01 {
            return (0.5 + sd).clamp(0.0, 1.0);
        }
        let x = (sd * (0.5_f32.sqrt() / sigma)).clamp(-6.0, 6.0);
        let s = x * 1.1283791671;
        let ss = s * s;
        let t = s + (0.24295 + (0.03395 + 0.0104 * ss) * ss) * (s * ss);
        let g = 0.5 + 0.5 * t / (1.0 + t * t).sqrt();
        match side {
            FeatherSide::Both => g,
            FeatherSide::Inner => (2.0 * g - 1.0).max(0.0),
            FeatherSide::Outer => (2.0 * g).min(1.0),
        }
    }

    fn options() -> FeatherOptions {
        FeatherOptions {
            sigma: 2.0,
            side: FeatherSide::Both,
            half_width: None,
            aa: 1.0,
        }
    }

    fn contour(path: &mut VectorPath, points: &[[f32; 2]], winding: Option<Winding>) {
        path.move_to(points[0][0], points[0][1]);
        // Subdivision gives the small fixtures several bands and packed index
        // texels, exercising more than the single-band case.
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            for step in 1..=16 {
                let t = step as f32 / 16.0;
                path.line_to(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t);
            }
        }
        path.close();
        if let Some(winding) = winding {
            path.winding(winding);
        }
    }

    fn assert_edge(sd: f32) {
        assert!(sd.abs() < 0.00001, "distance on edge: {sd}");
        assert_eq!(cover(sd, 2.0, FeatherSide::Both), 0.5);
        assert_eq!(cover(sd, 2.0, FeatherSide::Inner), 0.0);
        assert_eq!(cover(sd, 2.0, FeatherSide::Outer), 1.0);
    }

    #[test]
    fn concave_fill_and_ring_share_the_packed_distance_path() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        let mut path = VectorPath::new();
        contour(
            &mut path,
            &[
                [0.0, 0.0],
                [80.0, 0.0],
                [80.0, 20.0],
                [20.0, 20.0],
                [20.0, 80.0],
                [0.0, 80.0],
            ],
            None,
        );
        tess.flatten(&path, 0.25);
        let hull =
            append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch).unwrap();
        let l_header = hull[0].stroke_dist as usize;
        assert_eq!(
            (hull[0].x, hull[0].y, hull[2].x, hull[2].y),
            (-7.0, -7.0, 87.0, 87.0)
        );
        let bounds = &bands[l_header * 4 + 4..l_header * 4 + 8];
        for vertex in &hull {
            // UV reconstruction in DrawVector.pixel survives any extending
            // vertex shader that moves/scales/rotates the quad's positions.
            assert_eq!(bounds[0] + (bounds[2] - bounds[0]) * vertex.u, vertex.x);
            assert_eq!(bounds[1] + (bounds[3] - bounds[1]) * vertex.v, vertex.y);
        }
        assert!(bands[l_header * 4 + 3] > 1.0);
        for sample in [[10.0, 10.0], [10.0, 60.0], [60.0, 10.0]] {
            assert!(signed_distance(sample, l_header, &curves, &bands).0 > 0.0);
        }
        for sample in [[-2.0, 40.0], [50.0, 50.0], [40.0, 20.5]] {
            assert!(signed_distance(sample, l_header, &curves, &bands).0 < 0.0);
        }
        assert_edge(signed_distance([0.0, 40.0], l_header, &curves, &bands).0);
        assert_edge(signed_distance([40.0, 20.0], l_header, &curves, &bands).0);

        path.clear();
        contour(
            &mut path,
            &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
            None,
        );
        contour(
            &mut path,
            &[[30.0, 30.0], [70.0, 30.0], [70.0, 70.0], [30.0, 70.0]],
            None,
        );
        tess.flatten(&path, 0.25);
        let ring =
            append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch).unwrap();
        let ring_header = ring[0].stroke_dist as usize;
        assert!(ring_header > l_header);
        assert!(bands[ring_header * 4] > 0.0);
        assert!(signed_distance([10.0, 50.0], ring_header, &curves, &bands).0 > 0.0);
        assert!(signed_distance([-2.0, 50.0], ring_header, &curves, &bands).0 < 0.0);
        assert!(signed_distance([50.0, 50.0], ring_header, &curves, &bands).0 < 0.0);
        assert_edge(signed_distance([30.0, 50.0], ring_header, &curves, &bands).0);
        // Appending another shape never changes the first shape's addresses.
        assert!(signed_distance([50.0, 50.0], l_header, &curves, &bands).0 < 0.0);
    }

    #[test]
    fn nonzero_fill_respects_opposite_contours_and_same_winding_overlap() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        for hole in [true, false] {
            let mut path = VectorPath::new();
            contour(
                &mut path,
                &[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
                Some(Winding::CCW),
            );
            contour(
                &mut path,
                &[[30.0, 30.0], [70.0, 30.0], [70.0, 70.0], [30.0, 70.0]],
                Some(if hole { Winding::CW } else { Winding::CCW }),
            );
            tess.flatten(&path, 0.25);
            let hull =
                append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch)
                    .unwrap();
            let header = hull[0].stroke_dist as usize;
            assert_eq!(bands[header * 4 + 10], 1.0);
            let (sd, winding) = signed_distance([50.0, 50.0], header, &curves, &bands);
            assert_eq!(sd < 0.0, hole);
            assert_eq!(winding.abs(), if hole { 0 } else { 2 });
        }
    }

    #[test]
    fn nonzero_overlapping_squares_measure_the_union_boundary() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        let mut path = VectorPath::new();
        path.set_fill_rule(FillRule::NonZero);
        contour(
            &mut path,
            &[[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]],
            None,
        );
        contour(
            &mut path,
            &[[9.0, 6.0], [29.0, 6.0], [29.0, 26.0], [9.0, 26.0]],
            None,
        );
        tess.flatten(&path, 0.25);
        let segment_count = tess.flattened_segments(true).count();
        let hull =
            append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch).unwrap();
        let header = hull[0].stroke_dist as usize;
        assert!(bands[header * 4 + 1] > segment_count as f32);
        assert!(bands[header * 4 + 3] > 1.0);
        assert!(curves.chunks_exact(8).any(|curve| curve[6] == 1.0));
        // These lie exactly on edges hidden inside the union. The nearest
        // boundary is now a split endpoint, four units away, not that edge.
        for sample in [[20.0, 10.0], [9.0, 16.0]] {
            let (sd, winding) = signed_distance(sample, header, &curves, &bands);
            assert!((sd - 4.0).abs() < 0.00001, "sample {sample:?}: {sd}");
            assert_ne!(winding, 0);
            assert!(cover(sd, 2.0, FeatherSide::Both) > 0.97);
        }
        assert_eq!(
            signed_distance([15.0, 10.0], header, &curves, &bands).1.abs(),
            2
        );
        for sample in [[20.0, 3.0], [20.0, 6.0], [24.0, 6.0], [9.0, 23.0]] {
            assert_edge(signed_distance(sample, header, &curves, &bands).0);
        }
        assert!(
            (signed_distance([22.0, 4.0], header, &curves, &bands).0 + 2.0).abs() < 0.00001
        );
    }

    #[test]
    fn nonzero_figure_eight_overlap_splits_interior_edges_and_keeps_holes() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        let mut path = VectorPath::new();
        path.set_fill_rule(FillRule::NonZero);
        // The figure-eight's lower lobe has the square's winding; its upper
        // lobe has the opposite winding and cuts a hole out of the square.
        // No subdivision: the diagonal crossing must itself become a split.
        for points in [
            [[0.0, 0.0], [24.0, 24.0], [0.0, 24.0], [24.0, 0.0]],
            [[8.0, 7.0], [18.0, 7.0], [18.0, 19.0], [8.0, 19.0]],
        ] {
            path.move_to(points[0][0], points[0][1]);
            for point in &points[1..] {
                path.line_to(point[0], point[1]);
            }
            path.close();
        }
        tess.flatten(&path, 0.25);
        let hull =
            append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch).unwrap();
        let header = hull[0].stroke_dist as usize;
        assert!(bands[header * 4 + 1] > 8.0);
        for sample in [[16.0, 16.0], [10.0, 14.0]] {
            let sd = signed_distance(sample, header, &curves, &bands).0;
            assert!((sd - 2.0).abs() < 0.00001, "sample {sample:?}: {sd}");
        }
        assert_eq!(signed_distance([12.0, 18.0], header, &curves, &bands).1, 2);
        let (sd, winding) = signed_distance([12.0, 10.0], header, &curves, &bands);
        assert_eq!(winding, 0);
        assert!((sd + 2.0_f32.sqrt()).abs() < 0.00001);
        for sample in [[10.0, 10.0], [12.0, 12.0], [20.0, 20.0], [18.0, 16.0]] {
            assert_edge(signed_distance(sample, header, &curves, &bands).0);
        }
    }

    #[test]
    fn nonzero_collinear_overlaps_keep_boundaries_and_flag_cancelled_pieces() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        for cancelled in [false, true] {
            let mut path = VectorPath::new();
            for (x, winding) in [
                (0.0, Winding::CCW),
                (
                    10.0,
                    if cancelled { Winding::CW } else { Winding::CCW },
                ),
            ] {
                path.move_to(x, 0.0);
                path.line_to(x + 20.0, 0.0);
                path.line_to(x + 20.0, 20.0);
                path.line_to(x, 20.0);
                path.close();
                path.winding(winding);
            }
            tess.flatten(&path, 0.25);
            let hull =
                append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch)
                    .unwrap();
            let header = hull[0].stroke_dist as usize;
            assert!(bands[header * 4 + 1] > 8.0);
            let (sd, winding) = signed_distance([15.0, 1.0], header, &curves, &bands);
            assert_eq!(winding.abs(), if cancelled { 0 } else { 2 });
            assert!((sd - if cancelled { -5.0 } else { 1.0 }).abs() < 0.00001);
            if cancelled {
                // Both sides of this coincident edge are empty. Its nearest
                // real boundary is the edge of either surviving strip.
                assert!(
                    (signed_distance([15.0, 0.0], header, &curves, &bands).0 + 5.0).abs()
                        < 0.00001
                );
            } else {
                assert_edge(signed_distance([15.0, 0.0], header, &curves, &bands).0);
                assert_eq!(
                    signed_distance([10.0, 10.0], header, &curves, &bands).0,
                    10.0
                );
            }
        }
        // Compaction while appending the second shape preserves the first.
        assert_eq!(signed_distance([15.0, 1.0], 0, &curves, &bands).0, 1.0);
    }

    #[test]
    fn overlapping_evenodd_fills_and_strokes_keep_every_segment_for_distance() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        let mut path = VectorPath::new();
        for x in [0.0, 10.0] {
            contour(
                &mut path,
                &[[x, 0.0], [x + 20.0, 0.0], [x + 20.0, 20.0], [x, 20.0]],
                None,
            );
        }
        for stroke in [false, true] {
            path.set_fill_rule(if stroke {
                FillRule::NonZero
            } else {
                FillRule::EvenOdd
            });
            tess.flatten(&path, 0.25);
            let segment_count = tess.flattened_segments(!stroke).count();
            let hull = append_feather_shape(
                &tess,
                FeatherOptions {
                    half_width: if stroke { Some(2.0) } else { None },
                    ..options()
                },
                &mut curves,
                &mut bands,
                &mut scratch,
            )
            .unwrap();
            let header = hull[0].stroke_dist as usize;
            assert_eq!(bands[header * 4 + 1], segment_count as f32);
            let sd = signed_distance([20.0, 10.0], header, &curves, &bands).0;
            if stroke {
                assert_eq!(sd, 2.0);
            } else {
                assert_edge(sd);
                assert!(signed_distance([19.0, 10.0], header, &curves, &bands).0 < 0.0);
            }
        }
        assert!(curves.chunks_exact(8).all(|curve| curve[6] == 0.0));
    }

    #[test]
    fn fills_close_open_contours_and_strokes_keep_open_endpoints() {
        let mut tess = Tessellator::default();
        let mut curves = Vec::new();
        let mut bands = Vec::new();
        let mut scratch = BandScratch::default();
        let mut path = VectorPath::new();
        path.move_to(0.0, 0.0);
        path.line_to(20.0, 0.0);
        path.line_to(20.0, 20.0);
        tess.flatten(&path, 0.25);
        let fill =
            append_feather_shape(&tess, options(), &mut curves, &mut bands, &mut scratch).unwrap();
        assert_eq!(bands[fill[0].stroke_dist as usize * 4 + 1], 3.0);

        let stroke_options = FeatherOptions {
            half_width: Some(2.0),
            ..options()
        };
        let stroke =
            append_feather_shape(&tess, stroke_options, &mut curves, &mut bands, &mut scratch)
                .unwrap();
        let header = stroke[0].stroke_dist as usize;
        assert_eq!(bands[header * 4 + 1], 2.0);
        assert_eq!(signed_distance([10.0, 0.0], header, &curves, &bands).0, 2.0);
        assert_eq!(signed_distance([-1.0, 0.0], header, &curves, &bands).0, 1.0);
        assert_eq!(
            signed_distance([10.0, 4.0], header, &curves, &bands).0,
            -2.0
        );
        assert_edge(signed_distance([10.0, 2.0], header, &curves, &bands).0);
        // Closing this stroke would put [10,10] on an unwanted diagonal.
        assert!(signed_distance([10.0, 10.0], header, &curves, &bands).0 < 0.0);

        path.clear();
        path.move_to(0.0, 0.0);
        path.line_to(20.0, 0.0);
        tess.flatten(&path, 0.25);
        let horizontal =
            append_feather_shape(&tess, stroke_options, &mut curves, &mut bands, &mut scratch)
                .unwrap();
        assert_edge(
            signed_distance(
                [10.0, 2.0],
                horizontal[0].stroke_dist as usize,
                &curves,
                &bands,
            )
            .0,
        );
    }

    #[test]
    fn gaussian_sides_and_zero_sigma_aa() {
        assert_edge(0.0);
        assert_eq!(cover(-1.0, 2.0, FeatherSide::Inner), 0.0);
        assert_eq!(cover(1.0, 2.0, FeatherSide::Outer), 1.0);
        assert!(cover(-1.0, 2.0, FeatherSide::Both) < 0.5);
        assert!(cover(1.0, 2.0, FeatherSide::Both) > 0.5);
        // Phi(1) and its symmetry check the reference against known values.
        assert!((cover(2.0, 2.0, FeatherSide::Both) - 0.8413447).abs() < 0.001);
        assert!((cover(-2.0, 2.0, FeatherSide::Both) - 0.1586553).abs() < 0.001);
        for side in [FeatherSide::Both, FeatherSide::Inner, FeatherSide::Outer] {
            assert_eq!(cover(-1.0, 0.0, side), 0.0);
            assert_eq!(cover(-0.25, 0.0, side), 0.25);
            assert_eq!(cover(0.0, 0.0, side), 0.5);
            assert_eq!(cover(0.25, 0.0, side), 0.75);
            assert_eq!(cover(1.0, 0.0, side), 1.0);
            assert!(cover(1e20, 2.0, side).is_finite());
            assert!(cover(-1e20, 2.0, side).is_finite());
            // Converting both distances to device pixels preserves the
            // Gaussian under a uniform GPU scale.
            assert!((cover(1.0, 2.0, side) - cover(8.0, 16.0, side)).abs() < 0.000001);
        }
    }
}
