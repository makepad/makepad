//! Nominal UFACTORY 850 URDF joint transforms. Positions are base-frame mm,
//! angles are radians, +Z is up and every joint turns about its local +Z.
//! Source: xArm-Developer/xarm_ros at cf51617d98bc55a88d9aa735acfa43a0bbb8e425,
//! xarm_description/{config/kinematics/default/uf850_default_kinematics.yaml,
//! urdf/uf850/uf850.urdf.xacro,meshes/uf850/visual}. Vendor notice is in
//! resources/UF850-LICENSE.txt. A unit's calibrated kinematics can differ.

pub type Point = [f64; 3];
pub type Bounds = (Point, Point);

pub const JOINT_LIMITS: [(f64, f64); 6] = [
    (-std::f64::consts::TAU, std::f64::consts::TAU),
    (-2.3038346, 2.3038346),
    (-4.2236968, 0.061087),
    (-std::f64::consts::TAU, std::f64::consts::TAU),
    (-2.1642, 2.1642),
    (-std::f64::consts::TAU, std::f64::consts::TAU),
];

// Each row is XYZ (mm), then extrinsic roll/pitch/yaw in radians. Preserve
// the precision of the vendor's model so its rendered CAD and FK agree.
const URDF_ORIGINS: [(Point, Point); 6] = [
    ([0., 0., 364.], [0., 0., 0.]),
    ([0., 0., 0.], [1.5708, -1.5708, 0.]),
    ([390., 0., 0.], [-3.1416, 0., -1.5708]),
    ([150., 426., 0.], [-1.5708, 0., 0.]),
    ([0., 0., 0.], [-1.5708, 0., 0.]),
    ([0., -90., 0.], [1.5708, 0., 0.]),
];

// Extrema of the published link-local visual meshes rounded outwards to mm.
pub(crate) const LINK_LOCAL_BOUNDS: [Bounds; 7] = [
    ([-95., -95., 0.], [95., 95., 185.]),
    ([-61., -60., -179.], [61., 126., 60.]),
    ([-61., -60., -50.], [440., 60., 113.]),
    ([-50., -51., -45.], [195., 160., 121.]),
    ([-45., -116., -266.], [45., 45., 38.]),
    ([-42., -60., -51.], [42., 73., 42.]),
    ([-42., -47., -30.], [42., 42., 0.]),
];

#[derive(Clone, Copy)]
pub(crate) struct Frame {
    pub(crate) origin: Point,
    /// World-space unit vectors of the link's local X/Y/Z.
    pub(crate) axes: [Point; 3],
}

impl Frame {
    const IDENTITY: Self = Self {
        origin: [0.; 3],
        axes: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
    };
    fn rotate(&self, v: Point) -> Point {
        std::array::from_fn(|i| {
            self.axes[0][i] * v[0] + self.axes[1][i] * v[1] + self.axes[2][i] * v[2]
        })
    }
    pub(crate) fn point(&self, v: Point) -> Point {
        let r = self.rotate(v);
        std::array::from_fn(|i| self.origin[i] + r[i])
    }
    fn turn(&mut self, axis: usize, radians: f64) {
        let j = (axis + 1) % 3;
        let k = (axis + 2) % 3;
        let (a, b) = (self.axes[j], self.axes[k]);
        let (s, c) = radians.sin_cos();
        self.axes[j] = std::array::from_fn(|i| c * a[i] + s * b[i]);
        self.axes[k] = std::array::from_fn(|i| -s * a[i] + c * b[i]);
    }
}

pub(crate) fn frames(q: [f64; 6]) -> [Frame; 7] {
    let mut out = [Frame::IDENTITY; 7];
    for (i, (xyz, rpy)) in URDF_ORIGINS.iter().enumerate() {
        let mut f = out[i];
        f.origin = f.point(*xyz);
        f.turn(2, rpy[2]);
        f.turn(1, rpy[1]);
        f.turn(0, rpy[0]);
        f.turn(2, q[i]);
        out[i + 1] = f;
    }
    out
}

/// Mount origin, J1 through J6. J6 is at the bare flange. J1/J2 and J4/J5
/// have coincident origins in the nominal URDF.
pub fn joint_origins(q: [f64; 6]) -> [Point; 7] {
    frames(q).map(|frame| frame.origin)
}

/// Conservative base-frame AABBs of the seven bare links at ONE pose.
/// The boxes include the link offsets around each axis, including the elbow.
pub fn link_boxes(q: [f64; 6]) -> [Bounds; 7] {
    let frames = frames(q);
    std::array::from_fn(|i| {
        let (min, max) = LINK_LOCAL_BOUNDS[i];
        let center = std::array::from_fn(|j| (min[j] + max[j]) * 0.5);
        let half: Point = std::array::from_fn(|j| (max[j] - min[j]) * 0.5);
        let f = frames[i];
        let c = f.point(center);
        let h: Point = std::array::from_fn(|j| (0..3).map(|k| f.axes[k][j].abs() * half[k]).sum());
        (
            std::array::from_fn(|j| c[j] - h[j]),
            std::array::from_fn(|j| c[j] + h[j]),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nominal_zero_and_base_rotation() {
        let p = joint_origins([0.; 6]);
        assert_eq!(p[0], [0.; 3]);
        assert_eq!(p[1], [0., 0., 364.]);
        assert_eq!(p[2], p[1]);
        assert_eq!(p[4], p[5]);
        assert!((p[3][2] - 754.).abs() < 0.01);
        let mut q = [0.; 6];
        q[0] = std::f64::consts::FRAC_PI_2;
        let turned = joint_origins(q);
        for (a, b) in p.iter().zip(turned) {
            assert!(
                (b[0] + a[1]).abs() < 0.01
                    && (b[1] - a[0]).abs() < 0.01
                    && (b[2] - a[2]).abs() < 0.01
            );
        }
        assert!(
            (p[6][0] - 150.).abs() < 0.02 && p[6][1].abs() < 0.02 && (p[6][2] - 238.).abs() < 0.02,
            "{:?}",
            p[6]
        );
    }

    #[test]
    fn link_boxes_include_every_local_corner_at_a_rotated_pose() {
        let q = [0.2, -0.7, -1.1, 0.4, 0.3, 1.2];
        for ((f, (lo, hi)), (a, b)) in frames(q)
            .into_iter()
            .zip(LINK_LOCAL_BOUNDS)
            .zip(link_boxes(q))
        {
            for mask in 0..8 {
                let p = f.point(std::array::from_fn(|k| {
                    if mask & (1 << k) == 0 {
                        lo[k]
                    } else {
                        hi[k]
                    }
                }));
                assert!((0..3).all(|k| p[k] >= a[k] - 1e-8 && p[k] <= b[k] + 1e-8));
            }
        }
    }
}
