//! The waveform strip.
//!
//! # The one rule that makes a wave strip honest
//!
//! A column's value is the LOUDEST SAMPLE in that column's slice of time,
//! measured against the track's own peak. Nothing here is ever divided by
//! how many samples fell in the bucket, or by how long the track is. That
//! division is exactly the bug this module exists to keep out: a six-minute
//! song puts forty times more samples in a column than a ten-second clip, so
//! a mean — or a sum rescaled by count, or an amplitude scaled by duration —
//! draws the long track as a flat line and the short one as a solid block,
//! and the picture stops being about the music at all.
//!
//! RMS is the deliberate exception, and it is not the same quantity: it is
//! the root-mean-square of the samples in the bucket, a per-sample energy
//! that does not shrink as the bucket grows. It draws the solid body inside
//! the translucent peak envelope, which is what makes a mastered track read
//! as loud-and-dense rather than as a rectangle.
//!
//! # Antialiasing
//!
//! Columns are measured at [`SUPERSAMPLE`]× the output width and resolved by
//! coverage, horizontally and vertically: an edge pixel gets the fraction of
//! itself the envelope actually covers rather than a hard on/off. The peak
//! envelope is smoothed at sub-column resolution before rasterising, so the
//! outline is a curve instead of a comb, without blurring anything a viewer
//! could see at output resolution.

/// Sub-columns measured per output column. Four is where the outline stops
/// reading as stair-steps; eight costs twice as much for a difference you
/// have to look for.
pub const SUPERSAMPLE: usize = 4;

/// Colours of a strip, as straight RGB. The picture is opaque; translucency
/// in the peak band is composited here rather than left to the consumer, so
/// a strip drops into a JPEG thumbnail and a live texture identically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WavePalette {
    pub background: [u8; 3],
    /// The outer envelope: the loudest sample in each column. Drawn
    /// translucent over the background.
    pub peak: [u8; 3],
    /// How much of the peak colour reaches the background, 0..=1.
    pub peak_alpha: f32,
    /// The inner body: RMS energy. Solid.
    pub rms: [u8; 3],
    /// The zero line.
    pub centre: [u8; 3],
}

impl Default for WavePalette {
    fn default() -> Self {
        Self {
            background: [10, 12, 18],
            peak: [88, 196, 160],
            peak_alpha: 0.45,
            rms: [148, 235, 205],
            centre: [42, 50, 56],
        }
    }
}

impl WavePalette {
    /// The palette for the strip that sits under a spectrogram: the same
    /// deep navy ground the ramp starts from, so the composite reads as one
    /// picture with a strip along its edge rather than two pasted images.
    pub fn under_spectrogram() -> Self {
        Self {
            background: [6, 4, 20],
            peak: [219, 56, 88],
            peak_alpha: 0.5,
            rms: [250, 142, 52],
            centre: [38, 10, 74],
        }
    }
}

/// One measured sub-column, both values already normalised to the track's
/// own peak so they are directly a fraction of the strip's half-height.
#[derive(Clone, Copy, Default)]
struct Column {
    peak: f32,
    rms: f32,
}

/// Render `samples` as a `w`×`h` RGBA8 waveform strip.
///
/// `None` for an empty signal or a zero-sized picture, and for digital
/// silence — a flat line drawn from nothing is a lie about having measured
/// something, and the caller must be able to tell that apart from a quiet
/// track (which normalises up and draws fine).
pub fn wave_rgba(samples: &[f32], w: usize, h: usize, palette: WavePalette) -> Option<Vec<u8>> {
    if samples.is_empty() || w == 0 || h == 0 {
        return None;
    }
    let columns = measure(samples, w * SUPERSAMPLE)?;
    Some(rasterise(&columns, w, h, palette))
}

