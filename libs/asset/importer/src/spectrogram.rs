//! Spectrogram thumbnails for audio assets.
//!
//! A waveform strip tells you almost nothing: every mastered track is a
//! filled rectangle. A spectrogram tells you what the thing IS — a kick
//! pattern, a vocal, a pad, a field recording — at a glance and at icon
//! size. This is the picture an audio asset carries in the catalog, baked
//! once by whoever imports or generates it (the icon law: the app is the
//! icon factory, the server only stores what it is handed).
//!
//! Dependency-free: an iterative radix-2 FFT, a log-frequency mapping, and
//! a perceptual colour ramp, with the columns split across cores because a
//! full track is a few thousand transforms and nobody should wait for them.

use std::f32::consts::PI;

/// Thumbnail size. Wide enough that a bar of music is a few pixels, short
/// enough to draw as a card.
pub const THUMB_W: usize = 512;
pub const THUMB_H: usize = 128;

/// Transform size. 1024 samples is ~23 ms at 44.1 kHz: fine enough in time
/// to see a beat, long enough in frequency to see a bass note.
const N_FFT: usize = 1024;
/// Lowest and highest frequency the picture spans. Below 30 Hz is rumble no
/// speaker shows; the top is bounded by Nyquist per track.
const F_MIN: f32 = 30.0;
const F_MAX: f32 = 16_000.0;
/// Dynamic range, in dB below the track's own peak. Everything quieter is
/// floor: a per-track normalisation, so a quiet field recording reads as
/// well as a loud master.
const RANGE_DB: f32 = 78.0;

