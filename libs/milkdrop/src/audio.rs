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
//!
//! The same samples feed the beat tracker (`makepad_beat::BeatPulse`), so a
//! frame also carries the beat: what the equations read as `beat_pos`,
//! `bar_pos`, `beat_phase`, `bar_phase`, `tempo`, `beat_conf`,
//! `beat_pulse`, `bar_pulse` and `onset`, and the bar lines the automatic
//! preset change waits for ([`crate::AutoChange`]).

use makepad_audio_picture::fft::Fft;
use makepad_beat::{BeatFrame, BeatPulse};

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

/// Frames down-mixed for the beat tracker at a time (on the stack).
const MONO_BLOCK: usize = 512;

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
    /// The beat at this frame.
    pub beat: BeatFrame,
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
            beat: BeatFrame::default(),
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
    beat: BeatPulse,
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
            beat: BeatPulse::new(),
        }
    }

    pub fn set_sample_rate(&mut self, rate: f32) {
        self.rate = rate.max(1000.0);
    }

    /// Push interleaved samples with `channels` per frame (1 = mono, which
    /// feeds both sides; more than 2 keeps the first two).
    pub fn push_interleaved(&mut self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        let mut mono = [0.0f32; MONO_BLOCK];
        for block in samples.chunks(MONO_BLOCK * channels) {
            let mut n = 0;
            for frame in block.chunks_exact(channels) {
                let l = frame[0];
                let r = if channels > 1 { frame[1] } else { l };
                self.ring[0][self.head] = l;
                self.ring[1][self.head] = r;
                self.head = (self.head + 1) % RING;
                mono[n] = 0.5 * (l + r);
                n += 1;
            }
            self.beat.push(&mono[..n], self.rate as f64);
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
        out.beat = self.beat.frame(dt as f64);
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
    fn a_kick_track_reaches_the_equations_as_a_beat() {
        use crate::parse::Preset;
        use crate::state::{Engine, FrameInput, FrameParams};
        use std::sync::Arc;
        // A kick every half second (120 BPM) with an off-beat hat, played
        // a 60 Hz frame at a time.
        let rate = 48_000.0f32;
        let mut track = vec![0.0f32; (rate * 12.0) as usize];
        let mut seed = 0x1234_5678u32;
        for beat in 0..23 {
            let at = ((0.3 + beat as f32 * 0.5) * rate) as usize;
            for i in 0..(0.2 * rate) as usize {
                let t = i as f32 / rate;
                track[at + i] += 0.6 * (-20.0 * t).exp() * (std::f32::consts::TAU * (60.0 * t + 3.6 * (1.0 - (-25.0 * t).exp()))).sin();
            }
            for i in 0..(0.02 * rate) as usize {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                track[at + (0.25 * rate) as usize + i] += 0.1 * (-150.0 * i as f32 / rate).exp() * ((seed >> 8) as f32 / 8_388_608.0 - 1.0);
            }
        }
        let mut a = AudioAnalyzer::new(rate);
        let mut f = AudioFrame::default();
        for frame in track.chunks(800) {
            a.push_interleaved(frame, 1);
            a.frame(frame.len() as f32 / rate, &mut f);
        }
        assert!(f.beat.locked && (f.beat.tempo - 120.0).abs() < 2.0, "{:?}", f.beat);

        let preset = Arc::new(Preset::parse(b"per_frame_1=q1 = tempo; q2 = beat_conf; q3 = beat_pos; q4 = beat_phase + beat_pulse + bar_pulse + onset;\n"));
        let mut engine = Engine::new(preset, 1);
        let input = FrameInput { time: 12.0, fps: 60.0, frame: 0, progress: 0.0, audio: &f, width: 64.0, height: 64.0, mesh: (8, 8) };
        let mut params = FrameParams::default();
        engine.frame(&input, &mut params);
        let b = f.beat;
        assert_eq!(params.q[0], b.tempo);
        assert_eq!(params.q[1], b.confidence);
        assert_eq!(params.q[2], b.beat_pos as f32);
        assert_eq!(params.q[3], b.beat_phase + b.beat_pulse + b.bar_pulse + b.onset);
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