/// Measure `n` sub-columns: the loudest sample and the RMS energy of each
/// fixed slice of time, both divided by the track's own peak at the end and
/// by nothing else.
fn measure(samples: &[f32], n: usize) -> Option<Vec<Column>> {
    let n = n.max(1);
    let mut columns = vec![Column::default(); n];
    // A fixed slice of time per column, spread over the WHOLE signal: a
    // strip shows the shape of a piece, not its first two seconds.
    let per = samples.len() as f64 / n as f64;
    let mut track_peak = 0.0f32;
    for (i, column) in columns.iter_mut().enumerate() {
        let start = ((i as f64 * per) as usize).min(samples.len() - 1);
        let end = ((((i + 1) as f64) * per).ceil() as usize).clamp(start + 1, samples.len());
        let slice = &samples[start..end];
        // The loudest sample in the bucket. NOT a sum, NOT a mean: this
        // number must not change when the bucket gets longer.
        let peak = slice.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        // Energy per sample, which is a mean by definition and is scale-free
        // in the bucket length for the same reason.
        let energy: f64 = slice.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        let rms = (energy / slice.len() as f64).sqrt() as f32;
        column.peak = peak;
        column.rms = rms;
        track_peak = track_peak.max(peak);
    }
    if track_peak <= 0.0 {
        return None;
    }
    // Per-track normalisation, the only division in the whole measurement:
    // a quiet field recording reads as well as a loud master, and neither
    // reads differently for being long.
    for column in &mut columns {
        column.peak = (column.peak / track_peak).clamp(0.0, 1.0);
        column.rms = (column.rms / track_peak).clamp(0.0, 1.0);
    }
    smooth_outline(&mut columns);
    Some(columns)
}

/// A 1-2-1 pass over the sub-column envelope. At `SUPERSAMPLE`× the output
/// width this softens the OUTLINE — the jag between adjacent sub-columns —
/// without touching anything a viewer resolves at output width.
fn smooth_outline(columns: &mut [Column]) {
    if columns.len() < 3 {
        return;
    }
    let source: Vec<Column> = columns.to_vec();
    for i in 1..columns.len() - 1 {
        let (a, b, c) = (source[i - 1], source[i], source[i + 1]);
        columns[i].peak = (a.peak + 2.0 * b.peak + c.peak) * 0.25;
        columns[i].rms = (a.rms + 2.0 * b.rms + c.rms) * 0.25;
    }
}

