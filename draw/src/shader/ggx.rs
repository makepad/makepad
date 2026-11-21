//! Split-sum terms for environment reflections. E is the directional albedo at
//! F0 = 1, and B is the Schlick share at F0 = 0. Together they give the
//! reflectance `F0 * (E - B) + B` for any F0.
//!
//! The fit integrates the GGX distribution with `alpha = rough²`,
//! height-correlated Smith visibility `0.5 / (gv + gl)`, and Schlick Fresnel.
//! Each term is a polynomial of degree four in both roughness (rows) and
//! `sqrt(n·v)` (columns). For `n·v >= 0.05`, the maximum absolute errors are
//! 0.027 in E and 0.035 in B. In a white furnace, `E_true / E_fit` is within
//! 3% of one. The fit is weighted least squares (weight n·v) over a 32 x 32
//! grid, n·v in [0.02, 1] and roughness in [0, 1], each cell integrated with
//! 4096 GGX importance samples of the lobe above.
//!
//! Shared by the raster's lit lanes and the path tracer: E also gives the
//! energy GGX single scattering loses, which a lobe takes back with
//! `1 + F0 * (1 / E - 1)`. A white metal (F0 = 1) then keeps E_true / E_fit
//! of the light: 0.98 to 1.01 for n·v >= 0.3, 0.96 to 1.03 down to
//! n·v = 0.05 (against the lobe integrated over roughness 0.05 to 1).
//! `mod.draw.ggx_split_sum` is the shader form of [`ggx_split_sum`].
use crate::makepad_platform::*;

pub const GGX_E: [[f32; 5]; 5] = [
    [0.957185, 0.683125, -2.284411, 2.711845, -1.061016],
    [-1.799458, 0.324076, 22.586309, -39.608093, 18.203463],
    [12.717986, -48.749575, 12.453794, 75.423160, -49.576835],
    [-22.276850, 109.976394, -122.827198, -11.225048, 40.790733],
    [11.548468, -63.804640, 91.149534, -27.641604, -8.364680],
];

pub const GGX_B: [[f32; 5]; 5] = [
    [1.144837, -0.347347, -7.678551, 12.054197, -5.177841],
    [-5.910787, 38.428892, -83.363226, 74.998232, -24.142457],
    [12.092375, -143.290584, 398.495000, -418.716924, 151.449588],
    [-10.414114, 177.280745, -538.581622, 593.389713, -221.756438],
    [3.115612, -72.230068, 231.590169, -262.300450, 99.871319],
];

/// Returns the directional albedo and Schlick share as `[E, B]`.
pub fn ggx_split_sum(nv: f32, rough: f32) -> [f32; 2] {
    let s = nv.clamp(0.02, 1.0).sqrt();
    let r = rough.clamp(0.0, 1.0);
    let fit = |coeffs: &[[f32; 5]; 5]| {
        let column = |j: usize| {
            (0..4).rev().fold(coeffs[4][j], |p, i| p * r + coeffs[i][j])
        };
        (0..4).rev().fold(column(4), |p, j| p * s + column(j))
    };
    let e = fit(&GGX_E).clamp(0.05, 1.0);
    let b = fit(&GGX_B).clamp(0.0, e);
    [e, b]
}

script_mod! {
    use mod.pod.*
    use mod.math.*

    // Returns the GGX directional albedo E and Schlick share B.
    // Its CPU twin is `ggx_split_sum`.
    mod.draw.ggx_split_sum = fn(nv: float, rough: float) -> vec2 {
        let s = sqrt(clamp(nv, 0.02, 1.0))
        let r = clamp(rough, 0.0, 1.0)
        let e0 = (((#(GGX_E[4][0]) * r + #(GGX_E[3][0])) * r + #(GGX_E[2][0])) * r + #(GGX_E[1][0])) * r + #(GGX_E[0][0])
        let e1 = (((#(GGX_E[4][1]) * r + #(GGX_E[3][1])) * r + #(GGX_E[2][1])) * r + #(GGX_E[1][1])) * r + #(GGX_E[0][1])
        let e2 = (((#(GGX_E[4][2]) * r + #(GGX_E[3][2])) * r + #(GGX_E[2][2])) * r + #(GGX_E[1][2])) * r + #(GGX_E[0][2])
        let e3 = (((#(GGX_E[4][3]) * r + #(GGX_E[3][3])) * r + #(GGX_E[2][3])) * r + #(GGX_E[1][3])) * r + #(GGX_E[0][3])
        let e4 = (((#(GGX_E[4][4]) * r + #(GGX_E[3][4])) * r + #(GGX_E[2][4])) * r + #(GGX_E[1][4])) * r + #(GGX_E[0][4])
        let e = clamp((((e4 * s + e3) * s + e2) * s + e1) * s + e0, 0.05, 1.0)
        let b0 = (((#(GGX_B[4][0]) * r + #(GGX_B[3][0])) * r + #(GGX_B[2][0])) * r + #(GGX_B[1][0])) * r + #(GGX_B[0][0])
        let b1 = (((#(GGX_B[4][1]) * r + #(GGX_B[3][1])) * r + #(GGX_B[2][1])) * r + #(GGX_B[1][1])) * r + #(GGX_B[0][1])
        let b2 = (((#(GGX_B[4][2]) * r + #(GGX_B[3][2])) * r + #(GGX_B[2][2])) * r + #(GGX_B[1][2])) * r + #(GGX_B[0][2])
        let b3 = (((#(GGX_B[4][3]) * r + #(GGX_B[3][3])) * r + #(GGX_B[2][3])) * r + #(GGX_B[1][3])) * r + #(GGX_B[0][3])
        let b4 = (((#(GGX_B[4][4]) * r + #(GGX_B[3][4])) * r + #(GGX_B[2][4])) * r + #(GGX_B[1][4])) * r + #(GGX_B[0][4])
        let b = clamp((((b4 * s + b3) * s + b2) * s + b1) * s + b0, 0.0, e)
        return vec2(e, b)
    }
}
