//! Exact ports of the upstream Hunyuan3D-Paint camera math
//! (`hy3dpaint/DifferentiableRenderer/camera_utils.py` and
//! `textureGenPipeline.py` @ 82920d643c0dc2f7bfd7255f45f62d386edfe60c).
//!
//! Conventions (verified against the source): Z-up world; the input elevation
//! is negated and 90° is added to the input azimuth before the spherical
//! placement; the camera looks at `center` with up = +Z; the returned matrix
//! is world-to-camera (w2c) with the camera looking down its local -Z.

/// Row-major 4x4 matrix.
pub type Mat4 = [[f32; 4]; 4];

/// Upstream `MeshRender` default orbit radius.
pub const CAMERA_DISTANCE: f32 = 1.45;
/// Upstream orthographic near/far planes.
pub const ORTHO_NEAR: f32 = 0.0;
pub const ORTHO_FAR: f32 = 2.0;

pub fn mat4_identity() -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

pub fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            let mut acc = 0.0;
            for (k, bk) in b.iter().enumerate() {
                acc += a[i][k] * bk[j];
            }
            out[i][j] = acc;
        }
    }
    out
}

/// `m * [p, 1]`, returning the homogeneous result.
pub fn transform_point(m: &Mat4, p: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for (i, row) in m.iter().enumerate() {
        out[i] = row[0] * p[0] + row[1] * p[1] + row[2] * p[2] + row[3];
    }
    out
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Port of `get_mv_matrix(elev, azim, camera_distance, center)`.
pub fn model_view_matrix(elev_deg: f32, azim_deg: f32, camera_distance: f32, center: [f32; 3]) -> Mat4 {
    let elev = -elev_deg;
    let azim = azim_deg + 90.0;
    let elev_rad = elev.to_radians();
    let azim_rad = azim.to_radians();

    let camera_position = [
        camera_distance * elev_rad.cos() * azim_rad.cos(),
        camera_distance * elev_rad.cos() * azim_rad.sin(),
        camera_distance * elev_rad.sin(),
    ];

    let lookat = normalize3([
        center[0] - camera_position[0],
        center[1] - camera_position[1],
        center[2] - camera_position[2],
    ]);
    let up0 = [0.0, 0.0, 1.0];
    let right = normalize3(cross3(lookat, up0));
    let up = normalize3(cross3(right, lookat));

    // c2w rotation columns are [right, up, -lookat]; w2c = [R^T | -R^T t].
    let rows = [right, up, [-lookat[0], -lookat[1], -lookat[2]]];
    let mut w2c = [[0.0f32; 4]; 4];
    for (i, axis) in rows.iter().enumerate() {
        w2c[i][0] = axis[0];
        w2c[i][1] = axis[1];
        w2c[i][2] = axis[2];
        w2c[i][3] = -(axis[0] * camera_position[0]
            + axis[1] * camera_position[1]
            + axis[2] * camera_position[2]);
    }
    w2c[3][3] = 1.0;
    w2c
}

/// Port of `get_orthographic_projection_matrix`.
pub fn orthographic(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let mut m = mat4_identity();
    m[0][0] = 2.0 / (right - left);
    m[1][1] = 2.0 / (top - bottom);
    m[2][2] = -2.0 / (far - near);
    m[0][3] = -(right + left) / (right - left);
    m[1][3] = -(top + bottom) / (top - bottom);
    m[2][3] = -(far + near) / (far - near);
    m
}

/// Upstream `set_orth_scale`: symmetric box of side `ortho_scale`, near 0, far 2.
pub fn default_orthographic(ortho_scale: f32) -> Mat4 {
    orthographic(
        -ortho_scale * 0.5,
        ortho_scale * 0.5,
        -ortho_scale * 0.5,
        ortho_scale * 0.5,
        ORTHO_NEAR,
        ORTHO_FAR,
    )
}

/// Port of `get_perspective_projection_matrix`.
pub fn perspective(fovy_deg: f32, aspect_wh: f32, near: f32, far: f32) -> Mat4 {
    let fovy_rad = fovy_deg.to_radians();
    let t = (fovy_rad / 2.0).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = 1.0 / (t * aspect_wh);
    m[1][1] = 1.0 / t;
    m[2][2] = -(far + near) / (far - near);
    m[2][3] = -2.0 * far * near / (far - near);
    m[3][2] = -1.0;
    m
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewCandidate {
    pub azim: f32,
    pub elev: f32,
    pub weight: f32,
}

/// The exact candidate set from `Hunyuan3DPaintConfig`: six canonical views
/// (the first six are always selected) followed by a 30°-spaced ring at
/// elevation +20/-20 with weight 0.01, in upstream order.
pub fn candidate_views() -> Vec<ViewCandidate> {
    let mut views = vec![
        ViewCandidate { azim: 0.0, elev: 0.0, weight: 1.0 },
        ViewCandidate { azim: 90.0, elev: 0.0, weight: 0.1 },
        ViewCandidate { azim: 180.0, elev: 0.0, weight: 0.5 },
        ViewCandidate { azim: 270.0, elev: 0.0, weight: 0.1 },
        ViewCandidate { azim: 0.0, elev: 90.0, weight: 0.05 },
        ViewCandidate { azim: 180.0, elev: -90.0, weight: 0.05 },
    ];
    let mut azim = 0;
    while azim < 360 {
        views.push(ViewCandidate { azim: azim as f32, elev: 20.0, weight: 0.01 });
        views.push(ViewCandidate { azim: azim as f32, elev: -20.0, weight: 0.01 });
        azim += 30;
    }
    views
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn front_view_pose() {
        // azim 0, elev 0 -> internal azim 90deg: camera at (0, d, 0) looking at origin.
        let d = CAMERA_DISTANCE;
        let w2c = model_view_matrix(0.0, 0.0, d, [0.0; 3]);
        // Camera position maps to the camera origin.
        let cam = transform_point(&w2c, [0.0, d, 0.0]);
        assert!(approx(cam[0], 0.0, 1e-5) && approx(cam[1], 0.0, 1e-5) && approx(cam[2], 0.0, 1e-5));
        // World origin sits distance d in front of the camera (-Z).
        let origin = transform_point(&w2c, [0.0, 0.0, 0.0]);
        assert!(approx(origin[2], -d, 1e-5), "origin z {}", origin[2]);
        // Basis: right = -X, up = +Z, forward(-Z row) = +Y.
        assert!(approx(w2c[0][0], -1.0, 1e-5));
        assert!(approx(w2c[1][2], 1.0, 1e-5));
        assert!(approx(w2c[2][1], 1.0, 1e-5));
    }

    #[test]
    fn back_view_pose() {
        let d = CAMERA_DISTANCE;
        let w2c = model_view_matrix(0.0, 180.0, d, [0.0; 3]);
        let cam = transform_point(&w2c, [0.0, -d, 0.0]);
        assert!(cam[0].abs() < 1e-5 && cam[1].abs() < 1e-5 && cam[2].abs() < 1e-5);
    }

    #[test]
    fn ortho_matrix_matches_reference() {
        let m = default_orthographic(1.0);
        assert!(approx(m[0][0], 2.0, 1e-6));
        assert!(approx(m[1][1], 2.0, 1e-6));
        assert!(approx(m[2][2], -1.0, 1e-6));
        assert!(approx(m[2][3], -1.0, 1e-6));
        assert!(approx(m[3][3], 1.0, 1e-6));
    }

    #[test]
    fn candidate_set_is_exact() {
        let v = candidate_views();
        assert_eq!(v.len(), 30);
        assert_eq!(v[0], ViewCandidate { azim: 0.0, elev: 0.0, weight: 1.0 });
        assert_eq!(v[2], ViewCandidate { azim: 180.0, elev: 0.0, weight: 0.5 });
        assert_eq!(v[4], ViewCandidate { azim: 0.0, elev: 90.0, weight: 0.05 });
        assert_eq!(v[5], ViewCandidate { azim: 180.0, elev: -90.0, weight: 0.05 });
        assert_eq!(v[6], ViewCandidate { azim: 0.0, elev: 20.0, weight: 0.01 });
        assert_eq!(v[7], ViewCandidate { azim: 0.0, elev: -20.0, weight: 0.01 });
        assert_eq!(v[29], ViewCandidate { azim: 330.0, elev: -20.0, weight: 0.01 });
    }

    #[test]
    fn mat4_mul_identity() {
        let m = model_view_matrix(15.0, 40.0, 1.45, [0.0; 3]);
        let i = mat4_identity();
        assert_eq!(mat4_mul(&m, &i), m);
        assert_eq!(mat4_mul(&i, &m), m);
    }
}
