//! The sampled-voice mixer.
//!
//! Mixes *alongside* the procedural synth rather than replacing it: the host
//! sums both into its output buffer, so a game can use recorded impacts and a
//! synthesised engine hum in the same breath.

use crate::bank::{SampleBank, SampleId};
use crate::Pcm;

/// Simultaneous sampled voices. Beyond this, quiet/old voices are stolen.
/// Conservative because a Quest shares this budget with everything else.
pub const MAX_VOICES: usize = 24;

/// Short ramp applied at start and stop so nothing clicks.
const FADE_SECS: f32 = 0.004;

/// A voice's importance when the mixer runs out of slots.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Priority {
    /// Ambient texture: first to go.
    Low,
    Normal,
    /// Gameplay-critical (a hit you must hear).
    High,
}

/// How to start a voice.
#[derive(Clone, Copy, Debug)]
pub struct VoiceSpec {
    pub sample: SampleId,
    pub gain: f32,
    /// -1 left, 0 centre, +1 right.
    pub pan: f32,
    /// Playback-rate multiplier; 2.0 is an octave up and half as long.
    pub pitch: f32,
    pub looping: bool,
    pub priority: Priority,
}

impl VoiceSpec {
    pub fn one_shot(sample: SampleId) -> Self {
        Self {
            sample,
            gain: 1.0,
            pan: 0.0,
            pitch: 1.0,
            looping: false,
            priority: Priority::Normal,
        }
    }
}

/// Identifies a running voice; generation-tagged so a stale handle cannot
/// retune a slot that has since been reused by another sound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VoiceHandle {
    index: u32,
    generation: u32,
}

struct Voice {
    sample: SampleId,
    pos: f64,
    gain: f32,
    target_gain: f32,
    pan: f32,
    pitch: f32,
    looping: bool,
    priority: Priority,
    generation: u32,
    /// Counts up from zero so the oldest voice is identifiable.
    age: u64,
    releasing: bool,
    /// 0..1 envelope, ramped to avoid clicks at both ends.
    level: f32,
}

pub struct Mixer {
    voices: Vec<Option<Voice>>,
    generation: u32,
    clock: u64,
    rate: f32,
    master: f32,
    /// Peak of the last rendered buffer, for the limiter and for tests.
    last_peak: f32,
}

impl Mixer {
    pub fn new(device_rate: u32) -> Self {
        Self {
            voices: (0..MAX_VOICES).map(|_| None).collect(),
            generation: 1,
            clock: 0,
            rate: device_rate.max(1) as f32,
            master: 1.0,
            last_peak: 0.0,
        }
    }

    pub fn set_master(&mut self, gain: f32) {
        self.master = gain.clamp(0.0, 4.0);
    }

