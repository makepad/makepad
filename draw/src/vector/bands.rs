use std::cmp::Ordering;

/// Coverage scans use both axes and omit curves parallel to the scan. Distance
/// queries use horizontal bands only and must retain horizontal segments too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BandMode {
    Coverage,
    Distance,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BandOptions {
    pub bounds: [f32; 4],
    pub count: usize,
    /// Padding in the same coordinate units as the curves and bounds.
    pub reach: f32,
    pub mode: BandMode,
}

/// Kept by each emitter; clearing a band retains its index storage.
#[derive(Default, Debug)]
pub(crate) struct BandScratch {
    horizontal: Vec<Vec<usize>>,
    vertical: Vec<Vec<usize>>,
    // Reused when straight segments need splitting before distance bands are
    // built: indices sorted by minimum y, then (segment, split parameter).
    pub(super) segment_order: Vec<usize>,
    pub(super) segment_splits: Vec<(usize, f64)>,
}

/// Append the slug band layout: one RGBA offset/count texel per horizontal
/// band, then (in Coverage mode) per vertical band, followed by index lists
/// packed four per texel. Offsets are absolute band texels; indices are
/// absolute curves, each occupying two RGBA texels (p0.xy p1.xy | p2.xy metadata.zw).
/// The trailing metadata does not affect band membership or ordering.
/// Returns (first band texel, bands per axis).
///
/// Unit bounds, zero reach and Coverage mode preserve DrawGlyph's layout,
/// epsilon, parallel-curve exclusions and descending maximum-coordinate sort.
pub(crate) fn build_bands(
    curve_offset: usize,
    curves: &[f32],
    band_data: &mut Vec<f32>,
    options: BandOptions,
    scratch: &mut BandScratch,
) -> (usize, usize) {
    let num_bands = options.count;
    if curves.is_empty() || num_bands == 0 {
        return (0, 0);
    }
    debug_assert_eq!(curves.len() % 8, 0);
    debug_assert_eq!(band_data.len() % 4, 0);
    let both_axes = options.mode == BandMode::Coverage;
    let axis_count = if both_axes { 2 } else { 1 };
    let band_offset = band_data.len() / 4;
    band_data.resize(band_data.len() + num_bands * axis_count * 4, 0.0);
    prepare_bands(&mut scratch.horizontal, num_bands);
    if both_axes {
        prepare_bands(&mut scratch.vertical, num_bands);
    }
    let bands_f = num_bands as f32;
    let epsilon = 1.0 / 1024.0;
    let reach = options.reach.max(0.0);
    let width = (options.bounds[2] - options.bounds[0]).max(0.000001);
    let height = (options.bounds[3] - options.bounds[1]).max(0.000001);

    for (curve_index, curve) in curves.chunks_exact(8).enumerate() {
        if !both_axes || reach > 0.0 || !curve_is_horizontal(curve) {
            let (lo, hi) = band_range(
                (curve[1].min(curve[3]).min(curve[5]) - options.bounds[1] - reach) / height
                    - epsilon,
                (curve_max_y(curve) - options.bounds[1] + reach) / height + epsilon,
                bands_f,
                num_bands,
            );
            for band in &mut scratch.horizontal[lo..=hi] {
                band.push(curve_index);
            }
        }
        if both_axes && (reach > 0.0 || !curve_is_vertical(curve)) {
            let (lo, hi) = band_range(
                (curve[0].min(curve[2]).min(curve[4]) - options.bounds[0] - reach) / width
                    - epsilon,
                (curve_max_x(curve) - options.bounds[0] + reach) / width + epsilon,
                bands_f,
                num_bands,
            );
            for band in &mut scratch.vertical[lo..=hi] {
                band.push(curve_index);
            }
        }
    }

    if both_axes {
        // Glyph scans can exit early, so retain their descending order. The
        // index tie-break preserves the old stable sort without allocating a
        // merge buffer. Distance walks need every entry: keep contour order
        // there and avoid sorting each band of an animated path.
        for list in &mut scratch.horizontal[..num_bands] {
            list.sort_unstable_by(|a, b| {
                curve_max_x(&curves[*b * 8..*b * 8 + 8])
                    .partial_cmp(&curve_max_x(&curves[*a * 8..*a * 8 + 8]))
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.cmp(b))
            });
        }
        for list in &mut scratch.vertical[..num_bands] {
            list.sort_unstable_by(|a, b| {
                curve_max_y(&curves[*b * 8..*b * 8 + 8])
                    .partial_cmp(&curve_max_y(&curves[*a * 8..*a * 8 + 8]))
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| a.cmp(b))
            });
        }
    }

    let vertical_count = if both_axes { num_bands } else { 0 };
    let mut list_texel_offset = band_offset + num_bands * axis_count;
    for (band, list) in scratch.horizontal[..num_bands]
        .iter()
        .chain(scratch.vertical[..vertical_count].iter())
        .enumerate()
    {
        let meta = (band_offset + band) * 4;
        band_data[meta] = list_texel_offset as f32;
        band_data[meta + 1] = list.len() as f32;
        for chunk in list.chunks(4) {
            let mut texel = [0.0; 4];
            for (i, value) in chunk.iter().enumerate() {
                texel[i] = (curve_offset + *value) as f32;
            }
            band_data.extend_from_slice(&texel);
            list_texel_offset += 1;
        }
    }
    (band_offset, num_bands)
}

