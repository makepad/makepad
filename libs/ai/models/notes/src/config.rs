//! Fixed geometry of Spotify's published ICASSP 2022 `nmp` checkpoint.

pub const SAMPLE_RATE: usize = 22_050;
pub const FFT_HOP: usize = 256;
pub const AUDIO_WINDOW_SECONDS: usize = 2;
pub const AUDIO_N_SAMPLES: usize = SAMPLE_RATE * AUDIO_WINDOW_SECONDS - FFT_HOP; // 43,844
pub const WINDOW_FRAMES: usize = 172;
pub const OVERLAP_FRAMES: usize = 30;
pub const OVERLAP_SAMPLES: usize = OVERLAP_FRAMES * FFT_HOP;
pub const WINDOW_HOP_SAMPLES: usize = AUDIO_N_SAMPLES - OVERLAP_SAMPLES;
pub const OUTPUT_FRAMES_PER_WINDOW: usize = WINDOW_FRAMES - OVERLAP_FRAMES;

pub const NOTES: usize = 88;
pub const CONTOUR_BINS_PER_SEMITONE: usize = 3;
pub const CONTOUR_BINS: usize = NOTES * CONTOUR_BINS_PER_SEMITONE;
pub const CQT_BINS_PER_OCTAVE: usize = 12 * CONTOUR_BINS_PER_SEMITONE;
pub const CQT_BINS: usize = 309;
pub const CQT_KERNEL: usize = 256;
pub const CQT_OCTAVES: usize = 9;
pub const CQT_BLOCK_BINS: usize = CQT_BINS_PER_OCTAVE;
pub const BASE_FREQUENCY: f64 = 27.5;
pub const MIDI_OFFSET: i32 = 21;
pub const HARMONICS: [f64; 8] = [0.5, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

pub const ONSET_THRESHOLD: f32 = 0.5;
pub const FRAME_THRESHOLD: f32 = 0.3;
pub const MIN_NOTE_LEN: usize = 11;
pub const ENERGY_TOLERANCE: usize = 11;
pub const PITCH_BEND_TOLERANCE_BINS: isize = 25;

/// Model-frame timing. The network emits one frame per 256 input samples.
pub const FRAME_RATE: f64 = SAMPLE_RATE as f64 / FFT_HOP as f64;

pub fn frame_count(samples: usize) -> usize {
    usize::from(samples != 0) + samples / FFT_HOP
}

pub fn harmonic_shifts() -> [isize; HARMONICS.len()] {
    HARMONICS.map(|harmonic| {
        (12.0 * CONTOUR_BINS_PER_SEMITONE as f64 * harmonic.log2()).round() as isize
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_geometry() {
        assert_eq!(AUDIO_N_SAMPLES, 43_844);
        assert_eq!(frame_count(AUDIO_N_SAMPLES), WINDOW_FRAMES);
        assert_eq!(harmonic_shifts(), [-36, 0, 36, 57, 72, 84, 93, 101]);
    }
}
