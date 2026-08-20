//! The one picture an audio asset publishes.
//!
//! A track gets ONE mandatory thumbnail, so the picture has to be both
//! things: the spectrogram, which says what the piece IS, and a waveform
//! strip, which says where the loud bits are and reads instantly as "this is
//! sound" to anyone glancing at a grid.
//!
//! So: the spectrogram takes the picture, and a thin strip runs along the
//! bottom edge. FFT dominant on purpose — a reader that knows nothing about
//! regions and simply draws the whole image still sees a spectrogram with a
//! wave along its edge, which is a sensible picture rather than a puzzle.
//! The regions are then DECLARED on the manifest, so a preview that wants
//! only the wave, or only the spectrogram, cuts the one it wants instead of
//! measuring the picture and guessing where the boundary is.

use crate::spectrogram::spectrogram_rgba;
use crate::wave::{wave_rgba, WavePalette};

/// How much of the picture's height the wave strip takes. An eighth: enough
/// that the shape of the track is legible at a glance, little enough that
/// the spectrogram is plainly the picture.
pub const WAVE_STRIP_FRACTION: f32 = 0.125;

/// Where each part of a composite ended up, in pixels, top-left anchored.
/// The caller stamps these onto the thumbnail as declared views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompositeRegions {
    /// `(x, y, w, h)` of the spectrogram.
    pub fft: (u32, u32, u32, u32),
    /// `(x, y, w, h)` of the wave strip along the bottom edge.
    pub wave: (u32, u32, u32, u32),
}

/// Render the composite: spectrogram above, wave strip along the bottom.
///
/// `None` when the signal has no picture in it at all — too short to
/// transform, or digital silence. A black rectangle would be a lie about a
/// thumbnail having been rendered, and the caller has an honest fallback.
pub fn composite_rgba(
    samples: &[f32],
    sample_rate: u32,
    w: usize,
    h: usize,
) -> Option<(Vec<u8>, CompositeRegions)> {
    if w == 0 || h == 0 {
        return None;
    }
    // At least one row of strip, and never so much that the spectrogram
    // loses its own picture.
    let strip = ((h as f32 * WAVE_STRIP_FRACTION).round() as usize).clamp(1, h / 2);
    let top = h - strip;
    let fft = spectrogram_rgba(samples, sample_rate, w, top)?;
    let mut rgba = vec![0u8; w * h * 4];
    rgba[..fft.len()].copy_from_slice(&fft);

    // The strip is drawn against the ramp's own floor colour, so the two
    // halves read as one picture. A track with a spectrogram always has a
    // strip too, but if the wave measurement refuses (it cannot, given the
    // spectrogram just succeeded) the floor colour is what stays.
    let palette = WavePalette::under_spectrogram();
    if let Some(wave) = wave_rgba(samples, w, strip, palette) {
        rgba[top * w * 4..].copy_from_slice(&wave);
    } else {
        for px in rgba[top * w * 4..].chunks_exact_mut(4) {
            px[..3].copy_from_slice(&palette.background);
            px[3] = 255;
        }
    }
    Some((
        rgba,
        CompositeRegions {
            fft: (0, 0, w as u32, top as u32),
            wave: (0, top as u32, w as u32, strip as u32),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn music(secs: f32, rate: u32) -> Vec<f32> {
        let n = (secs * rate as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (2.0 * PI * 220.0 * t).sin() * 0.6 + (2.0 * PI * 3_000.0 * t).sin() * 0.2
            })
            .collect()
    }

    /// The declared regions are the picture: they tile it exactly, with no
    /// gap and no overlap, and the strip is the thin one along the bottom.
    #[test]
    fn the_regions_tile_the_picture_exactly() {
        let rate = 44_100;
        let (w, h) = (256, 128);
        let (rgba, regions) = composite_rgba(&music(4.0, rate), rate, w, h).expect("composite");
        assert_eq!(rgba.len(), w * h * 4);
        let (fx, fy, fw, fh) = regions.fft;
        let (vx, vy, vw, vh) = regions.wave;
        assert_eq!((fx, fy, fw), (0, 0, w as u32));
        assert_eq!((vx, vw), (0, w as u32));
        assert_eq!(fy + fh, vy, "the strip starts where the spectrogram ends");
        assert_eq!(vy + vh, h as u32, "and runs to the bottom edge");
        assert_eq!(vh, 16, "an eighth of 128");
        assert!(fh > vh * 4, "the spectrogram is plainly the picture");
    }

    /// Both halves are actually drawn: a reader that ignores the regions and
    /// looks at the whole image sees a spectrogram with a wave along its
    /// edge, not a spectrogram over a black bar.
    #[test]
    fn both_halves_carry_a_picture() {
        let rate = 44_100;
        let (w, h) = (256, 128);
        let (rgba, regions) = composite_rgba(&music(4.0, rate), rate, w, h).unwrap();
        let distinct = |y0: u32, y1: u32| {
            let mut seen = std::collections::HashSet::new();
            for y in y0..y1 {
                for x in 0..w as u32 {
                    let o = ((y as usize) * w + x as usize) * 4;
                    seen.insert([rgba[o], rgba[o + 1], rgba[o + 2]]);
                }
            }
            seen.len()
        };
        let (_, fy, _, fh) = regions.fft;
        let (_, vy, _, vh) = regions.wave;
        assert!(distinct(fy, fy + fh) > 16, "the spectrogram has a picture in it");
        assert!(distinct(vy, vy + vh) > 4, "and so does the strip");
        // Every pixel is opaque: this becomes a JPEG.
        assert!(rgba.chunks_exact(4).all(|px| px[3] == 255));
    }

    /// Nothing to show is `None`, all the way through the composite — the
    /// caller falls back to its own honest picture rather than publishing a
    /// black rectangle with two regions declared on it.
    #[test]
    fn silence_has_no_composite() {
        assert!(composite_rgba(&vec![0.0; 44_100], 44_100, 64, 64).is_none());
        assert!(composite_rgba(&[0.1; 64], 44_100, 64, 64).is_none(), "too short to transform");
        assert!(composite_rgba(&music(2.0, 44_100), 44_100, 0, 64).is_none());
    }
}
