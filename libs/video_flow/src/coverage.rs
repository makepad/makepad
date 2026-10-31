//! ROW / COLUMN COVERAGE of a decoded luma plane: which parts of a frame are
//! flat (one value across the whole line) and so look "blank". Used to tell a
//! clip FILE that is half empty from a PLAYER that shows a good file half
//! empty: decode the file frame by frame and measure the planes it holds.

/// Flatness of one frame's luma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coverage {
    /// Rows whose luma spans at most `tolerance` from darkest to brightest.
    pub flat_rows: usize,
    /// The longest run of consecutive flat rows.
    pub longest_flat_rows: usize,
    /// The same for columns.
    pub flat_cols: usize,
    pub longest_flat_cols: usize,
    pub width: usize,
    pub height: usize,
}

impl Coverage {
    /// The share of the frame's height the longest flat band of rows covers.
    pub fn flat_row_band(&self) -> f32 {
        self.longest_flat_rows as f32 / self.height.max(1) as f32
    }
    pub fn flat_col_band(&self) -> f32 {
        self.longest_flat_cols as f32 / self.width.max(1) as f32
    }
}

fn longest_run(flags: &[bool]) -> usize {
    let (mut best, mut run) = (0, 0);
    for &f in flags {
        run = if f { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// Measure `luma` (`width` x `height`, `stride` bytes per row).
pub fn measure(luma: &[u8], width: usize, height: usize, stride: usize, tolerance: u8) -> Coverage {
    let mut rows = vec![false; height];
    let mut cmin = vec![255u8; width];
    let mut cmax = vec![0u8; width];
    for y in 0..height {
        let row = &luma[y * stride..y * stride + width];
        let (mut lo, mut hi) = (255u8, 0u8);
        for (x, &v) in row.iter().enumerate() {
            lo = lo.min(v);
            hi = hi.max(v);
            cmin[x] = cmin[x].min(v);
            cmax[x] = cmax[x].max(v);
        }
        rows[y] = hi - lo <= tolerance;
    }
    let cols: Vec<bool> = (0..width).map(|x| cmax[x] - cmin[x] <= tolerance).collect();
    Coverage {
        flat_rows: rows.iter().filter(|f| **f).count(),
        longest_flat_rows: longest_run(&rows),
        flat_cols: cols.iter().filter(|f| **f).count(),
        longest_flat_cols: longest_run(&cols),
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy(width: usize, height: usize) -> Vec<u8> {
        (0..width * height).map(|i| ((i * 37 + (i / width) * 11) % 251) as u8).collect()
    }

    #[test]
    fn a_full_frame_has_no_flat_band() {
        let c = measure(&noisy(64, 48), 64, 48, 64, 2);
        assert_eq!((c.flat_rows, c.flat_cols), (0, 0));
    }

    #[test]
    fn a_frame_whose_bottom_half_is_empty_reports_the_band() {
        let mut p = noisy(64, 48);
        for v in &mut p[64 * 24..] {
            *v = 16;
        }
        let c = measure(&p, 64, 48, 64, 2);
        assert_eq!(c.longest_flat_rows, 24);
        assert!((c.flat_row_band() - 0.5).abs() < 1e-6);
        assert_eq!(c.flat_cols, 0);
    }

    #[test]
    fn stride_padding_is_not_measured() {
        let p = noisy(64, 48);
        let wide: Vec<u8> = (0..48).flat_map(|y| p[y * 64..y * 64 + 64].iter().copied().chain([7u8; 16])).collect();
        assert_eq!(measure(&wide, 64, 48, 80, 2), measure(&p, 64, 48, 64, 2));
    }
}
