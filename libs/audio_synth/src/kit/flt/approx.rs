//! The filter family's shared numerics (FILTERS.md section 8 and 3.2):
//! rational tan / tanh / exp / sin / cos, the interpolated (sin, cos) of a
//! cutoff note, and the windowed-sinc kernel of the comb filters.

use std::f64::consts::PI;
use std::sync::OnceLock;

/// tan on (-pi/2, pi/2) by a rational fit.
#[inline]
pub fn tan_r(x: f32) -> f32 {
    let x2 = x * x;
    let num = x * (-135135.0 + x2 * (17325.0 + x2 * (-378.0 + x2)));
    let den = -135135.0 + x2 * (62370.0 + x2 * (-3150.0 + 28.0 * x2));
    num / den
}

/// tanh by a rational fit, input clamped to +-5.
#[inline]
pub fn tanh_r(x: f32) -> f32 {
    let x = x.clamp(-5.0, 5.0);
    let x2 = x * x;
    let num = x * (135135.0 + x2 * (17325.0 + x2 * (378.0 + x2)));
    let den = 135135.0 + x2 * (62370.0 + x2 * (3150.0 + 28.0 * x2));
    num / den
}

/// exp by the [4/4] rational fit (good on roughly -6..4).
#[inline]
pub fn exp_r(x: f32) -> f32 {
    let num = 1680.0 + x * (840.0 + x * (180.0 + x * (20.0 + x)));
    let den = 1680.0 + x * (-840.0 + x * (180.0 + x * (-20.0 + x)));
    num / den
}

/// sin on [-pi, pi] by a rational fit.
#[inline]
pub fn sin_r(x: f32) -> f32 {
    let x2 = x * x;
    let num = x * (11511339840.0 - x2 * (1640635920.0 - x2 * (52785432.0 - 479249.0 * x2)));
    let den = 11511339840.0 + x2 * (277920720.0 + x2 * (3177720.0 + 18361.0 * x2));
    num / den
}

/// cos on [-pi, pi] by a rational fit.
#[inline]
pub fn cos_r(x: f32) -> f32 {
    let x2 = x * x;
    let num = 39251520.0 - x2 * (18471600.0 - x2 * (1075032.0 - 14615.0 * x2));
    let den = 39251520.0 + x2 * (1154160.0 + x2 * (16632.0 + 127.0 * x2));
    num / den
}

/// (sin w, cos w) of a cutoff note n vs A440 at `fs`, w = 2 pi min(0.5,
/// f / fs): exact at integer n, sin and cos interpolated separately in
/// between (not re-normalised), n clamped to the table's -256..255.
pub fn omega_sc(n: f32, fs: f32) -> (f32, f32) {
    let n = n.clamp(-256.0, 255.0);
    let e = n.floor();
    let t = (n - e) as f64;
    let at = |j: f64| {
        let w = 2.0 * PI * (440.0 * (j / 12.0).exp2() / fs as f64).min(0.5);
        w.sin_cos()
    };
    let (s0, c0) = at(e as f64);
    let (s1, c1) = at((e as f64 + 1.0).min(255.0));
    let (s, c) = ((1.0 - t) * s0 + t * s1, (1.0 - t) * c0 + t * c1);
    if super::QUIRK_OMEGA_UNNORMALISED {
        (s as f32, c as f32)
    } else {
        let h = (s * s + c * c).sqrt();
        ((s / h) as f32, (c / h) as f32)
    }
}

/// The comb's interpolation kernel: 257 phases x 12 taps, windowed sinc at
/// 0.455 of the rate.
pub fn sinc_kernel() -> &'static [[f32; 12]; 257] {
    static K: OnceLock<[[f32; 12]; 257]> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = [[0.0f32; 12]; 257];
        for (j, row) in k.iter_mut().enumerate() {
            for (i, v) in row.iter_mut().enumerate() {
                *v = kernel_tap(i, j) as f32;
            }
        }
        k
    })
}

/// One kernel value (t = 5 - i + j/256).
pub fn kernel_tap(i: usize, j: usize) -> f64 {
    let t = 5.0 - i as f64 + j as f64 / 256.0;
    let u = 0.455 * t;
    let sinc = if u == 0.0 { 1.0 } else { (PI * u).sin() / (PI * u) };
    let w = 0.42 - 0.5 * (2.0 * PI * (t - 6.0) / 12.0).cos() + 0.08 * (4.0 * PI * (t - 6.0) / 12.0).cos();
    w * 0.455 * sinc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_fits() {
        for i in -100..=100 {
            let x = i as f32 * 0.015;
            assert!((tan_r(x) - x.tan()).abs() < 1e-5 * (1.0 + x.tan().abs()), "tan {x}");
            assert!((tanh_r(2.0 * x) - (2.0 * x).tanh()).abs() < 1e-5, "tanh {x}");
            let y = 2.0 * x;
            // The [7/6] fits lose accuracy towards +-pi (about 1e-4 at 3).
            let tol = if y.abs() < 2.0 { 1e-5 } else { 3e-4 };
            assert!((sin_r(y) - y.sin()).abs() < tol && (cos_r(y) - y.cos()).abs() < tol, "sc {y}: {} {}", sin_r(y) - y.sin(), cos_r(y) - y.cos());
            assert!((exp_r(x) / x.exp() - 1.0).abs() < 1e-5, "exp {x}");
        }
        let (s, c) = omega_sc(0.0, 96000.0);
        let w = 2.0 * PI * 440.0 / 96000.0;
        assert!((s as f64 - w.sin()).abs() < 1e-7 && (c as f64 - w.cos()).abs() < 1e-7);
    }
}
