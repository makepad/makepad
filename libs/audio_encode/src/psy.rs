//! The psychoacoustic model: from an MDCT spectrum to the floor the encoder
//! quantizes against.
//!
//! Deliberately small — critical-band energies, a two-sided spreading walk,
//! an offset below the spread energy, an absolute floor and a per-band crest
//! guard — because the floor *is* the noise shape: residues are quantized in
//! units of the floor curve, so putting the floor at the masking estimate
//! puts the quantization noise there too. The `offset_db` knob (from the
//! caller's quality setting) slides the whole curve, trading bits for noise
//! floor; nothing else needs tuning per file.

use crate::setup::{FLOOR_INTERIOR, FLOOR_POINTS, HALF};

/// Upward masking spread (toward higher bands), dB per critical band.
const SLOPE_UP_DB: f32 = 13.0;
/// Downward spread (toward lower bands), dB per critical band.
const SLOPE_DOWN_DB: f32 = 32.0;
/// Absolute floor amplitude, relative to full-scale MDCT coefficients.
const ABS_FLOOR: f32 = 1.2e-5;
/// Hard ceiling on any guard depth: the floor may never sit more than this
/// far below a bin a point spans, bounding the quantized residue magnitude
/// by construction (44 dB is |q| about 230 after Y rounding, inside the
/// codebook clamp of 256). The guard is evaluated per point over the bins
/// between its neighbouring points, so the linearly interpolated curve
/// between two guarded points cannot dip further below a peak between them.
const GUARD_LIMIT_DB: f32 = 44.0;

/// The four dB anchors one quality setting expands into. `desired_floor`
/// interpolates between the noise and tonal values by each span's measured
/// tonality.
#[derive(Clone, Copy, Debug)]
pub struct PsyTuning {
    /// Masking offset below spread band energy for fully tonal spans.
    pub tonal_offset_db: f32,
    /// Same for fully noise-like spans: noise needs its envelope, not its
    /// waveform.
    pub noise_offset_db: f32,
    /// Guard depth below a span's own peak, tonal spans.
    pub tonal_guard_db: f32,
    /// Guard depth, noise spans.
    pub noise_guard_db: f32,
}

impl PsyTuning {
    /// Map the public 0..1 quality to the four anchors.
    pub fn from_quality(q: f32) -> PsyTuning {
        let q = q.clamp(0.0, 1.0);
        PsyTuning {
            tonal_offset_db: 8.0 + 30.0 * q,
            noise_offset_db: 4.0 + 10.0 * q,
            tonal_guard_db: (30.0 + 16.0 * q).min(GUARD_LIMIT_DB),
            noise_guard_db: 10.0 + 14.0 * q,
        }
    }

    /// Tabulate the two tonality-interpolated exponentials, so the per-point
    /// loop indexes instead of calling `powf` twice per floor point.
    pub fn tables(&self) -> TuningTables {
        TuningTables {
            offset_factor: std::array::from_fn(|i| {
                let t = i as f32 / (TUNE_STEPS - 1) as f32;
                let db = self.noise_offset_db + t * (self.tonal_offset_db - self.noise_offset_db);
                10f32.powf(-db / 10.0)
            }),
            guard_factor: std::array::from_fn(|i| {
                let t = i as f32 / (TUNE_STEPS - 1) as f32;
                let db = self.noise_guard_db + t * (self.tonal_guard_db - self.noise_guard_db);
                10f32.powf(-db / 20.0)
            }),
        }
    }
}

/// Tonality quantisation steps for the tuning tables.
const TUNE_STEPS: usize = 65;

/// Precomputed per-tonality factors for one tuning.
pub struct TuningTables {
    offset_factor: [f32; TUNE_STEPS],
    guard_factor: [f32; TUNE_STEPS],
}
/// Frequencies above this are not coded at all.
const CUTOFF_HZ: f32 = 20_000.0;

pub struct Psy {
    /// Contiguous bin range of each band (band indices rise monotonically).
    band_range: Vec<(u32, u32)>,
    n_bands: usize,
    /// Band index per floor point, in floor list order.
    band_of_point: [usize; FLOOR_POINTS],
    /// Bin range each floor point influences (previous to next point in X
    /// order), in floor list order.
    point_span: [(usize, usize); FLOOR_POINTS],
    /// Wider window per point for the tonality estimate: a partial's MDCT
    /// leakage spreads over several bins, so crest measured over a very
    /// narrow low-frequency span would read pure bass as noise.
    tone_span: [(usize, usize); FLOOR_POINTS],
    /// First bin not coded (everything at and above is forced to zero).
    cutoff_bin: usize,
    up_decay: f32,
    down_decay: f32,
    // scratch, reused per block
    energy: Vec<f32>,
    raw: Vec<f32>,
    peak: Vec<f32>,
    width: Vec<f32>,
    /// |spectrum| per bin.
    mag: Vec<f32>,
    /// Prefix sums of spectrum^2: `psum[k]` is the energy below bin `k`.
    psum: Vec<f32>,
}

