//! The encode's foveation: fewer bits towards an eye column's edges.

use makepad_widgets::makepad_platform::video_file::QpGrid;

/// The foveation's QP delta at the edge of the view (`MAKEPAD_XR_REMOTE_FOVEATE`: a number, or 0
/// for none; default 10: about a third of the bits per pixel out there).
pub(crate) fn foveation_peak() -> Option<i8> {
    let peak = std::env::var("MAKEPAD_XR_REMOTE_FOVEATE").ok().and_then(|v| v.parse::<i8>().ok()).unwrap_or(10);
    (peak > 0).then_some(peak)
}

/// An eye column's QP deltas on the encoder's grid: the colour (the top two thirds) at full
/// quality within 15 degrees of the eye's axis, then fewer bits outwards to `peak` at 50 degrees
/// (the lens blurs the edge; the eye looks through the centre); the band below: coverage as the
/// centre (its edges are the composite's edges), depth with fewer bits.
pub(crate) fn foveation(grid: QpGrid, size: [u32; 2], fov: [f32; 4], peak: i8) -> Vec<i8> {
    let [l, r, u, d] = fov.map(f32::tan);
    let colour_h = size[1] * 2 / 3;
    let mut map = Vec::with_capacity((grid.cols * grid.rows) as usize);
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let x = (col * grid.texel[0] + grid.texel[0] / 2).min(size[0] - 1) as f32;
            let y = row * grid.texel[1] + grid.texel[1] / 2;
            let delta = if y < colour_h {
                let tx = l + (x + 0.5) / size[0] as f32 * (r - l);
                let ty = u - (y as f32 + 0.5) / colour_h as f32 * (u - d);
                let angle = (tx * tx + ty * ty).sqrt().atan().to_degrees();
                let k = ((angle - 15.0) / 35.0).clamp(0.0, 1.0);
                (k * peak as f32).round() as i32
            } else if x < size[0] as f32 * 0.5 {
                0
            } else {
                (peak as i32 / 2).max(1)
            };
            map.push(delta.clamp(grid.min.max(-128), grid.max.min(127)) as i8);
        }
    }
    map
}

