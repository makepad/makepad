//! The two frequency layouts every envelope uses: the harmonic envelope's
//! mel-spaced grid (dB at [`ENV_POINTS`] frequencies, read between points by
//! linear interpolation on the mel axis) and the noise bands (Bark-like
//! centres, one band-pass each).

/// Points of a harmonic spectral envelope.
pub const ENV_POINTS: usize = 64;
pub const ENV_LO_HZ: f32 = 50.0;
pub const ENV_HI_HZ: f32 = 12_000.0;

/// Noise bands (consonants, breath, the aperiodic part of voiced sound).
pub const NOISE_BANDS: usize = 14;
pub const NOISE_CENTERS: [f32; NOISE_BANDS] =
    [200.0, 400.0, 630.0, 920.0, 1270.0, 1720.0, 2320.0, 3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 9500.0, 11_000.0];

/// A silent level (dB). Levels are amplitudes: 0 dB is a sine of amplitude
/// one for a harmonic, a band noise of RMS one for a noise band.
pub const FLOOR_DB: f32 = -120.0;

pub fn mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

pub fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10f32.powf(mel / 2595.0) - 1.0)
}

/// The frequency of envelope point `i`.
pub fn env_hz(i: usize) -> f32 {
    let (lo, hi) = (mel(ENV_LO_HZ), mel(ENV_HI_HZ));
    mel_to_hz(lo + (hi - lo) * i as f32 / (ENV_POINTS - 1) as f32)
}

/// All envelope point frequencies.
pub fn env_grid() -> [f32; ENV_POINTS] {
    std::array::from_fn(env_hz)
}

/// Fractional envelope point of `hz` (clamped to the grid).
#[inline]
pub fn env_pos(hz: f32) -> f32 {
    const LO: f32 = 77.754_56; // mel(ENV_LO_HZ)
    const SPAN: f32 = 3266.341_2 - LO; // mel(ENV_HI_HZ) - mel(ENV_LO_HZ)
    let m = 2595.0 * (1.0 + hz.max(0.0) / 700.0).log10();
    ((m - LO) / SPAN * (ENV_POINTS - 1) as f32).clamp(0.0, (ENV_POINTS - 1) as f32)
}

/// The envelope's level at `hz`: linear between points on the mel axis,
/// falling 12 dB an octave above the grid.
#[inline]
pub fn env_at(env: &[f32; ENV_POINTS], hz: f32) -> f32 {
    let p = env_pos(hz);
    let i = (p as usize).min(ENV_POINTS - 2);
    let f = p - i as f32;
    let v = env[i] + (env[i + 1] - env[i]) * f;
    if hz > ENV_HI_HZ { v - 12.0 * (hz / ENV_HI_HZ).log2() } else { v }
}

/// -3 dB bandwidth of noise band `b`: its centre distance to its neighbours.
pub fn noise_bandwidth(b: usize) -> f32 {
    let c = NOISE_CENTERS;
    let lo = if b == 0 { c[0] * 0.5 } else { c[b - 1] };
    let hi = if b + 1 == NOISE_BANDS { c[b] * 1.25 } else { c[b + 1] };
    (hi - lo) * 0.5
}

pub fn db_to_lin(db: f32) -> f32 {
    if db <= FLOOR_DB + 0.5 { 0.0 } else { 10f32.powf(db * 0.05) }
}

pub fn lin_to_db(lin: f32) -> f32 {
    if lin <= 1e-6 { FLOOR_DB } else { (20.0 * lin.log10()).max(FLOOR_DB) }
}

/// Quantise a level to a byte (half dB steps from -110 dB): the stored form
/// of every envelope and noise level.
pub fn db_to_code(db: f32) -> u8 {
    ((db + 110.0) * 2.0).round().clamp(0.0, 255.0) as u8
}

pub fn code_to_db(code: u8) -> f32 {
    if code == 0 { FLOOR_DB } else { code as f32 * 0.5 - 110.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_and_codes_round_trip() {
        assert!((env_hz(0) - ENV_LO_HZ).abs() < 0.5);
        assert!((env_hz(ENV_POINTS - 1) - ENV_HI_HZ).abs() < 2.0);
        for i in 0..ENV_POINTS {
            assert!((env_pos(env_hz(i)) - i as f32).abs() < 0.01, "{i}");
        }
        for db in [-100.0f32, -42.5, -3.0, 10.0] {
            assert_eq!(code_to_db(db_to_code(db)), db);
        }
        let mut env = [0.0; ENV_POINTS];
        env[10] = 6.0;
        assert!((env_at(&env, env_hz(10)) - 6.0).abs() < 1e-3);
    }
}
