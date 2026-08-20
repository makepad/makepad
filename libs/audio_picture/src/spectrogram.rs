//! The log-frequency spectrogram.
//!
//! A waveform strip tells you almost nothing: every mastered track is a
//! filled rectangle. A spectrogram tells you what the thing IS — a kick
//! pattern, a vocal, a pad, a field recording — and at high definition you
//! can read the arrangement off it: kicks as bright columns along the
//! bottom, hats as ticks along the top, a drop as the moment the whole
//! picture fills.
//!
//! This is the picture an audio asset carries in the catalog, baked once by
//! whoever imports or generates it (the icon law: the app is the icon
//! factory, the server only stores what it is handed), and the same picture
//! the preview widgets draw live from decoded samples.
//!
//! Dependency-free: an iterative radix-2 FFT, a musically weighted
//! log-frequency mapping, and a perceptual colour ramp, with the columns
//! split across cores because a full track at this resolution is tens of
//! thousands of transforms.

use std::f32::consts::PI;

/// High-definition picture size. Wide enough that a bar of music is tens of
/// pixels and a hi-hat is its own tick; 2048 is also exactly the VJ's
/// `MAX_THUMB_DIM`, and both sides sit inside the content contract's
/// 256..=4096 thumbnail bounds.
pub const HD_W: usize = 2048;
pub const HD_H: usize = 512;

/// Transform size: 4096 samples is ~93 ms at 44.1 kHz, which resolves a
/// bass note's pitch instead of smearing it into a band.
const N_FFT: usize = 4096;
/// Frames step a quarter of a window — 75% overlap — so a transient between
/// two column centres still lands in a window that sees it whole.
const HOP: usize = N_FFT / 4;
/// Frames one column peak-holds at most.
///
/// ONE, for a picture you can read. At 2048 columns a 4096-sample window
/// already overlaps its neighbour by ~90% on any track under three minutes,
/// so the transform sees everything; peak-holding several frames into one
/// column instead pushes every column toward the maximum and the
/// arrangement washes out. Only a very long track strides past a window,
/// and there the extra frames buy coverage rather than wash.
const MAX_FRAMES_PER_COLUMN: usize = 2;

/// The band a picture spans. Below 35 Hz is rumble no speaker shows; above
/// 11 kHz is air that costs rows and says little.
const F_MIN: f32 = 35.0;
const F_MAX: f32 = 11_000.0;

/// Dynamic range, in dB below the track's own peak. Everything quieter is
/// floor: a per-track normalisation, so a quiet field recording reads as
/// well as a loud master.
const RANGE_DB: f32 = 72.0;

/// Contrast shaping. Below 1 it lifts the midtones, which is what makes a
/// kick column read as a column rather than a smudge — with no temporal
/// smearing the floor stays dark on its own.
const GAMMA: f32 = 0.85;