fn bark(f: f32) -> f32 {
    13.0 * (0.00076 * f).atan() + 3.5 * ((f / 7500.0) * (f / 7500.0)).atan()
}

impl Psy {
    pub fn new(rate: u32) -> Psy {
        let bin_hz = rate as f32 / (2 * HALF) as f32;
        let mut band_of_bin = Vec::with_capacity(HALF);
        for k in 0..HALF {
            let f = (k as f32 + 0.5) * bin_hz;
            band_of_bin.push(bark(f).floor().max(0.0) as u16);
        }
        let n_bands = *band_of_bin.last().unwrap() as usize + 1;
        let point_x = |i: usize| -> usize {
            match i {
                0 => 0,
                1 => HALF - 1,
                _ => (FLOOR_INTERIOR[i - 2] as usize).min(HALF - 1),
            }
        };
        let band_of_point = std::array::from_fn(|i| band_of_bin[point_x(i)] as usize);
        // X positions in list order, then each point's reach: from the
        // previous point to the next in ascending X order.
        let list_x = |i: usize| -> usize {
            match i {
                0 => 0,
                1 => HALF,
                _ => FLOOR_INTERIOR[i - 2] as usize,
            }
        };
        let mut order: Vec<usize> = (0..FLOOR_POINTS).collect();
        order.sort_by_key(|&i| list_x(i));
        let mut point_span = [(0usize, 0usize); FLOOR_POINTS];
        let mut tone_span = [(0usize, 0usize); FLOOR_POINTS];
        for (s, &i) in order.iter().enumerate() {
            let lo = if s == 0 { 0 } else { list_x(order[s - 1]) };
            let hi = if s + 1 == order.len() { HALF } else { list_x(order[s + 1]).min(HALF) };
            let hi = hi.max(lo + 1).min(HALF);
            point_span[i] = (lo, hi);
            const TONE_WIN: usize = 24;
            if hi - lo >= TONE_WIN {
                tone_span[i] = (lo, hi);
            } else {
                let mid = (lo + hi) / 2;
                let tlo = mid.saturating_sub(TONE_WIN / 2).min(HALF - TONE_WIN);
                tone_span[i] = (tlo, tlo + TONE_WIN);
            }
        }
        let cutoff_bin = ((CUTOFF_HZ / bin_hz) as usize).min(HALF);
        let mut width = vec![0.0f32; n_bands];
        for &b in &band_of_bin {
            width[b as usize] += 1.0;
        }
        let mut band_range = vec![(0u32, 0u32); n_bands];
        for (k, &b) in band_of_bin.iter().enumerate() {
            let r = &mut band_range[b as usize];
            if r.1 == 0 {
                r.0 = k as u32;
            }
            r.1 = k as u32 + 1;
        }
        band_range[band_of_bin[0] as usize].0 = 0;
        Psy {
            band_range,
            n_bands,
            band_of_point,
            point_span,
            tone_span,
            cutoff_bin,
            up_decay: 10f32.powf(-SLOPE_UP_DB / 10.0),
            down_decay: 10f32.powf(-SLOPE_DOWN_DB / 10.0),
            energy: vec![0.0; n_bands],
            raw: vec![0.0; n_bands],
            peak: vec![0.0; n_bands],
            width,
            mag: vec![0.0; HALF],
            psum: vec![0.0; HALF + 1],
        }
    }

    /// First bin that is not coded.
    pub fn cutoff(&self) -> usize {
        self.cutoff_bin
    }