fn prepare_bands(bands: &mut Vec<Vec<usize>>, count: usize) {
    if bands.len() < count {
        bands.resize_with(count, Vec::new);
    }
    for band in &mut bands[..count] {
        band.clear();
    }
}

fn band_range(min: f32, max: f32, bands_f: f32, count: usize) -> (usize, usize) {
    let lo = ((min.clamp(0.0, 1.0) * bands_f).floor() as usize).min(count - 1);
    let hi = ((max.clamp(0.0, 1.0) * bands_f).floor() as usize).min(count - 1);
    (lo.min(hi), lo.max(hi))
}

fn curve_is_horizontal(curve: &[f32]) -> bool {
    (curve[1] - curve[3]).abs() <= 0.000001 && (curve[1] - curve[5]).abs() <= 0.000001
}

fn curve_is_vertical(curve: &[f32]) -> bool {
    (curve[0] - curve[2]).abs() <= 0.000001 && (curve[0] - curve[4]).abs() <= 0.000001
}

fn curve_max_x(curve: &[f32]) -> f32 {
    curve[0].max(curve[2]).max(curve[4])
}

fn curve_max_y(curve: &[f32]) -> f32 {
    curve[1].max(curve[3]).max(curve[5])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: [f32; 2], b: [f32; 2]) -> [f32; 8] {
        [
            a[0],
            a[1],
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            b[0],
            b[1],
            0.0,
            0.0,
        ]
    }

    fn indices(data: &[f32], band: usize) -> &[f32] {
        let start = data[band * 4] as usize * 4;
        let count = data[band * 4 + 1] as usize;
        &data[start..start + count]
    }

    #[test]
    fn zero_reach_preserves_glyph_band_bytes() {
        let mut curves = Vec::new();
        for (a, b) in [
            ([0.0, 0.0], [1.0, 0.0]),
            ([1.0, 0.0], [1.0, 1.0]),
            ([1.0, 1.0], [0.0, 1.0]),
            ([0.0, 1.0], [0.0, 0.0]),
        ] {
            curves.extend_from_slice(&line(a, b));
        }
        // A nonzero curve offset and existing band texel also exercise the
        // absolute addresses used when multiple glyph layers share textures.
        let mut data = vec![17.0, 18.0, 19.0, 20.0];
        let mut scratch = BandScratch::default();
        assert_eq!(
            build_bands(
                7,
                &curves,
                &mut data,
                BandOptions {
                    bounds: [0.0, 0.0, 1.0, 1.0],
                    count: 2,
                    reach: 0.0,
                    mode: BandMode::Coverage,
                },
                &mut scratch
            ),
            (1, 2)
        );
        // Exact output of DrawGlyph's original builder for this square:
        // horizontal scans omit curves 0/2; vertical scans omit 1/3.
        let expected: [f32; 36] = [
            17.0, 18.0, 19.0, 20.0, 5.0, 2.0, 0.0, 0.0, 6.0, 2.0, 0.0, 0.0, 7.0, 2.0, 0.0, 0.0,
            8.0, 2.0, 0.0, 0.0, 8.0, 10.0, 0.0, 0.0, 8.0, 10.0, 0.0, 0.0, 9.0, 7.0, 0.0, 0.0, 9.0,
            7.0, 0.0, 0.0,
        ];
        assert_eq!(
            data.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn distance_reach_lists_neighbours_and_keeps_horizontal_edges() {
        let curves = line([0.0, 3.5], [10.0, 3.5]);
        let mut data = Vec::new();
        let mut scratch = BandScratch::default();
        let mut options = BandOptions {
            bounds: [0.0, 0.0, 10.0, 10.0],
            count: 4,
            reach: 0.0,
            mode: BandMode::Distance,
        };
        build_bands(9, &curves, &mut data, options, &mut scratch);
        assert!(indices(&data, 0).is_empty());
        assert_eq!(indices(&data, 1), &[9.0]);
        assert!(indices(&data, 2).is_empty());
        assert!(indices(&data, 3).is_empty());
        // Only four horizontal metadata texels, then one packed index texel.
        assert_eq!(data.len(), 5 * 4);

        data.clear();
        options.reach = 3.0;
        build_bands(9, &curves, &mut data, options, &mut scratch);
        for band in 0..3 {
            assert_eq!(indices(&data, band), &[9.0]);
        }
        assert!(indices(&data, 3).is_empty());

        // Reusing the scratch with fewer bands does not retain old indices.
        data.clear();
        options.count = 1;
        build_bands(2, &curves, &mut data, options, &mut scratch);
        assert_eq!(data, [1.0, 1.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0]);
    }
}