/// Resolve sub-columns into pixels by coverage: each output pixel takes the
/// fraction of itself the envelope actually covers, averaged over the
/// sub-columns that fall in it.
fn rasterise(columns: &[Column], w: usize, h: usize, palette: WavePalette) -> Vec<u8> {
    let mut rgba = vec![0u8; w * h * 4];
    let mid = h as f32 * 0.5;
    // One pixel of margin, so a full-scale peak still shows its own edge
    // rather than being clipped by the picture border.
    let half = (mid - 1.0).max(1.0);
    for x in 0..w {
        let subs = &columns[x * SUPERSAMPLE..((x + 1) * SUPERSAMPLE).min(columns.len())];
        if subs.is_empty() {
            continue;
        }
        for y in 0..h {
            // Distance from the zero line to this pixel's centre, in pixels.
            let d = ((y as f32 + 0.5) - mid).abs();
            let mut peak_cov = 0.0f32;
            let mut rms_cov = 0.0f32;
            for column in subs {
                // Coverage of a one-pixel-tall row by a band of half-height
                // `v * half`: full inside, zero outside, linear across the
                // one pixel at the edge.
                peak_cov += (column.peak * half - d + 0.5).clamp(0.0, 1.0);
                rms_cov += (column.rms * half - d + 0.5).clamp(0.0, 1.0);
            }
            let n = subs.len() as f32;
            let peak_cov = peak_cov / n;
            let rms_cov = rms_cov / n;
            // The zero line shows through where nothing is drawn over it.
            let line = (1.0 - (d - 0.0).min(1.0)).max(0.0);
            let mut px = mix(palette.background, palette.centre, line);
            px = mix(px, palette.peak, peak_cov * palette.peak_alpha);
            px = mix(px, palette.rms, rms_cov);
            let o = (y * w + x) * 4;
            rgba[o] = px[0];
            rgba[o + 1] = px[1];
            rgba[o + 2] = px[2];
            rgba[o + 3] = 255;
        }
    }
    rgba
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let mut out = [0u8; 3];
    for c in 0..3 {
        out[c] = (a[c] as f32 + (b[c] as f32 - a[c] as f32) * t).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn tone(freq: f32, secs: f32, rate: u32, amp: f32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n).map(|i| (2.0 * PI * freq * i as f32 / rate as f32).sin() * amp).collect()
    }

    fn filled_rows(rgba: &[u8], w: usize, h: usize, x: usize, palette: WavePalette) -> usize {
        (0..h)
            .filter(|y| {
                let o = (y * w + x) * 4;
                [rgba[o], rgba[o + 1], rgba[o + 2]] != palette.background
            })
            .count()
    }

    /// THE regression. The same music, at two lengths, draws the same
    /// picture: a column is the loudest sample in its bucket, so putting
    /// forty times more samples in each bucket does not shrink it. Any
    /// division by bucket count or duration fails here.
    #[test]
    fn a_long_track_is_not_a_flat_line() {
        let rate = 8_000;
        let palette = WavePalette::default();
        let (w, h) = (64, 64);
        let height_of = |secs: f32| {
            let rgba = wave_rgba(&tone(220.0, secs, rate, 0.8), w, h, palette).expect("strip");
            filled_rows(&rgba, w, h, w / 2, palette)
        };
        let short = height_of(2.0);
        let long = height_of(120.0);
        assert!(short > h / 2, "a full-scale tone fills most of the strip: {short}/{h}");
        assert!(
            (short as i32 - long as i32).abs() <= 2,
            "sixty times the samples must draw the same height: {short} vs {long}"
        );
    }

    /// And loud is loud: the strip is normalised to the track's own peak, so
    /// a quiet recording reads the same as a loud one, while a quiet PASSAGE
    /// inside a loud track reads as quiet.
    #[test]
    fn normalisation_is_per_track_not_per_column() {
        let rate = 8_000;
        let palette = WavePalette::default();
        let (w, h) = (64, 64);
        let loud = wave_rgba(&tone(220.0, 4.0, rate, 0.9), w, h, palette).unwrap();
        let quiet = wave_rgba(&tone(220.0, 4.0, rate, 0.02), w, h, palette).unwrap();
        assert_eq!(
            filled_rows(&loud, w, h, w / 2, palette),
            filled_rows(&quiet, w, h, w / 2, palette),
            "a quiet track draws the same picture"
        );

        // A track that is loud for its first half and quiet for its second
        // draws a tall left and a short right.
        let mut mixed = tone(220.0, 4.0, rate, 0.9);
        let half = mixed.len() / 2;
        for s in &mut mixed[half..] {
            *s *= 0.1;
        }
        let rgba = wave_rgba(&mixed, w, h, palette).unwrap();
        let left = filled_rows(&rgba, w, h, w / 4, palette);
        let right = filled_rows(&rgba, w, h, 3 * w / 4, palette);
        assert!(right * 2 < left, "the quiet half is visibly shorter: {left} vs {right}");
    }

    /// The envelope is antialiased, not stepped: the edge of the band is a
    /// partial colour, which is the whole difference between a drawn shape
    /// and a bar chart.
    #[test]
    fn the_outline_is_antialiased() {
        let rate = 8_000;
        let palette = WavePalette::default();
        let (w, h) = (32, 64);
        // A slow amplitude ramp, so the envelope crosses pixel boundaries at
        // a shallow angle and every edge pixel is a partial one.
        let n = rate as usize * 2;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / n as f32;
                (2.0 * PI * 200.0 * i as f32 / rate as f32).sin() * t
            })
            .collect();
        let rgba = wave_rgba(&samples, w, h, palette).unwrap();
        // Somewhere in the picture there is a pixel that is neither the
        // background nor a fully-covered peak colour.
        let peak_solid = mix(palette.background, palette.peak, palette.peak_alpha);
        let partial = (0..w * h)
            .map(|i| [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]])
            .filter(|px| {
                *px != palette.background
                    && *px != peak_solid
                    && *px != palette.rms
                    && *px != palette.centre
            })
            .count();
        assert!(partial > w, "a coverage-drawn edge has partial pixels: {partial}");
    }

    /// Peak outside, RMS inside: the translucent envelope is always at least
    /// as tall as the solid body, because the loudest sample in a bucket is
    /// never quieter than the bucket's energy.
    #[test]
    fn the_peak_envelope_contains_the_rms_body() {
        let rate = 8_000;
        // Sparse clicks: a big peak, very little energy — the case where the
        // two differ most.
        let mut samples = vec![0.0f32; rate as usize * 4];
        for i in (0..samples.len()).step_by(400) {
            samples[i] = 1.0;
        }
        let columns = measure(&samples, 64).expect("columns");
        for c in &columns {
            assert!(c.rms <= c.peak + 1e-6, "rms {} above peak {}", c.rms, c.peak);
        }
        assert!(
            columns.iter().any(|c| c.rms < c.peak * 0.5),
            "clicks are all peak and no body"
        );
    }

    /// Silence has no picture, and neither does nothing at all: a caller
    /// must be able to tell "no strip" from "a strip of a flat line".
    #[test]
    fn silence_and_emptiness_have_no_strip() {
        let palette = WavePalette::default();
        assert!(wave_rgba(&[], 32, 32, palette).is_none());
        assert!(wave_rgba(&[0.0; 4096], 32, 32, palette).is_none());
        assert!(wave_rgba(&[0.5; 4096], 0, 32, palette).is_none());
    }
}
