//! A compact psychoacoustic model: block switching from a time-domain attack
//! detector, and per-band allowed noise from the MDCT spectrum itself (band
//! energy spread to the neighbouring bands, a fixed signal-to-mask offset and
//! the absolute threshold of hearing). Far simpler than the standard's model
//! 2, and enough at the rates this encoder is for (160 kbit/s and up), where
//! the bit budget rarely lets the noise reach the threshold anyway.

use super::analysis::LINES;
use super::bands::Bands;

/// Signal-to-mask offsets in dB for long and short blocks.
const SMR_LONG_DB: f32 = 21.0;
const SMR_SHORT_DB: f32 = 15.0;

/// Energy of one MDCT line carrying a full-scale sine (amplitude 1) in a long
/// block, measured from this filterbank; the threshold of hearing is placed
/// relative to it, with a full-scale sine at 96 dB SPL.
const FULL_SCALE_LINE_ENERGY: f32 = 0.25;
const FULL_SCALE_DB_SPL: f32 = 96.0;

/// Absolute threshold of hearing in dB SPL at `hz` (the usual fit to the
/// measured curve), floored a little below its minimum.
fn ath_db(hz: f32) -> f32 {
    let f = (hz / 1000.0).clamp(0.02, 20.0);
    let db = 3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3) * (f - 3.3)).exp() + 1e-3 * f.powi(4);
    db.max(-5.0).min(96.0)
}

/// Allowed noise energy per band and the granule's
/// perceptual entropy (bits the model thinks the content needs).
pub struct Masking {
    pub allowed: [f32; 39],
    pub pe: f32,
}

pub fn masking(xr: &[f32; LINES], bands: &Bands, rate: u32) -> Masking {
    let n = bands.count;
    let mut energy = [0.0f32; 39];
    for b in 0..n {
        let (start, width) = (bands.start[b] as usize, bands.width[b] as usize);
        energy[b] = xr[start..start + width].iter().map(|v| v * v).sum();
    }
    // Spreading in energy density, within one window's run of bands: a band
    // masks upward more than downward.
    let stride = if bands.short { 3 } else { 1 };
    let smr = 10f32.powf(-(if bands.short { SMR_SHORT_DB } else { SMR_LONG_DB }) / 10.0);
    let mut allowed = [0.0f32; 39];
    let mut pe = 0.0f32;
    let lines_per_window = if bands.short { 192.0 } else { 576.0 };
    let ath_scale = if bands.short { 1.0 / 3.0 } else { 1.0 };
    for b in 0..n {
        let width = bands.width[b] as f32;
        let density = |i: usize| energy[i] / bands.width[i] as f32;
        let mut spread = density(b);
        if b >= stride {
            spread += 0.12 * density(b - stride);
        }
        if b + stride < n {
            spread += 0.03 * density(b + stride);
        }
        let mask = spread * width * smr;
        // The threshold of hearing at the band's lowest line (the most
        // sensitive part of the band dominates what is heard).
        let line = bands.line_in_window[b] as f32;
        let hz = (line + 0.5) * rate as f32 * 0.5 / lines_per_window;
        let ath = FULL_SCALE_LINE_ENERGY * ath_scale * 10f32.powf((ath_db(hz) - FULL_SCALE_DB_SPL) / 10.0) * width;
        allowed[b] = mask.max(ath).max(1e-20);
        if energy[b] > allowed[b] {
            pe += width * (energy[b] / allowed[b]).log2() * 0.5;
        }
    }
    Masking { allowed, pe }
}

/// Attack detection for one granule of one channel: does a sharp rise in
/// high-passed energy fall inside the samples the granule's long window would
/// smear backwards? `pcm` is the channel, `granule` the granule index.
pub fn attack(pcm: &[f32], granule: i64) -> bool {
    // Granule g's MDCT window is centred about 288 samples into granule g,
    // minus the polyphase filter's 240-sample delay; look at the 576 samples
    // around it in eight sub-blocks, with the eight before as the reference.
    const SUB: i64 = 72;
    let centre = granule * 576 + 288 - 240;
    let first = centre - 288 - 8 * SUB;
    let mut e = [0.0f32; 16];
    let sample = |i: i64| if i < 0 || i as usize >= pcm.len() { 0.0 } else { pcm[i as usize] };
    for (blk, slot) in e.iter_mut().enumerate() {
        let start = first + blk as i64 * SUB;
        let mut acc = 0.0f32;
        let mut prev = sample(start - 1);
        for i in start..start + SUB {
            let x = sample(i);
            let h = x - prev;
            prev = x;
            acc += h * h;
        }
        *slot = acc;
    }
    let floor = 1e-5 * SUB as f32;
    let mut reference = e[..8].iter().copied().fold(0.0f32, f32::max);
    for blk in 8..16 {
        if e[blk] > floor && e[blk] > 10.0 * reference.max(floor * 0.1) {
            return true;
        }
        reference = reference.max(e[blk]);
    }
    false
}