/// Render mono PCM as a log-frequency spectrogram, RGBA8, `w` by `h`.
///
/// Returns `None` for input too short to transform even once — silence has
/// no picture worth showing, and a black rectangle would be a lie about the
/// thumbnail having been rendered.
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
    // One column per output pixel, spread over the whole track: an icon
    // shows the SHAPE of a piece, not its first two seconds.
    let span = samples.len().saturating_sub(N_FFT);
    let hop = (span as f64 / (w.max(2) - 1) as f64).max(1.0);
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
                    let start = (x as f64 * hop) as usize;
                    *column = fft.column(samples, start, window, bins);
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
    let mut rgba = vec![0u8; w * h * 4];
    for (x, column) in columns.iter().enumerate() {
        for (row, mag) in column.iter().enumerate() {
            let db = 20.0 * (mag / peak).max(1e-9).log10();
            let level = ((db + RANGE_DB) / RANGE_DB).clamp(0.0, 1.0);
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

/// The same picture, PNG-encoded, at the standard thumbnail size.
pub fn spectrogram_png(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    let rgba = spectrogram_rgba(samples, sample_rate, THUMB_W, THUMB_H)?;
    crate::classic_import::encode_png_rgba(&rgba, THUMB_W as u32, THUMB_H as u32).ok()
}

/// Which FFT bins each output row averages, low frequency first. A log
/// scale, so an octave takes the same space everywhere — which is how
/// people hear.
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

/// Dark → indigo → magenta → orange → white. A perceptual ramp: brightness
/// rises monotonically, so loud reads as loud even in greyscale.
fn ramp(level: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 6] = [
        [0.02, 0.02, 0.08],
        [0.18, 0.06, 0.36],
        [0.48, 0.10, 0.48],
        [0.78, 0.24, 0.30],
        [0.96, 0.58, 0.16],
        [1.00, 0.95, 0.85],
    ];
    let t = level.clamp(0.0, 1.0) * (STOPS.len() - 1) as f32;
    let i = (t as usize).min(STOPS.len() - 2);
    let f = t - i as f32;
    let mut out = [0u8; 3];
    for c in 0..3 {
        let v = STOPS[i][c] + (STOPS[i + 1][c] - STOPS[i][c]) * f;
        out[c] = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
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
}

impl Fft {
    fn new() -> Self {
        Self { re: vec![0.0; N_FFT], im: vec![0.0; N_FFT] }
    }

    /// One column: window a frame, transform it, and fold the magnitude
    /// spectrum into the log-frequency rows.
    fn column(
        &mut self,
        samples: &[f32],
        start: usize,
        window: &[f32],
        bins: &[(usize, usize)],
    ) -> Vec<f32> {
        for i in 0..N_FFT {
            let s = samples.get(start + i).copied().unwrap_or(0.0);
            self.re[i] = s * window[i];
            self.im[i] = 0.0;
        }
        self.transform();
        bins.iter()
            .map(|(lo, hi)| {
                let mut peak = 0.0f32;
                for bin in *lo..*hi {
                    let (re, im) = (self.re[bin], self.im[bin]);
                    // The loudest bin in the band, not the average: a
                    // single strong partial must not be averaged away by
                    // the silence around it.
                    peak = peak.max((re * re + im * im).sqrt());
                }
                peak
            })
            .collect()
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

    fn row_of(rgba: &[u8], w: usize, x: usize, y: usize) -> [u8; 3] {
        let o = (y * w + x) * 4;
        [rgba[o], rgba[o + 1], rgba[o + 2]]
    }

    /// The brightest row of a pure tone's picture is the row whose band
    /// contains that tone — the whole point of the thing.
    #[test]
    fn a_tone_lights_its_own_frequency_row() {
        let rate = 44_100;
        let (w, h) = (64, 64);
        let brightest = |freq: f32| {
            let rgba = spectrogram_rgba(&tone(freq, 1.0, rate), rate, w, h).expect("picture");
            (0..h)
                .max_by_key(|y| {
                    let [r, g, b] = row_of(&rgba, w, w / 2, *y);
                    r as u32 + g as u32 + b as u32
                })
                .unwrap()
        };
        let low = brightest(110.0);
        let high = brightest(4_000.0);
        // Pictures grow upward: a higher tone lights a row nearer the top.
        assert!(high < low, "4 kHz row {high} must sit above 110 Hz row {low}");
        // And the same tone lands in the same place every time.
        assert_eq!(brightest(110.0), low);
    }

    /// Normalisation is per track, so a quiet recording is as readable as a
    /// loud one — the same signal at a tenth the amplitude draws the same
    /// picture.
    #[test]
    fn the_picture_is_normalised_to_the_track_not_to_full_scale() {
        let rate = 22_050;
        let loud = tone(440.0, 0.5, rate);
        let quiet: Vec<f32> = loud.iter().map(|s| s * 0.02).collect();
        let a = spectrogram_rgba(&loud, rate, 32, 32).unwrap();
        let b = spectrogram_rgba(&quiet, rate, 32, 32).unwrap();
        assert_eq!(a, b, "amplitude alone must not change the picture");
    }

    /// Colour rises monotonically with level, so loud reads as loud.
    #[test]
    fn the_ramp_gets_brighter_all_the_way_up() {
        let mut last = 0u32;
        for step in 0..=20 {
            let [r, g, b] = ramp(step as f32 / 20.0);
            let sum = r as u32 + g as u32 + b as u32;
            assert!(sum >= last, "ramp dipped at {step}: {sum} < {last}");
            last = sum;
        }
        assert_eq!(ramp(0.0), [5, 5, 20], "the floor is near-black");
        assert!(ramp(1.0).iter().all(|c| *c > 200), "the peak is near-white");
    }

    /// Too short to transform is not a picture. A caller must be able to
    /// tell "no thumbnail" from "a black thumbnail".
    #[test]
    fn silence_and_scraps_have_no_picture() {
        assert!(spectrogram_rgba(&[0.0; 64], 44_100, 32, 32).is_none(), "too short");
        assert!(
            spectrogram_rgba(&vec![0.0; 44_100], 44_100, 32, 32).is_none(),
            "digital silence has nothing to show"
        );
        assert!(spectrogram_rgba(&tone(440.0, 1.0, 44_100), 0, 32, 32).is_none());
    }

    /// The PNG a thumbnail actually ships as.
    #[test]
    fn the_thumbnail_encodes_at_its_standard_size() {
        let png = spectrogram_png(&tone(220.0, 2.0, 44_100), 44_100).expect("png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((w as usize, h as usize), (THUMB_W, THUMB_H));
    }
}
