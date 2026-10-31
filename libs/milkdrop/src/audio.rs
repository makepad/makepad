//! What a MilkDrop frame knows about the music: the recent waveform of each
//! channel, its spectrum, and the three band levels the equations read.
//!
//! The host pushes played samples (any rate, mono or stereo) as they come;
//! [`AudioAnalyzer::frame`] turns the newest window into an [`AudioFrame`].
//! Band levels follow MilkDrop's convention: `bass`, `mid`, `treb` are the
//! band's immediate energy relative to its long-term average, so they sit
//! near 1 for music at any loudness and jump on hits; the `_att` values are
//! the same ratio of a smoothed energy. All rates are per second, so the
//! values do not depend on the frame rate.

use makepad_audio_picture::fft::Fft;

/// Waveform samples per channel handed to waves and per-point code.
pub const WAVE_SAMPLES: usize = 576;
/// Spectrum bins per channel.
pub const SPECTRUM_BINS: usize = 512;
const FFT_SIZE: usize = SPECTRUM_BINS * 2;
/// Samples kept per channel (a little more than one FFT window).
const RING: usize = 2048;

/// Below this mean energy per bin a band counts as silent. `Fft::magnitudes`
/// reads 1.0 for a full-scale sine, so this is -120 dBFS: far under any
/// quiet passage, above the transform's rounding noise on digital silence.
const SILENT_ENERGY: f32 = 1e-12;

/// Band edges in Hz: bass, mid, treble.
const BANDS: [(f32, f32); 3] = [(20.0, 250.0), (250.0, 2000.0), (2000.0, 11000.0)];

#[derive(Clone, Debug)]
pub struct AudioFrame {
    /// The newest waveform, oldest first, per channel (left, right), in
    /// -1..1.
    pub wave: [Vec<f32>; 2],
    /// Magnitude spectrum per channel, bin 0 = DC.
    pub spectrum: [Vec<f32>; 2],
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    pub vol: f32,
    pub bass_att: f32,
    pub mid_att: f32,
    pub treb_att: f32,
    pub vol_att: f32,
}

impl Default for AudioFrame {
    fn default() -> Self {
        AudioFrame {
            wave: [vec![0.0; WAVE_SAMPLES], vec![0.0; WAVE_SAMPLES]],
            spectrum: [vec![0.0; SPECTRUM_BINS], vec![0.0; SPECTRUM_BINS]],
            bass: 1.0,
            mid: 1.0,
            treb: 1.0,
            vol: 1.0,
            bass_att: 1.0,
            mid_att: 1.0,
            treb_att: 1.0,
            vol_att: 1.0,
        }
    }
}

pub struct AudioAnalyzer {
    rate: f32,
    ring: [Vec<f32>; 2],
    /// Next write position in the rings.
    head: usize,
    fft: Fft,
    window: Vec<f32>,
    /// Smoothed energy per band (for `_att`) and long-term average.
    att: [f32; 3],
    long: [f32; 3],
    primed: bool,
}

