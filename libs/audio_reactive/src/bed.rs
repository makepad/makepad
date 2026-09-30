//! THE MUSIC BED — a deterministic four-bar groove a tool plays into the
//! analyser ([`crate::AudioReactive`]) when no real music is there (a
//! thumbnail baker, a gallery, a test, a game's attract mode), so an
//! audio-reactive look shows what it DOES: the
//! kick lands on every beat of the 120 BPM clock the baker runs, a snare
//! on two and four, hats on the eighths, a bass line that changes root
//! every bar, a pad in the mids and an arpeggio on top — so every band,
//! every onset detector, the auto-gain and the drop detector get real
//! work. Without it, a document that draws from `audio_fft`, `audio_hit`
//! or `audio_norm` renders its idle floor: a thumbnail that reads as "does
//! not work".
//!
//! Every sample is a pure function of its index, so the bed can be asked
//! for any span in any chunking and the answer is bit-identical: the
//! baker's preroll and its captures, a gallery run and a sweep grab all
//! hear the same music at the same beat.

/// The bed's sample rate.
pub const BED_RATE: f32 = 48_000.0;
/// The bed's tempo: the baker's free-running clock (fx_thumbs.rs
/// `set_bpm(120.0)`), so the kick sits on the view's own beat.
pub const BED_BPM: f32 = 120.0;

/// The bed, rendered by absolute time.
#[derive(Default, Clone, Copy)]
pub struct AudioBed {
    /// Where the last `render_to` left off, in samples.
    cursor: i64,
}

impl AudioBed {
    /// Start (again) so the first render begins at `secs`.
    pub fn seek(&mut self, secs: f64) {
        self.cursor = (secs.max(0.0) * BED_RATE as f64).round() as i64;
    }

    /// Render from the cursor up to `secs` (mono, -1..1) into `out`, and
    /// move the cursor there. Nothing when `secs` is not ahead.
    pub fn render_to(&mut self, secs: f64, out: &mut Vec<f32>) {
        out.clear();
        let end = (secs.max(0.0) * BED_RATE as f64).round() as i64;
        if end <= self.cursor {
            return;
        }
        // Bounded work per call: a runaway clock never renders minutes.
        let start = self.cursor.max(end - BED_RATE as i64 * 8);
        out.reserve((end - start) as usize);
        for n in start..end {
            out.push(sample(n));
        }
        self.cursor = end;
    }
}

/// A stateless hash → 0..1 for noise (the same value for the same index).
fn noise(n: i64) -> f32 {
    let mut x = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    ((x >> 40) as f32) / ((1u64 << 24) as f32)
}

/// White noise centred on zero, then differenced so its energy sits in
/// the highs (a hat, a snare's rattle).
fn bright_noise(n: i64) -> f32 {
    (noise(n) - noise(n - 1)) * 1.4
}

/// One sample of the bed at index `n`.
fn sample(n: i64) -> f32 {
    use std::f32::consts::TAU;
    let t = n as f32 / BED_RATE;
    let beat_len = 60.0 / BED_BPM;
    let beat = t / beat_len;
    let beat_i = beat.floor();
    let tb = (beat - beat_i) * beat_len;
    let bar_step = (beat_i as i64).rem_euclid(4);
    let bar = (beat_i / 4.0).floor() as i64;
    let mut v = 0.0f32;

    // KICK on every beat: a sine that sweeps 160 → 48 Hz as it decays.
    {
        let env = (-tb * 14.0).exp();
        let phase = TAU * (48.0 * tb + 112.0 * (1.0 - (-tb * 35.0).exp()) / 35.0);
        v += phase.sin() * env * 0.9;
    }
    // SNARE on two and four: a 190 Hz body under a bright rattle.
    if bar_step == 1 || bar_step == 3 {
        let env = (-tb * 18.0).exp();
        v += ((TAU * 190.0 * tb).sin() * 0.5 + bright_noise(n) * 0.7) * env * 0.45;
    }
    // HATS on the eighths, the off-beat one open.
    {
        let eighth = beat * 2.0;
        let te = (eighth - eighth.floor()) * beat_len * 0.5;
        let open = (eighth.floor() as i64).rem_euclid(2) == 1;
        let env = (-te * if open { 22.0 } else { 70.0 }).exp();
        v += bright_noise(n) * env * if open { 0.16 } else { 0.22 };
    }
    // BASS: a root a bar (A1, C2, F1, G1), a 16th-step gate.
    {
        let root = [55.0f32, 65.41, 43.65, 49.0][bar.rem_euclid(4) as usize];
        let step16 = beat * 4.0;
        let s16 = (step16.floor() as i64).rem_euclid(16);
        let ts = (step16 - step16.floor()) * beat_len * 0.25;
        const GATE: [bool; 16] =
            [true, false, false, true, false, false, true, false, true, false, true, false, true, false, true, false];
        if GATE[s16 as usize] {
            let env = (1.0 - (-ts * 200.0).exp()) * (-ts * 9.0).exp();
            let p = TAU * root * t;
            v += (p.sin() + 0.35 * (2.0 * p).sin() + 0.15 * (3.0 * p).sin()) * env * 0.4;
        }
    }
    // PAD: a chord in the mids, breathing over four bars.
    {
        let breath = 0.6 + 0.4 * (TAU * t / (beat_len * 16.0)).sin();
        for f in [220.0f32, 261.63, 329.63, 440.0] {
            v += (TAU * f * t).sin() * 0.05 * breath;
        }
    }
    // ARPEGGIO on the eighths, a square-ish voice.
    {
        let eighth = beat * 2.0;
        let i = (eighth.floor() as i64).rem_euclid(4) as usize;
        let te = (eighth - eighth.floor()) * beat_len * 0.5;
        let f = [440.0f32, 523.25, 659.25, 880.0][i];
        let env = (-te * 8.0).exp();
        let p = TAU * f * t;
        v += (p.sin() + (3.0 * p).sin() / 3.0) * env * 0.12;
    }
    // Soft clip: never past full scale, never a hard edge.
    v / (1.0 + v.abs()) * 1.25
}

