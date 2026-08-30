// Modal resonator kernels — the inner loop of the whole instrument.
//
// Every string partial (and every soundboard / sympathetic mode) is one
// exponentially damped complex rotator ("phasor filter"):
//
//     z[k+1] = C * z[k] + i * g_in * x[k],  C = r * e^{i theta}
//     r = exp(-sigma / fs), theta = 2*pi*f / fs
//     y[k] = sum_modes g_out * Im(z[k+1])
//
// This is the modal-synthesis discretisation of the stiff-string PDE modes:
// unconditionally stable for any input because |C| < 1 makes each mode a
// contraction, so bounded input -> bounded state, with no CFL condition to
// violate (the reason modal synthesis was chosen over FDTD; see lib.rs).
//
// Layout is SoA: zr/zi (state), cr/ci (rotation), gin/gout (per-mode gains),
// all padded to a multiple of 8 with zero coefficients so the 4-wide and
// 8-wide kernels can run over the tail without branching (padding modes have
// C = 0, g = 0 and therefore stay exactly zero forever).
//
// Loop order is modes-outer / samples-inner so all per-mode state lives in
// registers across the sample loop; per-sample vector partial sums land in a
// stack accumulator that is collapsed with one horizontal add per sample at
// the end. Accumulation order is fixed and independent of chunk boundaries,
// which is what makes the output bit-identical across host block sizes.

use crate::simd::*;

/// Maximum samples rendered per internal chunk. Also the control-tick grid:
/// the engine only makes control decisions on absolute-sample multiples of
/// this, never on host-buffer boundaries (determinism across block sizes).
pub const MAX_CHUNK: usize = 64;

/// Which kernel implementation to run. Scalar is always available and always
/// correct; Simd4 is NEON on aarch64 / SSE2 on x86_64; Avx2 is runtime-gated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelPath {
    Scalar,
    Simd4,
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

/// Best kernel available on this machine.
pub fn detect_path() -> KernelPath {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") && std::arch::is_x86_feature_detected!("fma") {
            return KernelPath::Avx2;
        }
        KernelPath::Simd4
    }
    #[cfg(target_arch = "aarch64")]
    {
        KernelPath::Simd4
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        KernelPath::Scalar
    }
}

/// Round a mode count up to the kernel lane granularity.
pub fn pad8(n: usize) -> usize {
    (n + 7) & !7
}

/// Run one modal bank for `input.len()` samples (<= MAX_CHUNK).
/// acc[k] += sum_modes gout[m] * Im(z_m[k+1]);  input is scaled by in_gain.
/// All slices over modes have identical length, a multiple of 8.
#[inline]
pub fn run_modes(
    path: KernelPath,
    zr: &mut [f32],
    zi: &mut [f32],
    cr: &[f32],
    ci: &[f32],
    gin: &[f32],
    gout: &[f32],
    input: &[f32],
    in_gain: f32,
    acc: &mut [f32],
) {
    debug_assert!(input.len() <= MAX_CHUNK && acc.len() >= input.len());
    debug_assert!(zr.len() % 8 == 0);
    debug_assert!(
        zi.len() == zr.len() && cr.len() == zr.len() && ci.len() == zr.len() && gin.len() == zr.len() && gout.len() == zr.len()
    );
    match path {
        KernelPath::Scalar => run_modes_scalar(zr, zi, cr, ci, gin, gout, input, in_gain, acc),
        KernelPath::Simd4 => run_modes_simd4(zr, zi, cr, ci, gin, gout, input, in_gain, acc),
        #[cfg(target_arch = "x86_64")]
        KernelPath::Avx2 => unsafe { run_modes_avx2(zr, zi, cr, ci, gin, gout, input, in_gain, acc) },
    }
}

fn run_modes_scalar(
    zr: &mut [f32],
    zi: &mut [f32],
    cr: &[f32],
    ci: &[f32],
    gin: &[f32],
    gout: &[f32],
    input: &[f32],
    in_gain: f32,
    acc: &mut [f32],
) {
    let n = input.len();
    for m in 0..zr.len() {
        let (crm, cim, ginm, goutm) = (cr[m], ci[m], gin[m], gout[m]);
        let (mut r, mut i) = (zr[m], zi[m]);
        for k in 0..n {
            let t = crm * r - cim * i;
            i = cim * r + crm * i + ginm * (in_gain * input[k]);
            r = t;
            acc[k] += goutm * i;
        }
        zr[m] = r;
        zi[m] = i;
    }
}

fn run_modes_simd4(
    zr: &mut [f32],
    zi: &mut [f32],
    cr: &[f32],
    ci: &[f32],
    gin: &[f32],
    gout: &[f32],
    input: &[f32],
    in_gain: f32,
    acc: &mut [f32],
) {
    let n = input.len();
    let mut vacc = [zero_v4(); MAX_CHUNK];
    let mut m = 0;
    while m < zr.len() {
        let mut zrv = load_v4(&zr[m..]);
        let mut ziv = load_v4(&zi[m..]);
        let crv = load_v4(&cr[m..]);
        let civ = load_v4(&ci[m..]);
        let ginv = load_v4(&gin[m..]);
        let goutv = load_v4(&gout[m..]);
        for k in 0..n {
            let f = splat_v4(in_gain * input[k]);
            let t = sub_v4(mul_v4(crv, zrv), mul_v4(civ, ziv));
            ziv = fma_v4(ginv, f, fma_v4(civ, zrv, mul_v4(crv, ziv)));
            zrv = t;
            vacc[k] = fma_v4(goutv, ziv, vacc[k]);
        }
        store_v4(&mut zr[m..], zrv);
        store_v4(&mut zi[m..], ziv);
        m += 4;
    }
    for k in 0..n {
        acc[k] += hsum_v4(vacc[k]);
    }
}