/// Render mono PCM as a log-frequency spectrogram, RGBA8, `w` by `h`.
///
/// Returns `None` for input too short to transform even once, or for
/// digital silence — a black rectangle would be a lie about a thumbnail
/// having been rendered.
pub fn spectrogram_rgba(
    samples: &[f32],
    sample_rate: u32,
    w: usize,
    h: usize,
) -> Option<Vec<u8>> {
    if samples.len() < N_FFT || w == 0 || h == 0 || sample_rate == 0 {
        return None;
    }
    let window = hann(N_FFT);
    // One column per output pixel, spread over the WHOLE track: an icon
    // shows the shape of a piece, not its first two seconds.
    let span = samples.len().saturating_sub(N_FFT);
    let stride = (span as f64 / (w.max(2) - 1) as f64).max(1.0);
    // Frames per column: enough to cover the stride at 75% overlap, so no
    // music falls between two columns, bounded so a long track costs the
    // same as a short one per pixel.
    let frames = ((stride / (N_FFT as f64 * 0.75)).ceil() as usize).clamp(1, MAX_FRAMES_PER_COLUMN);
    let bins = bin_rows(sample_rate, h);

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
        .min(w);
    let per_thread = w.div_ceil(threads);
    let mut columns: Vec<Vec<f32>> = vec![Vec::new(); w];
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in columns.chunks_mut(per_thread).enumerate() {
            let (window, bins, samples) = (&window, &bins, samples);
            scope.spawn(move || {
                let mut fft = Fft::new();
                for (i, column) in chunk.iter_mut().enumerate() {
                    let x = chunk_index * per_thread + i;
                    let start = (x as f64 * stride) as usize;
                    *column = fft.column(samples, start, frames, window, bins);
                }
            });
        }
    });

    // Per-track normalisation, in dB against the loudest bin anywhere.
    let peak = columns
        .iter()
        .flat_map(|c| c.iter().copied())
        .fold(0.0f32, f32::max);
    if peak <= 0.0 {
        return None;
    }

    // Levels first, then the fall-off, then colour: the decay has to act on
    // perceived loudness, not on raw magnitude, or a -60 dB tail is
    // invisible anyway.
    let levels: Vec<Vec<f32>> = columns
        .iter()
        .map(|column| {
            column
                .iter()
                .map(|mag| {
                    let db = 20.0 * (mag / peak).max(1e-9).log10();
                    ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0).powf(GAMMA)
                })
                .collect()
        })
        .collect();
    let mut rgba = vec![0u8; w * h * 4];
    for (x, column) in levels.iter().enumerate() {
        for (row, level) in column.iter().copied().enumerate() {
            // Row 0 is the lowest frequency, and pictures grow upward.
            let y = h - 1 - row.min(h - 1);
            let [r, g, b] = ramp(level);
            let o = (y * w + x) * 4;
            rgba[o] = r;
            rgba[o + 1] = g;
            rgba[o + 2] = b;
            rgba[o + 3] = 255;
        }
    }
    Some(rgba)
}

/// Which FFT bins each output row peak-holds, low frequency first: a plain
/// log scale across the band, so an octave takes the same space wherever it
/// sits — which is how people hear.
fn bin_rows(sample_rate: u32, h: usize) -> Vec<(usize, usize)> {
    let nyquist = sample_rate as f32 / 2.0;
    let top = F_MAX.min(nyquist);
    let bottom = F_MIN.min(top / 2.0);
    let bin_hz = sample_rate as f32 / N_FFT as f32;
    let ratio = (top / bottom).ln();
    (0..h)
        .map(|row| {
            let lo_f = bottom * ((row as f32 / h as f32) * ratio).exp();
            let hi_f = bottom * (((row + 1) as f32 / h as f32) * ratio).exp();
            let lo = (lo_f / bin_hz) as usize;
            let hi = ((hi_f / bin_hz).ceil() as usize).clamp(lo + 1, N_FFT / 2);
            (lo.min(N_FFT / 2 - 1), hi)
        })
        .collect()
}

/// Deep navy → indigo → magenta → crimson → orange → pale gold, at the
/// positions that make a track readable: most of the range is spent getting
/// from "floor" to "there is something here", and the top of the ramp is
/// reserved for transients.
pub fn ramp(level: f32) -> [u8; 3] {
    const STOPS: [(f32, [f32; 3]); 6] = [
        (0.00, [6.0, 4.0, 20.0]),
        (0.22, [38.0, 10.0, 74.0]),
        (0.45, [122.0, 22.0, 124.0]),
        (0.66, [219.0, 56.0, 88.0]),
        (0.85, [250.0, 142.0, 52.0]),
        (1.00, [254.0, 228.0, 180.0]),
    ];
    let level = level.clamp(0.0, 1.0);
    let mut i = 0;
    while i + 2 < STOPS.len() && level > STOPS[i + 1].0 {
        i += 1;
    }
    let (a_pos, a) = STOPS[i];
    let (b_pos, b) = STOPS[i + 1];
    let span = (b_pos - a_pos).max(1e-6);
    let f = ((level - a_pos) / span).clamp(0.0, 1.0);
    let mut out = [0u8; 3];
    for c in 0..3 {
        out[c] = (a[c] + (b[c] - a[c]) * f).clamp(0.0, 255.0) as u8;
    }
    out
}

fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect()
}

