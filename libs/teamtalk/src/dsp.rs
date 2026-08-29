//! Small pure DSP pieces used on the capture path: a peak limiter, a voice
//! gate with hangover, click-free edge fades, and channel folding. All of it
//! is allocation-free and runs per frame on the audio thread.

use crate::wire::INTERNAL_RATE;

/// Root-mean-square level of a block.
pub fn rms(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt()
}

/// Linear fade from 0 to 1 over the first `n` samples.
pub fn fade_in(buf: &mut [f32], n: usize) {
    let n = n.min(buf.len()).max(1);
    for (i, v) in buf[..n].iter_mut().enumerate() {
        *v *= i as f32 / n as f32;
    }
}

/// Linear fade from 1 to 0 over the last `n` samples.
pub fn fade_out_tail(buf: &mut [f32], n: usize) {
    let len = buf.len();
    let n = n.min(len).max(1);
    for i in 0..n {
        buf[len - n + i] *= 1.0 - (i + 1) as f32 / n as f32;
    }
}

/// Fold a planar (channel-major, as `makepad_platform::audio::AudioBuffer`)
/// block to mono by averaging. `data.len() == frames * channels`.
pub fn fold_planar_to_mono(frames: usize, channels: usize, data: &[f32], out: &mut [f32]) {
    let channels = channels.max(1);
    let scale = 1.0 / channels as f32;
    for f in 0..frames.min(out.len()) {
        let mut acc = 0.0;
        for c in 0..channels {
            acc += data[c * frames + f];
        }
        out[f] = acc * scale;
    }
}

/// Fold an interleaved block to mono by averaging.
pub fn fold_interleaved_to_mono(channels: usize, data: &[f32], out: &mut [f32]) {
    let channels = channels.max(1);
    let scale = 1.0 / channels as f32;
    let frames = data.len() / channels;
    for f in 0..frames.min(out.len()) {
        let mut acc = 0.0;
        for c in 0..channels {
            acc += data[f * channels + c];
        }
        out[f] = acc * scale;
    }
}

/// A peak limiter with a fast attack and a slow release: keeps the wire
/// signal under `ceiling` without the pumping of a hard duck.
#[derive(Clone, Debug)]
pub struct Limiter {
    env: f32,
    ceiling: f32,
    attack: f32,
    release: f32,
}

impl Limiter {
    /// `attack_ms`/`release_ms` are the envelope time constants at
    /// [`INTERNAL_RATE`].
    pub fn new(ceiling: f32, attack_ms: f32, release_ms: f32) -> Self {
        let coef = |ms: f32| 1.0 - (-1.0 / (ms * 1e-3 * INTERNAL_RATE as f32)).exp();
        Self {
            env: 0.0,
            ceiling,
            attack: coef(attack_ms),
            release: coef(release_ms),
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for v in buf.iter_mut() {
            let a = v.abs();
            let coef = if a > self.env { self.attack } else { self.release };
            self.env += (a - self.env) * coef;
            if self.env > self.ceiling {
                *v *= self.ceiling / self.env;
            }
        }
    }
}

/// What the [`Gate`] decided for a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateState {
    /// Below threshold and past the hangover: send a silence packet.
    Silent,
    /// The first audible frame after silence: send it, with a short fade-in.
    Opened,
    /// Audible, or inside the hangover: send it.
    Open,
    /// The last frame before silence: send it with a short fade-out.
    Closing,
}

/// A frame-level voice gate. It never delays audio (no lookahead, so no
/// added latency) and never cuts the frame that contains the onset: the
/// frame the speech starts in is sent whole. A hangover keeps the gate open
/// across the pauses inside a sentence so a syllable is never chopped.
#[derive(Clone, Debug)]
pub struct Gate {
    threshold_rms: f32,
    hangover_frames: u32,
    remaining: u32,
    open: bool,
}

impl Gate {
    pub fn new(threshold_rms: f32, hangover_frames: u32) -> Self {
        Self {
            threshold_rms,
            hangover_frames,
            remaining: 0,
            open: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn process(&mut self, frame: &[f32]) -> GateState {
        let active = rms(frame) > self.threshold_rms;
        if active {
            self.remaining = self.hangover_frames;
            if self.open {
                GateState::Open
            } else {
                self.open = true;
                GateState::Opened
            }
        } else if self.open {
            if self.remaining > 0 {
                self.remaining -= 1;
                GateState::Open
            } else {
                self.open = false;
                GateState::Closing
            }
        } else {
            GateState::Silent
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_sends_the_onset_frame_whole_and_holds_through_pauses() {
        let mut gate = Gate::new(0.01, 3);
        let quiet = [0.0f32; 240];
        let mut loud = [0.0f32; 240];
        // Speech starts mid-frame.
        for v in &mut loud[120..] {
            *v = 0.3;
        }
        assert_eq!(gate.process(&quiet), GateState::Silent);
        assert_eq!(gate.process(&loud), GateState::Opened);
        assert_eq!(gate.process(&loud), GateState::Open);
        // A pause shorter than the hangover keeps the gate open.
        assert_eq!(gate.process(&quiet), GateState::Open);
        assert_eq!(gate.process(&quiet), GateState::Open);
        assert_eq!(gate.process(&loud), GateState::Open);
        // Past the hangover it closes with one fade-out frame, then silence.
        assert_eq!(gate.process(&quiet), GateState::Open);
        assert_eq!(gate.process(&quiet), GateState::Open);
        assert_eq!(gate.process(&quiet), GateState::Open);
        assert_eq!(gate.process(&quiet), GateState::Closing);
        assert_eq!(gate.process(&quiet), GateState::Silent);
    }

    #[test]
    fn fades_only_touch_the_edges() {
        let mut buf = [1.0f32; 240];
        fade_in(&mut buf, 48);
        assert_eq!(buf[0], 0.0);
        assert!(buf[47] < 1.0 && buf[47] > 0.9);
        assert!(buf[48..].iter().all(|&v| v == 1.0));
        let mut buf = [1.0f32; 240];
        fade_out_tail(&mut buf, 48);
        assert!(buf[..192].iter().all(|&v| v == 1.0));
        assert_eq!(buf[239], 0.0);
    }

    #[test]
    fn limiter_holds_the_ceiling_and_leaves_quiet_audio_alone() {
        let mut lim = Limiter::new(0.9, 0.5, 50.0);
        let mut quiet: Vec<f32> = (0..4800).map(|i| 0.3 * ((i as f32) * 0.05).sin()).collect();
        let before = quiet.clone();
        lim.process(&mut quiet);
        assert_eq!(quiet, before);
        let mut loud: Vec<f32> = (0..4800).map(|i| 1.5 * ((i as f32) * 0.05).sin()).collect();
        lim.process(&mut loud);
        let peak = loud[480..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        // The envelope releases a little between peaks: allow ~5 % over.
        assert!(peak <= 0.95, "peak {peak}");
        assert!(peak > 0.8, "limiter should not squash: peak {peak}");
    }

    #[test]
    fn folding_averages_channels() {
        let planar = [1.0, 2.0, 3.0, 0.0, 0.0, 1.0]; // 3 frames, 2 channels
        let mut out = [0.0f32; 3];
        fold_planar_to_mono(3, 2, &planar, &mut out);
        assert_eq!(out, [0.5, 1.0, 2.0]);
        let inter = [1.0, 0.0, 2.0, 0.0, 3.0, 1.0];
        fold_interleaved_to_mono(2, &inter, &mut out);
        assert_eq!(out, [0.5, 1.0, 2.0]);
    }
}