/// 8-wide AVX2+FMA kernel. Compiled on x86_64, selected only when the CPU
/// reports avx2 && fma at runtime.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn run_modes_avx2(
    zr: &mut [f32],
    zi: &mut [f32],
    cr: &[f32],
    ci: &[f32],
    gin: &[f32],
    gout: &[f32],
    input: &[f32],
    in_gain: f32,
    acc: &mut [f32],
) {
    use core::arch::x86_64::*;
    let n = input.len();
    let mut vacc = [_mm256_setzero_ps(); MAX_CHUNK];
    let mut m = 0;
    while m < zr.len() {
        let mut zrv = _mm256_loadu_ps(zr.as_ptr().add(m));
        let mut ziv = _mm256_loadu_ps(zi.as_ptr().add(m));
        let crv = _mm256_loadu_ps(cr.as_ptr().add(m));
        let civ = _mm256_loadu_ps(ci.as_ptr().add(m));
        let ginv = _mm256_loadu_ps(gin.as_ptr().add(m));
        let goutv = _mm256_loadu_ps(gout.as_ptr().add(m));
        for k in 0..n {
            let f = _mm256_set1_ps(in_gain * input[k]);
            let t = _mm256_sub_ps(_mm256_mul_ps(crv, zrv), _mm256_mul_ps(civ, ziv));
            ziv = _mm256_fmadd_ps(ginv, f, _mm256_fmadd_ps(civ, zrv, _mm256_mul_ps(crv, ziv)));
            zrv = t;
            vacc[k] = _mm256_fmadd_ps(goutv, ziv, vacc[k]);
        }
        _mm256_storeu_ps(zr.as_mut_ptr().add(m), zrv);
        _mm256_storeu_ps(zi.as_mut_ptr().add(m), ziv);
        m += 8;
    }
    for k in 0..n {
        // ((lo lanes tree) + (hi lanes tree)) — fixed order.
        let lo = _mm256_castps256_ps128(vacc[k]);
        let hi = _mm256_extractf128_ps(vacc[k], 1);
        let sh = |a: __m128| _mm_add_ps(a, _mm_shuffle_ps(a, a, 0b10_11_00_01));
        let s_lo = sh(lo);
        let s_hi = sh(hi);
        let lo2 = _mm_add_ss(s_lo, _mm_movehl_ps(s_lo, s_lo));
        let hi2 = _mm_add_ss(s_hi, _mm_movehl_ps(s_hi, s_hi));
        acc[k] += _mm_cvtss_f32(_mm_add_ss(lo2, hi2));
    }
}

/// Stereo-tap variant used by the soundboard: the left tap reads Im(z), the
/// right tap reads Re(z) (a free 90-degree per-mode phase offset that
/// decorrelates the channels the way a listener's two ears see a large
/// radiating plate at different phases).
#[inline]
pub fn run_modes_stereo(
    path: KernelPath,
    zr: &mut [f32],
    zi: &mut [f32],
    cr: &[f32],
    ci: &[f32],
    gin: &[f32],
    gout_l: &[f32],
    gout_r: &[f32],
    input: &[f32],
    in_gain: f32,
    acc_l: &mut [f32],
    acc_r: &mut [f32],
) {
    debug_assert!(input.len() <= MAX_CHUNK);
    match path {
        KernelPath::Scalar => {
            let n = input.len();
            for m in 0..zr.len() {
                let (crm, cim, ginm, glm, grm) = (cr[m], ci[m], gin[m], gout_l[m], gout_r[m]);
                let (mut r, mut i) = (zr[m], zi[m]);
                for k in 0..n {
                    let t = crm * r - cim * i;
                    i = cim * r + crm * i + ginm * (in_gain * input[k]);
                    r = t;
                    acc_l[k] += glm * i;
                    acc_r[k] += grm * r;
                }
                zr[m] = r;
                zi[m] = i;
            }
        }
        _ => {
            // 4-wide is plenty for the (shared, small) soundboard bank; the
            // AVX2 machine also takes this path here.
            let n = input.len();
            let mut vacc_l = [zero_v4(); MAX_CHUNK];
            let mut vacc_r = [zero_v4(); MAX_CHUNK];
            let mut m = 0;
            while m < zr.len() {
                let mut zrv = load_v4(&zr[m..]);
                let mut ziv = load_v4(&zi[m..]);
                let crv = load_v4(&cr[m..]);
                let civ = load_v4(&ci[m..]);
                let ginv = load_v4(&gin[m..]);
                let glv = load_v4(&gout_l[m..]);
                let grv = load_v4(&gout_r[m..]);
                for k in 0..n {
                    let f = splat_v4(in_gain * input[k]);
                    let t = sub_v4(mul_v4(crv, zrv), mul_v4(civ, ziv));
                    ziv = fma_v4(ginv, f, fma_v4(civ, zrv, mul_v4(crv, ziv)));
                    zrv = t;
                    vacc_l[k] = fma_v4(glv, ziv, vacc_l[k]);
                    vacc_r[k] = fma_v4(grv, zrv, vacc_r[k]);
                }
                store_v4(&mut zr[m..], zrv);
                store_v4(&mut zi[m..], ziv);
                m += 4;
            }
            for k in 0..n {
                acc_l[k] += hsum_v4(vacc_l[k]);
                acc_r[k] += hsum_v4(vacc_r[k]);
            }
        }
    }
}
