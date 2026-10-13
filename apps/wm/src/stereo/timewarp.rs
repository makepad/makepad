//! Rotational reprojection of a completed stereo image. The image keeps
//! its render pose and per-eye projections; the display pose can change
//! without rendering the scene again. No device-specific tracking here.

use makepad_widgets::{live_id, LiveId};
use super::tracking::quat_columns;

/// Metadata that must travel with an eye image, including images supplied
/// by a future 3D client. Axes are columns of each eye-to-head rotation;
/// projections are (x scale, x offset, y scale, y offset).
#[derive(Clone, Copy, Debug)]
pub struct EyeFrame {
    pub orientation: [f32; 4],
    pub eye_axes: [[[f32; 3]; 3]; 2],
    pub projection: [[f32; 4]; 2],
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn rotate(columns: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|row| columns[0][row] * v[0] + columns[1][row] * v[1] + columns[2][row] * v[2])
}

impl EyeFrame {
    /// Inverse mapping: source_eye^-1 * source_head^-1 * display_head *
    /// display_eye. A destination ray is rotated into the old image and
    /// projected there, as in Monado's render_calc_time_warp_matrix.
    pub fn uniforms(&self, display: &Self) -> [(LiveId, [f32; 4]); 10] {
        let source_head = quat_columns(self.orientation);
        let display_head = quat_columns(display.orientation);
        let column = |eye: usize, axis: usize| {
            let world = rotate(display_head, display.eye_axes[eye][axis]);
            let head = source_head.map(|v| dot(v, world));
            let v = self.eye_axes[eye].map(|v| dot(v, head));
            [v[0], v[1], v[2], 0.0]
        };
        [
            (live_id!(tw_lx), column(0, 0)),
            (live_id!(tw_ly), column(0, 1)),
            (live_id!(tw_lz), column(0, 2)),
            (live_id!(tw_rx), column(1, 0)),
            (live_id!(tw_ry), column(1, 1)),
            (live_id!(tw_rz), column(1, 2)),
            (live_id!(tw_src_l), self.projection[0]),
            (live_id!(tw_src_r), self.projection[1]),
            (live_id!(tw_dst_l), display.projection[0]),
            (live_id!(tw_dst_r), display.projection[1]),
        ]
    }

}