/// The bed played into an analyser: one of these per baker lane and one in
/// the gallery. `pump` renders the bed up to `secs` of BEAT TIME (the
/// view's beat position × a beat's length — the kick then sits on the
/// view's own beat whatever its tempo), feeds the bus and hands back the
/// binding to give the view. A first pump, or one that jumps back (a
/// capture restarting a document's clock), plays [`PRELUDE_SECS`] first so
/// the followers and the auto-gain are settled by the time anything is
/// captured.
pub struct BedPlayer {
    bed: AudioBed,
    pub bus: crate::AudioReactive,
    buf: Vec<f32>,
    last: Option<f64>,
}

/// Bed rendered ahead of a fresh start, so the analysis is warm.
pub const PRELUDE_SECS: f64 = 4.0;

impl Default for BedPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl BedPlayer {
    pub fn new() -> Self {
        BedPlayer { bed: AudioBed::default(), bus: crate::AudioReactive::new(), buf: Vec::new(), last: None }
    }

    /// Beat position → bed seconds.
    pub fn beat_secs(beat_pos: f64) -> f64 {
        beat_pos * 60.0 / BED_BPM as f64
    }

    /// Play the bed up to `secs` and republish the analysis.
    pub fn pump(&mut self, cx: &mut makepad_draw::Cx, secs: f64) -> Option<crate::AudioFrame> {
        let fresh = match self.last {
            None => true,
            Some(last) => secs + 0.05 < last,
        };
        if fresh {
            self.bed.seek((secs - PRELUDE_SECS).max(0.0));
        }
        self.bed.render_to(secs, &mut self.buf);
        if self.buf.is_empty() {
            self.bus.pump(cx, &[], BED_RATE);
        } else {
            // Bounded pushes: the bus keeps at most a few hops pending.
            let cut = self.buf.len().saturating_sub(4096);
            for chunk in self.buf[..cut].chunks(4096) {
                self.bus.push_samples(chunk, BED_RATE);
            }
            self.bus.pump(cx, &self.buf[cut..], BED_RATE);
        }
        self.last = Some(secs);
        self.bus.binding()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bed_is_a_pure_function_of_time() {
        let mut one = AudioBed::default();
        let mut whole = Vec::new();
        one.render_to(0.5, &mut whole);
        let mut two = AudioBed::default();
        let (mut a, mut b) = (Vec::new(), Vec::new());
        two.render_to(0.2, &mut a);
        two.render_to(0.5, &mut b);
        a.extend_from_slice(&b);
        assert_eq!(whole.len(), (0.5 * BED_RATE) as usize);
        assert_eq!(whole, a, "chunking changed the music");
        let mut again = AudioBed::default();
        again.seek(0.2);
        let mut c = Vec::new();
        again.render_to(0.5, &mut c);
        assert_eq!(c, b, "a seek does not land on the same samples");
    }

    #[test]
    fn the_bed_has_a_kick_on_the_beat_and_energy_in_every_band() {
        let mut bed = AudioBed::default();
        let mut s = Vec::new();
        bed.render_to(2.0, &mut s);
        assert!(s.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
        // The first 30 ms of each beat is louder than the 30 ms before it.
        let beat = (60.0 / BED_BPM * BED_RATE) as usize;
        let win = (0.03 * BED_RATE) as usize;
        for b in 1..4 {
            let at = b * beat;
            assert!(rms(&s[at..at + win]) > rms(&s[at - win..at]) * 1.5, "no kick at beat {b}");
        }
        // Lows: a crude 200 Hz low-pass (moving average over 240 samples).
        let lows: Vec<f32> = s.windows(240).map(|w| w.iter().sum::<f32>() / 240.0).collect();
        // Highs: the first difference.
        let highs: Vec<f32> = s.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(rms(&lows) > 0.05, "no bass: {}", rms(&lows));
        assert!(rms(&highs) > 0.02, "no highs: {}", rms(&highs));
        assert!(rms(&s) > 0.15, "too quiet: {}", rms(&s));
    }

    #[test]
    fn the_analyser_hears_the_bed() {
        let mut bus = crate::AudioReactive::new();
        let mut bed = AudioBed::default();
        let mut buf = Vec::new();
        bed.render_to(3.0, &mut buf);
        for chunk in buf.chunks(4096) {
            bus.push_samples(chunk, BED_RATE);
        }
        assert!(bus.has_audio(), "nothing analysed");
    }
}