/// Iterative in-place radix-2 Cooley-Tukey, scratch reused across columns.
struct Fft {
    re: Vec<f32>,
    im: Vec<f32>,
    band: Vec<f32>,
}

impl Fft {
    fn new() -> Self {
        Self { re: vec![0.0; N_FFT], im: vec![0.0; N_FFT], band: Vec::new() }
    }

    /// One column: `frames` overlapping transforms starting at `start`,
    /// peak-held per band, so a transient anywhere in the column's span
    /// lights it.
    fn column(
        &mut self,
        samples: &[f32],
        start: usize,
        frames: usize,
        window: &[f32],
        bins: &[(usize, usize)],
    ) -> Vec<f32> {
        self.band.clear();
        self.band.resize(bins.len(), 0.0);
        for frame in 0..frames.max(1) {
            let base = start + frame * HOP;
            if base >= samples.len() {
                break;
            }
            for i in 0..N_FFT {
                let s = samples.get(base + i).copied().unwrap_or(0.0);
                self.re[i] = s * window[i];
                self.im[i] = 0.0;
            }
            self.transform();
            for (row, (lo, hi)) in bins.iter().enumerate() {
                let mut peak = 0.0f32;
                for bin in *lo..*hi {
                    let (re, im) = (self.re[bin], self.im[bin]);
                    // The loudest bin in the band, not the average: one
                    // strong partial must not be averaged away by the
                    // silence around it.
                    peak = peak.max((re * re + im * im).sqrt());
                }
                if peak > self.band[row] {
                    self.band[row] = peak;
                }
            }
        }
        self.band.clone()
    }