    /// Desired floor Y (0..127, multiplier-2 dB index) per floor point, from
    /// one channel's spectrum.
    pub fn desired_floor(&mut self, spec: &[f32], tuning: &TuningTables, desired: &mut [i32]) {
        debug_assert!(spec.len() >= HALF && desired.len() >= FLOOR_POINTS);
        // Magnitudes and an energy prefix sum once, then every band and span
        // statistic is a contiguous scan or a subtraction.
        let mut acc = 0f32;
        for k in 0..HALF {
            let v = spec[k];
            self.mag[k] = v.abs();
            self.psum[k] = acc;
            acc += v * v;
        }
        self.psum[HALF] = acc;
        for b in 0..self.n_bands {
            let (lo, hi) = self.band_range[b];
            let (lo, hi) = (lo as usize, hi as usize);
            self.energy[b] = self.psum[hi] - self.psum[lo];
            self.peak[b] = self.mag[lo..hi].iter().fold(0f32, |m, &a| m.max(a));
        }
        self.raw.copy_from_slice(&self.energy);
        // Spread: masking leaks into neighbouring bands with asymmetric decay.
        for b in 1..self.n_bands {
            let leaked = self.energy[b - 1] * self.up_decay;
            if leaked > self.energy[b] {
                self.energy[b] = leaked;
            }
        }
        for b in (0..self.n_bands - 1).rev() {
            let leaked = self.energy[b + 1] * self.down_decay;
            if leaked > self.energy[b] {
                self.energy[b] = leaked;
            }
        }
        for i in 0..FLOOR_POINTS {
            let b = self.band_of_point[i];
            // Span statistics: the bins between this point's neighbours.
            let (lo, hi) = self.point_span[i];
            let span_peak = self.mag[lo..hi].iter().fold(0f32, |m, &a| m.max(a));
            // Tonality from the crest factor over the (wider) tone window:
            // a lone partial stands well over the window mean even with its
            // MDCT leakage, noise stays flat. Tonal spans are worth deep
            // coding; noise spans need their envelope, not their waveform —
            // the classic tonality-steered offset, minimally.
            let (tlo, thi) = self.tone_span[i];
            let tone_peak = if (tlo, thi) == (lo, hi) {
                span_peak
            } else {
                self.mag[tlo..thi].iter().fold(0f32, |m, &a| m.max(a))
            };
            let tone_energy = self.psum[thi] - self.psum[tlo];
            let crest_db = if tone_energy > 0.0 {
                10.0 * (tone_peak * tone_peak * (thi - tlo) as f32 / tone_energy).log10()
            } else {
                0.0
            };
            let tonality = ((crest_db - 5.0) / 9.0).clamp(0.0, 1.0);
            let t_idx = (tonality * (TUNE_STEPS - 1) as f32).round() as usize;
            // Per-bin masking amplitude: spread band energy, offset down,
            // averaged over the band's width.
            let mask = (self.energy[b] * tuning.offset_factor[t_idx] / self.width[b]).sqrt();
            // The crest guard also deepens with tonality: tonal peaks are
            // held guard dB above the floor (bounding |q| by construction),
            // noise peaks only enough to keep their envelope.
            let guard = span_peak * tuning.guard_factor[t_idx];
            let target = mask.max(guard).max(ABS_FLOOR);
            desired[i] = amp_to_y(target);
        }
    }
}

/// Amplitude to the floor's multiplier-2 dB index: the exact inverse of the
/// decoder's `inverse_db(y * 2)`.
fn amp_to_y(amp: f32) -> i32 {
    if amp <= 0.0 {
        return 0;
    }
    let idx = 255.0 + (256.0 / 7.0) * (amp as f64).log10();
    ((idx / 2.0).round() as i32).clamp(0, 127)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_audio_decode::vorbis::floor::inverse_db;

    #[test]
    fn amp_to_y_inverts_the_decoder_table() {
        for y in 1..128 {
            let amp = inverse_db(y * 2);
            assert_eq!(amp_to_y(amp), y, "y={y}");
        }
        assert_eq!(amp_to_y(0.0), 0);
        assert_eq!(amp_to_y(2.0), 127);
        assert_eq!(amp_to_y(1e-12), 0);
    }

    #[test]
    fn bands_cover_the_spectrum_contiguously() {
        for rate in [44_100u32, 48_000, 22_050] {
            let p = Psy::new(rate);
            // Ranges tile [0, HALF) exactly, in order, none empty.
            let mut at = 0u32;
            for &(lo, hi) in &p.band_range {
                assert_eq!(lo, at, "{rate}: band starts where the last ended");
                assert!(hi > lo, "{rate}: empty band");
                at = hi;
            }
            assert_eq!(at as usize, HALF);
            assert!(p.n_bands >= 12 && p.n_bands <= 27, "{} bands", p.n_bands);
        }
    }

    #[test]
    fn cutoff_tracks_the_sample_rate() {
        assert!(Psy::new(44_100).cutoff() < HALF);
        assert!(Psy::new(48_000).cutoff() < Psy::new(44_100).cutoff() + 60);
        // At 32 kHz everything is below 20 kHz.
        assert_eq!(Psy::new(32_000).cutoff(), HALF);
    }

    #[test]
    fn a_tone_raises_the_floor_around_itself_and_not_far_away() {
        let mut p = Psy::new(44_100);
        let mut spec = [0.0f32; HALF];
        spec[100] = 0.5; // ~4.3 kHz
        let mut desired = [0i32; FLOOR_POINTS];
        p.desired_floor(&spec, &PsyTuning::from_quality(0.5).tables(), &mut desired);
        // Floor point nearest bin 102 (interior index 16) sits well above the
        // absolute floor; the lowest point (bin 2) stays near it.
        let near = desired[2 + 16];
        let far = desired[2];
        assert!(near > far + 20, "near {near} far {far}");
        // Everything is in range for the 7-bit packet fields.
        assert!(desired.iter().all(|&y| (0..128).contains(&y)));
    }

    #[test]
    fn higher_quality_pushes_the_floor_down() {
        let mut p = Psy::new(44_100);
        let spec = [0.01f32; HALF];
        let mut coarse = [0i32; FLOOR_POINTS];
        let mut fine = [0i32; FLOOR_POINTS];
        p.desired_floor(&spec, &PsyTuning::from_quality(0.1).tables(), &mut coarse);
        p.desired_floor(&spec, &PsyTuning::from_quality(1.0).tables(), &mut fine);
        // A deeper offset means a lower floor: smaller Y.
        assert!(fine[10] < coarse[10], "fine {} coarse {}", fine[10], coarse[10]);
    }
}