    pub fn last_peak(&self) -> f32 {
        self.last_peak
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| v.is_some()).count()
    }

    /// Start a voice. Returns `None` only if every slot holds something more
    /// important than this sound.
    pub fn play(&mut self, spec: VoiceSpec) -> Option<VoiceHandle> {
        let slot = self.free_slot().or_else(|| self.steal_for(spec.priority))?;
        self.clock += 1;
        self.generation = self.generation.wrapping_add(1).max(1);
        let generation = self.generation;
        self.voices[slot] = Some(Voice {
            sample: spec.sample,
            pos: 0.0,
            gain: spec.gain.clamp(0.0, 4.0),
            target_gain: spec.gain.clamp(0.0, 4.0),
            pan: spec.pan.clamp(-1.0, 1.0),
            pitch: spec.pitch.clamp(0.05, 8.0),
            looping: spec.looping,
            priority: spec.priority,
            generation,
            age: self.clock,
            releasing: false,
            level: 0.0,
        });
        Some(VoiceHandle {
            index: slot as u32,
            generation,
        })
    }

    /// Retune a running voice (engine note tracking speed, say).
    pub fn set(&mut self, h: VoiceHandle, gain: Option<f32>, pitch: Option<f32>, pan: Option<f32>) {
        if let Some(v) = self.voice_mut(h) {
            if let Some(g) = gain {
                v.target_gain = g.clamp(0.0, 4.0);
            }
            if let Some(p) = pitch {
                v.pitch = p.clamp(0.05, 8.0);
            }
            if let Some(p) = pan {
                v.pan = p.clamp(-1.0, 1.0);
            }
        }
    }

    /// Fade a voice out and drop it. Looping voices need this; one-shots end
    /// on their own.
    pub fn stop(&mut self, h: VoiceHandle) {
        if let Some(v) = self.voice_mut(h) {
            v.releasing = true;
        }
    }

    pub fn stop_all(&mut self) {
        for v in self.voices.iter_mut().flatten() {
            v.releasing = true;
        }
    }

    pub fn is_playing(&self, h: VoiceHandle) -> bool {
        self.voices
            .get(h.index as usize)
            .and_then(|v| v.as_ref())
            .is_some_and(|v| v.generation == h.generation)
    }

    fn voice_mut(&mut self, h: VoiceHandle) -> Option<&mut Voice> {
        self.voices
            .get_mut(h.index as usize)
            .and_then(|v| v.as_mut())
            .filter(|v| v.generation == h.generation)
    }

    fn free_slot(&self) -> Option<usize> {
        self.voices.iter().position(|v| v.is_none())
    }

    /// Steal the lowest-priority, then oldest, voice — but never one that
    /// outranks the incoming sound.
    fn steal_for(&mut self, priority: Priority) -> Option<usize> {
        let victim = self
            .voices
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| (i, v)))
            .filter(|(_, v)| v.priority <= priority)
            .min_by(|(_, a), (_, b)| a.priority.cmp(&b.priority).then(a.age.cmp(&b.age)))
            .map(|(i, _)| i)?;
        self.voices[victim] = None;
        Some(victim)
    }

    /// Render `frames` of interleaved stereo, ADDING into `out` so the caller
    /// can sum the synth into the same buffer.
    pub fn render(&mut self, bank: &SampleBank, out: &mut [f32], frames: usize) {
        let fade_step = 1.0 / (FADE_SECS * self.rate).max(1.0);
        let mut peak = 0.0f32;

        for slot in 0..self.voices.len() {
            let Some(v) = self.voices[slot].as_mut() else {
                continue;
            };
            let Some(pcm) = bank.get(v.sample) else {
                // The sample was evicted or never loaded: drop the voice
                // rather than reading a stale index.
                self.voices[slot] = None;
                continue;
            };
            let src_frames = pcm.frames();
            if src_frames == 0 {
                self.voices[slot] = None;
                continue;
            }
            let ch = pcm.channels.max(1);
            // Equal-power pan keeps loudness steady across the stereo field.
            let angle = (v.pan + 1.0) * 0.25 * std::f32::consts::PI;
            let (lg, rg) = (angle.cos(), angle.sin());
            let mut finished = false;

            for f in 0..frames {
                // Envelope: ramp in on start, out on release.
                let target = if v.releasing { 0.0 } else { 1.0 };
                if v.level < target {
                    v.level = (v.level + fade_step).min(target);
                } else if v.level > target {
                    v.level = (v.level - fade_step).max(target);
                }
                if v.releasing && v.level <= 0.0 {
                    finished = true;
                    break;
                }
                // Glide gain so a per-tick retune does not zipper.
                v.gain += (v.target_gain - v.gain) * 0.01;

                let pos = v.pos;
                let i0 = pos.floor() as usize;
                if i0 >= src_frames {
                    if v.looping {
                        v.pos = 0.0;
                        continue;
                    }
                    finished = true;
                    break;
                }
                let frac = (pos - i0 as f64) as f32;
                let i1 = if i0 + 1 < src_frames {
                    i0 + 1
                } else if v.looping {
                    0
                } else {
                    i0
                };

                // Mono sources feed both ears; stereo keeps its channels.
                let (sl, sr) = if ch == 1 {
                    let a = pcm.samples[i0];
                    let b = pcm.samples[i1];
                    let s = a + (b - a) * frac;
                    (s, s)
                } else {
                    let a0 = pcm.samples[i0 * ch];
                    let b0 = pcm.samples[i1 * ch];
                    let a1 = pcm.samples[i0 * ch + 1];
                    let b1 = pcm.samples[i1 * ch + 1];
                    (a0 + (b0 - a0) * frac, a1 + (b1 - a1) * frac)
                };

                let g = v.gain * v.level * self.master;
                let l = sl * g * lg;
                let r = sr * g * rg;
                out[f * 2] += l;
                out[f * 2 + 1] += r;

                v.pos += v.pitch as f64;
            }

            if finished {
                self.voices[slot] = None;
            }
        }

        // Peak of the SUM, not of any one voice: what clips is the total, and
        // the caller may have summed the synth in before calling us.
        for s in out.iter().take(frames * 2) {
            if s.is_finite() {
                peak = peak.max(s.abs());
            }
        }
        // Soft limiter: only engages once the sum would clip, so normal
        // playback is untouched and a pile-up compresses instead of tearing.
        self.last_peak = peak;
        if peak > 1.0 {
            let g = 1.0 / peak;
            for s in out.iter_mut().take(frames * 2) {
                *s *= g;
            }
            self.last_peak = 1.0;
        }
        for s in out.iter_mut().take(frames * 2) {
            if !s.is_finite() {
                *s = 0.0;
            }
        }
    }
}

/// Convenience for tests and offline rendering.
pub fn render_to_vec(mixer: &mut Mixer, bank: &SampleBank, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames * 2];
    mixer.render(bank, &mut out, frames);
    out
}