    fn transform(&mut self) {
        let n = N_FFT;
        // Bit-reversal permutation.
        let mut j = 0usize;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                self.re.swap(i, j);
                self.im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let ang = -2.0 * PI / len as f32;
            let (wr, wi) = (ang.cos(), ang.sin());
            let mut i = 0;
            while i < n {
                let (mut cr, mut ci) = (1.0f32, 0.0f32);
                for k in 0..len / 2 {
                    let (a, b) = (i + k, i + k + len / 2);
                    let (xr, xi) = (self.re[b] * cr - self.im[b] * ci,
                                    self.re[b] * ci + self.im[b] * cr);
                    self.re[b] = self.re[a] - xr;
                    self.im[b] = self.im[a] - xi;
                    self.re[a] += xr;
                    self.im[a] += xi;
                    let next = (cr * wr - ci * wi, cr * wi + ci * wr);
                    cr = next.0;
                    ci = next.1;
                }
                i += len;
            }
            len <<= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / rate as f32).sin() * 0.8)
            .collect()
    }

    fn pixel(rgba: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let o = (y * w + x) * 4;
        [rgba[o], rgba[o + 1], rgba[o + 2]]
    }

    fn brightness(rgba: &[u8], w: usize, x: usize, y: usize) -> u32 {
        let [r, g, b] = pixel(rgba, w, x, y);
        r as u32 + g as u32 + b as u32
    }
    #[test]
    fn a_tone_lights_its_own_frequency_row() {
        let rate = 44_100;
        let (w, h) = (64, 128);
        let brightest = |freq: f32| {
            let rgba = spectrogram_rgba(&tone(freq, 3.0, rate), rate, w, h).expect("picture");
            (0..h)
                .max_by_key(|y| brightness(&rgba, w, w / 2, *y))
                .unwrap()
        };
        let low = brightest(110.0);
        let high = brightest(4_000.0);
        assert!(high < low, "4 kHz row {high} must sit above 110 Hz row {low}");
        assert_eq!(brightest(110.0), low, "the same tone lands in the same place");
    }

    #[test]
    fn rows_are_a_plain_log_scale_across_the_band() {
        let rate = 44_100;
        let h = 512;
        let rows = bin_rows(rate, h);
        let bin_hz = rate as f32 / N_FFT as f32;
        let row_of = |freq: f32| {
            rows.iter()
                .position(|(lo, hi)| {
                    let bin = (freq / bin_hz) as usize;
                    bin >= *lo && bin < *hi
                })
                .unwrap_or(h - 1)
        };
        let low_octave = row_of(200.0) as i32 - row_of(100.0) as i32;
        let high_octave = row_of(4_000.0) as i32 - row_of(2_000.0) as i32;
        assert!(
            (low_octave - high_octave).abs() <= 8,
            "octaves take the same room: {low_octave} vs {high_octave} rows"
        );
        assert!(row_of(35.0) < 4, "the band starts at its bottom");
        assert!(row_of(11_000.0) > h - 8, "and ends at its top");
    }

    #[test]
    fn a_beat_reads_as_separate_columns() {
        let rate = 44_100;
        let (w, h) = (256, 128);
        let mut samples = vec![0.0f32; rate as usize * 4];
        for beat in 0..8 {
            let at = beat * rate as usize / 2;
            for i in 0..(rate as usize / 20) {
                let t = i as f32 / rate as f32;
                let env = (-t * 40.0).exp();
                if at + i < samples.len() {
                    samples[at + i] = (2.0 * PI * 60.0 * t).sin() * env;
                }
            }
        }
        let rgba = spectrogram_rgba(&samples, rate, w, h).expect("picture");
        // Bottom rows: the loudest columns are the beats, and there are
        // clearly gaps between them.
        let col_brightness: Vec<u32> = (0..w)
            .map(|x| (h - 8..h).map(|y| brightness(&rgba, w, x, y)).sum())
            .collect();
        let peak = *col_brightness.iter().max().unwrap();
        let quiet = col_brightness.iter().filter(|b| **b < peak / 3).count();
        assert!(quiet > w / 3, "a beat has gaps: only {quiet}/{w} quiet columns");
        let loud = col_brightness.iter().filter(|b| **b > peak / 2).count();
        assert!((4..=64).contains(&loud), "eight kicks, not a smear: {loud} loud columns");
    }

    #[test]
    fn the_picture_is_normalised_to_the_track_not_to_full_scale() {
        let rate = 22_050;
        let loud = tone(440.0, 2.0, rate);
        let quiet: Vec<f32> = loud.iter().map(|s| s * 0.02).collect();
        let a = spectrogram_rgba(&loud, rate, 32, 64).unwrap();
        let b = spectrogram_rgba(&quiet, rate, 32, 64).unwrap();
        // The same picture, within the noise floor: at a fiftieth of the
        // amplitude the leakage between bins reaches the dB floor a shade
        // sooner, which is a pixel or two of the darkest colour and nothing
        // a person could see.
        let worst = a
            .iter()
            .zip(&b)
            .map(|(x, y)| x.abs_diff(*y) as u32)
            .max()
            .unwrap_or(0);
        assert!(worst <= 6, "a quiet track draws the same picture (worst channel delta {worst})");
        let bright_row = |rgba: &[u8]| {
            (0..64)
                .max_by_key(|y| brightness(rgba, 32, 16, *y))
                .unwrap()
        };
        assert_eq!(
            bright_row(&a),
            bright_row(&b),
            "and its tone lands in the same row"
        );
    }

    #[test]
    fn the_ramp_gets_brighter_all_the_way_up() {
        let mut last = 0u32;
        for step in 0..=20 {
            let [r, g, b] = ramp(step as f32 / 20.0);
            let sum = r as u32 + g as u32 + b as u32;
            assert!(sum >= last, "ramp dipped at {step}: {sum} < {last}");
            last = sum;
        }
        assert_eq!(ramp(0.0), [6, 4, 20], "the floor is deep navy");
        assert_eq!(ramp(1.0), [254, 228, 180], "the peak is pale gold");
        // The stops are where the recipe puts them.
        assert_eq!(ramp(0.22), [38, 10, 74]);
        assert_eq!(ramp(0.66), [219, 56, 88]);
    }

    #[test]
    fn silence_and_scraps_have_no_picture() {
        assert!(spectrogram_rgba(&[0.0; 64], 44_100, 32, 32).is_none(), "too short");
        assert!(
            spectrogram_rgba(&vec![0.0; 44_100], 44_100, 32, 32).is_none(),
            "digital silence has nothing to show"
        );
        assert!(spectrogram_rgba(&tone(440.0, 1.0, 44_100), 0, 32, 32).is_none());
    }

}
