//! Lanczos (a = 4) interpolating read of a sample ring, shared by the twist
//! oscillator and the nimbus effect for their fixed-rate engines (our own code).

/// Kernel half-width.
pub const LANCZOS_A: i64 = 4;

/// Lanczos (a = 4) read of a ring at absolute position `pos` (sample `n`
/// lives at `ring[n % RING]`; negative indices read as zero).
#[inline]
pub fn lanczos(ring: &[f32], pos: f64) -> f32 {
    let n = ring.len() as i64;
    let i0 = pos.floor();
    let f = (pos - i0) as f32;
    let i0 = i0 as i64;
    if f < 1e-7 {
        return if i0 >= 0 { ring[(i0 % n) as usize] } else { 0.0 };
    }
    use std::f32::consts::PI;
    // sinc(f - k) sinc((f - k)/4), k = -3..=4, with sin(pi (f-k)) = (-1)^k sin(pi f)
    // and sin(pi (f-k)/4) from the angle sum.
    let spf = (PI * f).sin();
    let (s4, c4) = (PI * f / 4.0).sin_cos();
    const SK: [f32; 8] = [
        -0.70710678, -1.0, -0.70710678, 0.0, 0.70710678, 1.0, 0.70710678, 0.0,
    ]; // sin(k pi / 4), k = -3..=4
    const CK: [f32; 8] = [
        -0.70710678, 0.0, 0.70710678, 1.0, 0.70710678, 0.0, -0.70710678, -1.0,
    ]; // cos(k pi / 4)
    let mut acc = 0.0f32;
    for (t, k) in (-(LANCZOS_A - 1)..=LANCZOS_A).enumerate() {
        let j = i0 + k;
        if j < 0 {
            continue;
        }
        let d = f - k as f32;
        let sign = if k & 1 == 0 { 1.0 } else { -1.0 };
        let s_pi = sign * spf;
        let s_q = s4 * CK[t] - c4 * SK[t];
        let w = (s_pi / (PI * d)) * (s_q / (PI * d / 4.0));
        acc += ring[(j % n) as usize] * w;
    }
    acc
}