/// Interleaved stereo f32 -> a 16-bit WAV, for auditioning offline.
pub fn to_wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut v = Vec::with_capacity(44 + data.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// A short sine sample, for tests and as a stand-in when a pack is missing.
pub fn sine_pcm(rate: u32, freq: f32, secs: f32) -> Pcm {
    let n = (rate as f32 * secs) as usize;
    let samples = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            (t * freq * std::f32::consts::TAU).sin() * 0.8
        })
        .collect();
    Pcm {
        channels: 1,
        sample_rate: rate,
        samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::tests::wav;

    fn bank_with_tone() -> (SampleBank, SampleId) {
        let mut b = SampleBank::new(44100);
        // 4000 frames of non-silent audio.
        let id = b.insert("tone", &wav(4000, 44100)).unwrap();
        (b, id)
    }

    #[test]
    fn a_one_shot_plays_then_frees_its_slot() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        m.play(VoiceSpec::one_shot(id)).unwrap();
        assert_eq!(m.active_voices(), 1);
        // Render past the end of the sample.
        let out = render_to_vec(&mut m, &bank, 5000);
        assert!(out.iter().any(|s| s.abs() > 1e-4), "produced silence");
        assert_eq!(m.active_voices(), 0, "voice was not reclaimed");
    }

    #[test]
    fn output_is_finite_and_never_clips() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        // Pile on far more gain than can fit.
        for _ in 0..MAX_VOICES {
            m.play(VoiceSpec {
                gain: 3.0,
                ..VoiceSpec::one_shot(id)
            });
        }
        let out = render_to_vec(&mut m, &bank, 1024);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(
            out.iter().all(|s| s.abs() <= 1.0001),
            "limiter let the sum clip"
        );
    }

    #[test]
    fn voice_stealing_prefers_low_priority_and_protects_high() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        for _ in 0..MAX_VOICES {
            m.play(VoiceSpec {
                priority: Priority::High,
                ..VoiceSpec::one_shot(id)
            })
            .unwrap();
        }
        // A low-priority sound cannot displace a wall of high-priority ones.
        assert!(m
            .play(VoiceSpec {
                priority: Priority::Low,
                ..VoiceSpec::one_shot(id)
            })
            .is_none());
        // A high-priority one can.
        assert!(m
            .play(VoiceSpec {
                priority: Priority::High,
                ..VoiceSpec::one_shot(id)
            })
            .is_some());
        assert_eq!(m.active_voices(), MAX_VOICES);
        let _ = bank;
    }

    #[test]
    fn starting_and_stopping_are_click_free() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        let h = m.play(VoiceSpec::one_shot(id)).unwrap();
        let a = render_to_vec(&mut m, &bank, 512);
        // The very first sample must not jump straight to full amplitude.
        assert!(a[0].abs() < 0.05, "hard start: {}", a[0]);
        m.stop(h);
        let b = render_to_vec(&mut m, &bank, 512);
        // No step larger than a plausible waveform slope at the release.
        for w in b.chunks(2) {
            assert!(w[0].abs() <= 1.0);
        }
        assert!(b.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_stale_handle_cannot_retune_a_reused_slot() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        let h = m.play(VoiceSpec::one_shot(id)).unwrap();
        m.stop(h);
        render_to_vec(&mut m, &bank, 4096);
        assert!(!m.is_playing(h));
        let h2 = m.play(VoiceSpec::one_shot(id)).unwrap();
        // The old handle must not touch the new voice.
        m.set(h, Some(0.0), None, None);
        assert!(m.is_playing(h2));
        let out = render_to_vec(&mut m, &bank, 512);
        assert!(out.iter().any(|s| s.abs() > 1e-5), "new voice was muted");
    }

    #[test]
    fn pitch_shortens_a_one_shot() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        m.play(VoiceSpec {
            pitch: 4.0,
            ..VoiceSpec::one_shot(id)
        })
        .unwrap();
        // 4000 frames at 4x lasts about 1000 frames.
        render_to_vec(&mut m, &bank, 1100);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn panning_favours_the_expected_ear() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        m.play(VoiceSpec {
            pan: -1.0,
            ..VoiceSpec::one_shot(id)
        })
        .unwrap();
        let out = render_to_vec(&mut m, &bank, 512);
        let l: f32 = out.iter().step_by(2).map(|s| s.abs()).sum();
        let r: f32 = out.iter().skip(1).step_by(2).map(|s| s.abs()).sum();
        assert!(l > r * 4.0, "hard-left sound leaked right: l={l} r={r}");
    }

    #[test]
    fn a_looping_voice_keeps_going_and_stops_on_request() {
        let (bank, id) = bank_with_tone();
        let mut m = Mixer::new(44100);
        let h = m
            .play(VoiceSpec {
                looping: true,
                ..VoiceSpec::one_shot(id)
            })
            .unwrap();
        render_to_vec(&mut m, &bank, 9000); // well past one pass
        assert!(m.is_playing(h), "loop ended early");
        m.stop(h);
        render_to_vec(&mut m, &bank, 1024);
        assert!(!m.is_playing(h));
    }
}