impl AudioAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
        AudioAnalyzer {
            rate: sample_rate.max(1000.0),
            ring: [vec![0.0; RING], vec![0.0; RING]],
            head: 0,
            fft: Fft::new(FFT_SIZE),
            window: vec![0.0; FFT_SIZE],
            att: [0.0; 3],
            long: [0.0; 3],
            primed: false,
        }
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        self.rate = rate.max(1000.0);
    }

    /// Push interleaved samples with `channels` per frame (1 = mono, which
    /// feeds both sides; more than 2 keeps the first two).
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        for frame in samples.chunks_exact(channels) {
            let l = frame[0];
            let r = if channels > 1 { frame[1] } else { l };
            self.ring[0][self.head] = l;
            self.ring[1][self.head] = r;
            self.head = (self.head + 1) % RING;
        }
    }

    /// The newest `n` samples of `channel`, oldest first, into `out`.
    fn newest(&self, channel: usize, out: &mut [f32]) {
        let n = out.len();
        let start = (self.head + RING - n) % RING;
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.ring[channel][(start + i) % RING];
        }
    }

    /// Analyse the newest window, advancing the smoothed levels by `dt`
    /// seconds.
    pub fn frame(&mut self, dt: f32, out: &mut AudioFrame) {
        let dt = dt.clamp(0.0, 0.25);
        for ch in 0..2 {
            let mut wave = std::mem::take(&mut out.wave[ch]);
            wave.resize(WAVE_SAMPLES, 0.0);
            self.newest(ch, &mut wave);
            out.wave[ch] = wave;
            let mut window = std::mem::take(&mut self.window);
            self.newest(ch, &mut window);
            let mut spectrum = std::mem::take(&mut out.spectrum[ch]);
            spectrum.resize(SPECTRUM_BINS, 0.0);
            self.fft.magnitudes(&window, &mut spectrum);
            self.window = window;
            out.spectrum[ch] = spectrum;
        }
        // Band energies from both channels.
        let hz_per_bin = self.rate / FFT_SIZE as f32;
        let mut imm = [0.0f32; 3];
        for (b, (lo, hi)) in BANDS.iter().enumerate() {
            let a = ((lo / hz_per_bin) as usize).clamp(1, SPECTRUM_BINS - 1);
            let z = ((hi / hz_per_bin) as usize).clamp(a + 1, SPECTRUM_BINS);
            let mut sum = 0.0;
            for i in a..z {
                sum += out.spectrum[0][i] * out.spectrum[0][i] + out.spectrum[1][i] * out.spectrum[1][i];
            }
            imm[b] = sum / (z - a) as f32;
        }
        if !self.primed {
            self.att = imm;
            self.long = imm;
            self.primed = true;
        }
        // Per-second smoothing: the attenuated level follows in ~0.2 s, the
        // long-term average over ~6 s.
        let k_att = 1.0 - (-dt / 0.2).exp();
        let k_long = 1.0 - (-dt / 6.0).exp();
        let mut rel = [1.0f32; 3];
        let mut rel_att = [1.0f32; 3];
        for b in 0..3 {
            self.att[b] += (imm[b] - self.att[b]) * k_att;
            self.long[b] += (imm[b] - self.long[b]) * k_long;
            if self.long[b] > SILENT_ENERGY {
                rel[b] = (imm[b] / self.long[b]).sqrt().min(8.0);
                rel_att[b] = (self.att[b] / self.long[b]).sqrt().min(8.0);
            } else {
                // Silence: no motion, the values rest at 1.
                rel[b] = 1.0;
                rel_att[b] = 1.0;
            }
        }
        out.bass = rel[0];
        out.mid = rel[1];
        out.treb = rel[2];
        out.vol = (rel[0] + rel[1] + rel[2]) / 3.0;
        out.bass_att = rel_att[0];
        out.mid_att = rel_att[1];
        out.treb_att = rel_att[2];
        out.vol_att = (rel_att[0] + rel_att[1] + rel_att[2]) / 3.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(hz: f32, rate: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * hz / rate * std::f32::consts::TAU).sin() * amp).collect()
    }

    #[test]
    fn a_bass_hit_lifts_bass_not_treble() {
        let mut a = AudioAnalyzer::new(44100.0);
        let mut f = AudioFrame::default();
        // Quiet mixed tone for a few seconds sets the averages.
        for _ in 0..180 {
            let mut s = tone(80.0, 44100.0, 735, 0.1);
            for (i, x) in tone(5000.0, 44100.0, 735, 0.1).iter().enumerate() {
                s[i] += x;
            }
            a.push_interleaved(&s, 1);
            a.frame(1.0 / 60.0, &mut f);
        }
        assert!((f.bass - 1.0).abs() < 0.3 && (f.treb - 1.0).abs() < 0.3, "{} {}", f.bass, f.treb);
        // A loud bass frame.
        let mut s = tone(80.0, 44100.0, 1024, 0.9);
        for (i, x) in tone(5000.0, 44100.0, 1024, 0.1).iter().enumerate() {
            s[i] += x;
        }
        a.push_interleaved(&s, 1);
        a.frame(1.0 / 60.0, &mut f);
        assert!(f.bass > 2.0, "bass {}", f.bass);
        assert!(f.treb < 1.5, "treb {}", f.treb);
        assert!(f.bass_att < f.bass, "att lags");
    }

    #[test]
    fn a_quiet_passage_still_moves() {
        // -66 dBFS bass with a hit 12 dB louder: quiet, not silent.
        let mut a = AudioAnalyzer::new(44100.0);
        let mut f = AudioFrame::default();
        for _ in 0..180 {
            a.push_interleaved(&tone(80.0, 44100.0, 735, 0.0005), 1);
            a.frame(1.0 / 60.0, &mut f);
        }
        assert!((f.bass - 1.0).abs() < 0.3, "bass {}", f.bass);
        a.push_interleaved(&tone(80.0, 44100.0, 1024, 0.002), 1);
        a.frame(1.0 / 60.0, &mut f);
        assert!(f.bass > 2.0, "quiet hit reads as silence: bass {}", f.bass);
    }

    #[test]
    fn silence_rests_at_one() {
        let mut a = AudioAnalyzer::new(48000.0);
        let mut f = AudioFrame::default();
        a.frame(0.016, &mut f);
        assert_eq!((f.bass, f.mid, f.treb), (1.0, 1.0, 1.0));
        assert_eq!(f.wave[0].len(), WAVE_SAMPLES);
        assert_eq!(f.spectrum[1].len(), SPECTRUM_BINS);
    }
}
