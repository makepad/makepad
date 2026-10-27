//! Rotational reprojection of a completed stereo image. The image keeps
//! its render pose and per-eye projections; the display pose can change
//! without rendering the scene again. No device-specific tracking here.

use crate::pose::quat_columns;

/// Metadata that must travel with an eye image, including images supplied
/// by a remote renderer (the stream's render eyes). Axes are columns of each eye-to-head rotation;
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
    /// In order: the left eye's x, y, z columns, the right eye's x, y, z columns, then the source
    /// projections (left, right) and the display projections (left, right).
    pub fn warp(&self, display: &Self) -> [[f32; 4]; 10] {
        let source_head = quat_columns(self.orientation);
        let display_head = quat_columns(display.orientation);
        let column = |eye: usize, axis: usize| {
            let world = rotate(display_head, display.eye_axes[eye][axis]);
            let head = source_head.map(|v| dot(v, world));
            let v = self.eye_axes[eye].map(|v| dot(v, head));
            [v[0], v[1], v[2], 0.0]
        };
        [
            column(0, 0),
            column(0, 1),
            column(0, 2),
            column(1, 0),
            column(1, 1),
            column(1, 2),
            self.projection[0],
            self.projection[1],
            display.projection[0],
            display.projection[1],
        ]
    }

}
