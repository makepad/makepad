//! Game sound effects: a tiny polyphonic synthesizer.
//!
//! Games ship no audio assets — the art style is procedural, so the sound is
//! too: a named bank of kid-game staples plus raw `beep`/`jingle`/`tone`
//! primitives, mixed additively into the app's audio output callback.
//!
//! Ported from `examples/gamemaker/src/synth.rs`, which this deliberately does
//! not share yet: the shared home for it is `libs/game/audio`, and creating
//! that crate means editing gamemaker (tape-parity-critical) — out of this
//! task's scope. The one behavioural addition here is **stereo**: a voice
//! carries a pan, so `game.sfx_at` can actually be heard to one side. Merging
//! the two copies is a mechanical follow-up, gated by the tape.

use makepad_widgets::makepad_platform::audio::AudioBuffer;
use std::sync::Mutex;

/// Percussive envelope attack, long enough to avoid clicks.
const ATTACK_SECS: f32 = 0.004;
/// Voice cap: oldest voice is dropped, a stuck script can't build a wall of sound.
const MAX_VOICES: usize = 24;
/// Sustained voice cap — engine hums, wind, sirens. Small on purpose.
const MAX_TONES: usize = 6;
/// Parameter smoothing rate (≈30ms to target) so per-tick retuning from
/// `game.tone_set` glides instead of zipper-stepping.
const TONE_SMOOTH_RATE: f32 = 33.0;
const TONE_ATTACK_RATE: f32 = 60.0;
const TONE_RELEASE_RATE: f32 = 18.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Wave {
    Sine,
    Square,
    Saw,
    Triangle,
    Noise,
}

impl Wave {
    pub fn parse(name: &str) -> Wave {
        match name {
            "sine" => Wave::Sine,
            "saw" => Wave::Saw,
            "triangle" | "tri" => Wave::Triangle,
            "noise" => Wave::Noise,
            _ => Wave::Square,
        }
    }
}

/// Per-channel gains for a pan in -1..1. Centre keeps full volume in both
/// channels (rather than equal-power, which would make every 2D sound
/// quieter than it is today just for having gained a pan field).
fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    (1.0 - pan.max(0.0), 1.0 + pan.min(0.0))
}

struct Voice {
    wave: Wave,
    freq_from: f32,
    freq_to: f32,
    len: f32,
    gain: f32,
    /// -1 left .. 0 centre .. +1 right. Positional sounds set this; the 2D
    /// verbs leave it at 0 and behave exactly as before.
    pan: f32,
    /// Seconds until the voice starts — how jingles sequence notes.
    delay: f32,
    t: f32,
    phase: f32,
    noise: u32,
}

/// A looping tone: no envelope end, retunable without retriggering. This is
/// the "car engine note" primitive one-shot beeps can't fake (60 envelope
/// restarts a second).
struct Tone {
    id: u64,
    wave: Wave,
    freq: f32,
    freq_target: f32,
    gain: f32,
    gain_target: f32,
    /// Attack/release level; a released tone fades out and is dropped.
    level: f32,
    releasing: bool,
    phase: f32,
    noise: u32,
}

pub struct Synth {
    voices: Vec<Voice>,
    tones: Vec<Tone>,
}

/// Shared with the audio callback. The script side only ever pushes voices;
/// the audio thread only ever advances and drops them.
static SYNTH: Mutex<Synth> = Mutex::new(Synth {
    voices: Vec::new(),
    tones: Vec::new(),
});

/// Start a sustained tone under a host-minted id (`Ctx::next_tone`), so the
/// script gets its handle back synchronously from a drained queue.
pub fn tone(id: u64, freq: f32, wave: Wave, gain: f32) {
    let Ok(mut synth) = SYNTH.lock() else { return };
    if synth.tones.len() >= MAX_TONES {
        synth.tones.remove(0);
    }
    synth.tones.push(Tone {
        id,
        wave,
        freq: freq.clamp(20.0, 8000.0),
        freq_target: freq.clamp(20.0, 8000.0),
        gain: 0.0,
        gain_target: gain.clamp(0.0, 1.0),
        level: 0.0,
        releasing: false,
        phase: 0.0,
        noise: 0x51ed_2705,
    });
}

/// Retune a running tone — smoothed, never retriggered.
pub fn tone_set(id: u64, freq: Option<f32>, gain: Option<f32>) {
    let Ok(mut synth) = SYNTH.lock() else { return };
    if let Some(tone) = synth.tones.iter_mut().find(|t| t.id == id) {
        if let Some(freq) = freq {
            tone.freq_target = freq.clamp(20.0, 8000.0);
        }
        if let Some(gain) = gain {
            tone.gain_target = gain.clamp(0.0, 1.0);
        }
    }
}

pub fn tone_stop(id: u64) {
    let Ok(mut synth) = SYNTH.lock() else { return };
    if let Some(tone) = synth.tones.iter_mut().find(|t| t.id == id) {
        tone.releasing = true;
    }
}

/// A rebuilt world must never inherit a stuck engine hum: the script queues
/// this on every eval/reset.
pub fn stop_all_tones() {
    let Ok(mut synth) = SYNTH.lock() else { return };
    for tone in synth.tones.iter_mut() {
        tone.releasing = true;
    }
}

/// One tone, optionally gliding from `freq` to `to` over its length.
pub fn beep(freq: f32, to: f32, secs: f32, wave: Wave, gain: f32, delay: f32) {
    beep_panned(freq, to, secs, wave, gain, delay, 0.0);
}

pub fn beep_panned(
    freq: f32,
    to: f32,
    secs: f32,
    wave: Wave,
    gain: f32,
    delay: f32,
    pan: f32,
) {
    let Ok(mut synth) = SYNTH.lock() else { return };
    if synth.voices.len() >= MAX_VOICES {
        synth.voices.remove(0);
    }
    synth.voices.push(Voice {
        wave,
        freq_from: freq.clamp(20.0, 8000.0),
        freq_to: to.clamp(20.0, 8000.0),
        len: secs.clamp(0.01, 3.0),
        gain: gain.clamp(0.0, 1.0),
        pan: pan.clamp(-1.0, 1.0),
        delay: delay.max(0.0),
        t: 0.0,
        phase: 0.0,
        noise: 0x2f6e2b1,
    });
}

/// Note names, e.g. "C4 E4 G4 C5" (sharps as "F#5"). Unknown tokens are rests,
/// so a slightly-wrong jingle still plays instead of erroring at a kid.
pub fn jingle(notes: &str, note_secs: f32, wave: Wave, gain: f32) {
    let step = note_secs.clamp(0.03, 1.0);
    for (index, token) in notes.split_whitespace().enumerate() {
        if let Some(freq) = note_freq(token) {
            beep(freq, freq, step * 0.9, wave, gain, index as f32 * step);
        }
    }
}

fn note_freq(token: &str) -> Option<f32> {
    let bytes = token.as_bytes();
    let semitone = match bytes.first()?.to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut index = 1;
    let mut sharp = 0;
    if bytes.get(index) == Some(&b'#') {
        sharp = 1;
        index += 1;
    }
    let octave: i32 = token.get(index..)?.parse().ok()?;
    let midi = (octave + 1) * 12 + semitone + sharp;
    Some(440.0 * 2f32.powf((midi as f32 - 69.0) / 12.0))
}

/// The kid-game staple bank, at a position. `gain_scale` is the distance
/// attenuation and `pan` the direction; a 2D `sfx` passes (1.0, 0.0) and gets
/// exactly the historical sound.
pub fn play_named_at(name: &str, pitch: f32, gain_scale: f32, pan: f32) -> bool {
    let p = pitch.clamp(0.25, 4.0);
    let g = gain_scale.clamp(0.0, 1.0);
    // Local helpers so the bank recipes stay readable and identical to the
    // gamemaker originals apart from the scale/pan pass-through.
    let b = |freq: f32, to: f32, secs: f32, wave: Wave, gain: f32, delay: f32| {
        beep_panned(freq, to, secs, wave, gain * g, delay, pan)
    };
    let j = |notes: &str, secs: f32, wave: Wave, gain: f32| {
        let step = secs.clamp(0.03, 1.0);
        for (index, token) in notes.split_whitespace().enumerate() {
            if let Some(freq) = note_freq(token) {
                beep_panned(
                    freq,
                    freq,
                    step * 0.9,
                    wave,
                    gain * g,
                    index as f32 * step,
                    pan,
                );
            }
        }
    };
    match name {
        "jump" => b(260.0 * p, 540.0 * p, 0.12, Wave::Square, 0.22, 0.0),
        "shoot" => b(880.0 * p, 180.0 * p, 0.09, Wave::Square, 0.20, 0.0),
        "zap" => {
            b(1200.0 * p, 90.0 * p, 0.18, Wave::Saw, 0.22, 0.0);
            b(600.0, 600.0, 0.10, Wave::Noise, 0.12, 0.0);
        }
        "grab" => b(320.0 * p, 180.0 * p, 0.12, Wave::Sine, 0.25, 0.0),
        "angry" => b(150.0 * p, 90.0 * p, 0.25, Wave::Square, 0.22, 0.0),
        "calm" => b(390.0 * p, 520.0 * p, 0.20, Wave::Sine, 0.20, 0.0),
        "rescue" => j("E5 G5", 0.09, Wave::Triangle, 0.22),
        "shove" => b(200.0, 200.0, 0.06, Wave::Noise, 0.30, 0.0),
        // Firework: a noise burst sweeping down into a long tail. Noise
        // because a shell is broadband — a tone reads as a laser, not a bang.
        "firework" => b(900.0 * p, 70.0 * p, 0.85, Wave::Noise, 0.34, 0.0),
        "board" => b(220.0 * p, 330.0 * p, 0.11, Wave::Sine, 0.22, 0.0),
        "coin" => j("B5 E6", 0.07, Wave::Triangle, 0.20),
        "hurt" => b(300.0 * p, 120.0 * p, 0.15, Wave::Saw, 0.22, 0.0),
        "win" => j("C5 E5 G5 C6", 0.10, Wave::Triangle, 0.22),
        "lose" => j("E4 C4 A3", 0.14, Wave::Square, 0.20),
        "squeak" => b(900.0 * p, 1400.0 * p, 0.08, Wave::Sine, 0.18, 0.0),
        "roar" => {
            b(220.0 * p, 60.0 * p, 0.5, Wave::Saw, 0.28, 0.0);
            b(300.0, 300.0, 0.35, Wave::Noise, 0.14, 0.0);
        }
        "bark" => {
            b(520.0 * p, 520.0 * p, 0.06, Wave::Square, 0.30, 0.0);
            b(340.0 * p, 340.0 * p, 0.06, Wave::Square, 0.30, 0.06);
        }
        "moo" => b(200.0 * p, 150.0 * p, 0.35, Wave::Square, 0.18, 0.0),
        "clank" => {
            b(980.0 * p, 980.0 * p, 0.07, Wave::Square, 0.30, 0.0);
            b(300.0 * p, 300.0 * p, 0.07, Wave::Square, 0.30, 0.07);
        }
        "whip" => b(420.0 * p, 1500.0 * p, 0.09, Wave::Square, 0.25, 0.0),
        _ => return false,
    }
    true
}

/// 2D form: full volume, centred.
pub fn play_named(name: &str, pitch: f32) -> bool {
    play_named_at(name, pitch, 1.0, 0.0)
}

fn wave_sample(wave: Wave, phase: f32, noise: &mut u32) -> f32 {
    match wave {
        Wave::Sine => (phase * std::f32::consts::TAU).sin(),
        Wave::Square => {
            if phase < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        Wave::Saw => 2.0 * phase - 1.0,
        Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
        Wave::Noise => {
            *noise ^= *noise << 13;
            *noise ^= *noise >> 17;
            *noise ^= *noise << 5;
            (*noise as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }
}

/// Mix all live voices additively into `output`. Runs on the audio thread —
/// no allocation, one brief lock. Callers zero or fill the buffer first.
pub fn mix_into(output: &mut AudioBuffer, sample_rate: f64) {
    let Ok(mut synth) = SYNTH.lock() else { return };
    if synth.voices.is_empty() && synth.tones.is_empty() {
        return;
    }
    let dt = 1.0 / sample_rate as f32;
    let frames = output.frame_count();
    let channels = output.channel_count();
    for frame in 0..frames {
        // Sustained tones are non-positional (an engine hum belongs to the
        // whole scene), so they land in the centre sum.
        let mut centre = 0.0f32;
        let mut left = 0.0f32;
        let mut right = 0.0f32;
        for tone in synth.tones.iter_mut() {
            tone.freq += (tone.freq_target - tone.freq) * (TONE_SMOOTH_RATE * dt).min(1.0);
            tone.gain += (tone.gain_target - tone.gain) * (TONE_SMOOTH_RATE * dt).min(1.0);
            if tone.releasing {
                tone.level -= TONE_RELEASE_RATE * dt;
            } else {
                tone.level = (tone.level + TONE_ATTACK_RATE * dt).min(1.0);
            }
            if tone.level <= 0.0 {
                continue;
            }
            tone.phase = (tone.phase + tone.freq * dt).fract();
            let raw = wave_sample(tone.wave, tone.phase, &mut tone.noise);
            centre += raw * tone.level * tone.gain;
        }
        for voice in synth.voices.iter_mut() {
            if voice.delay > 0.0 {
                voice.delay -= dt;
                continue;
            }
            if voice.t >= voice.len {
                continue;
            }
            let u = voice.t / voice.len;
            let freq = voice.freq_from + (voice.freq_to - voice.freq_from) * u;
            voice.phase = (voice.phase + freq * dt).fract();
            let raw = wave_sample(voice.wave, voice.phase, &mut voice.noise);
            let attack = (voice.t / ATTACK_SECS).min(1.0);
            let envelope = attack * (1.0 - u) * (1.0 - u);
            let sample = raw * envelope * voice.gain;
            if voice.pan == 0.0 {
                centre += sample;
            } else {
                let (gl, gr) = pan_gains(voice.pan);
                left += sample * gl;
                right += sample * gr;
            }
            voice.t += dt;
        }
        if centre != 0.0 || left != 0.0 || right != 0.0 {
            for channel in 0..channels {
                // Channel 0 is left, 1 is right; anything beyond a stereo pair
                // (or a mono device) gets the unpanned sum so nothing is lost.
                let sample = match channel {
                    0 if channels >= 2 => centre + left,
                    1 if channels >= 2 => centre + right,
                    _ => centre + left + right,
                };
                output.channel_mut(channel)[frame] += sample.clamp(-0.9, 0.9);
            }
        }
    }
    synth.voices.retain(|v| v.delay > 0.0 || v.t < v.len);
    synth.tones.retain(|t| !(t.releasing && t.level <= 0.0));
}

/// Drop every live voice and tone. Used when a world is torn down so a new
/// game never inherits the previous one's noise.
pub fn reset() {
    let Ok(mut synth) = SYNTH.lock() else { return };
    synth.voices.clear();
    synth.tones.clear();
}

/// Live voice + tone count, for tests and diagnostics.
pub fn live_counts() -> (usize, usize) {
    match SYNTH.lock() {
        Ok(synth) => (synth.voices.len(), synth.tones.len()),
        Err(_) => (0, 0),
    }
}

/// The synth is a process-global, so tests that assert on voice counts must
/// not interleave — including tests in the modules that drive it.
#[cfg(test)]
pub static SYNTH_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_widgets::makepad_platform::audio::AudioBuffer;

    fn buffer() -> AudioBuffer {
        let mut b = AudioBuffer::new_with_size(256, 2);
        b.zero();
        b
    }

    fn peak(buf: &AudioBuffer, channel: usize) -> f32 {
        buf.channel(channel)
            .iter()
            .fold(0.0f32, |acc, s| acc.max(s.abs()))
    }

    #[test]
    fn a_named_sfx_produces_audible_samples() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        assert!(play_named("jump", 1.0), "jump is in the bank");
        let mut buf = buffer();
        mix_into(&mut buf, 44100.0);
        assert!(peak(&buf, 0) > 0.0, "a played sfx must be audible");
        reset();
    }

    #[test]
    fn an_unknown_sfx_name_is_reported_not_played() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        assert!(!play_named("not-a-sound", 1.0));
        assert_eq!(live_counts().0, 0, "an unknown name must queue nothing");
    }

    #[test]
    fn pan_moves_the_sound_between_the_channels() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Hard right: the right channel is louder than the left.
        reset();
        beep_panned(440.0, 440.0, 0.2, Wave::Square, 0.5, 0.0, 1.0);
        let mut buf = buffer();
        mix_into(&mut buf, 44100.0);
        let (l, r) = (peak(&buf, 0), peak(&buf, 1));
        assert!(r > l, "panned right: left {l} right {r}");
        assert_eq!(l, 0.0, "hard right must be silent on the left");

        // Hard left is the mirror image.
        reset();
        beep_panned(440.0, 440.0, 0.2, Wave::Square, 0.5, 0.0, -1.0);
        let mut buf = buffer();
        mix_into(&mut buf, 44100.0);
        let (l, r) = (peak(&buf, 0), peak(&buf, 1));
        assert!(l > r, "panned left: left {l} right {r}");
        assert_eq!(r, 0.0);
        reset();
    }

    #[test]
    fn a_centred_sound_is_equal_and_keeps_full_volume() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        beep(440.0, 440.0, 0.2, Wave::Square, 0.5, 0.0);
        let mut centred = buffer();
        mix_into(&mut centred, 44100.0);
        let (l, r) = (peak(&centred, 0), peak(&centred, 1));
        assert_eq!(l, r, "centre must be identical in both channels");
        assert!(l > 0.0);
        reset();
    }

    #[test]
    fn distance_attenuation_scales_the_bank() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        play_named_at("jump", 1.0, 1.0, 0.0);
        let mut loud = buffer();
        mix_into(&mut loud, 44100.0);
        let near = peak(&loud, 0);

        reset();
        play_named_at("jump", 1.0, 0.2, 0.0);
        let mut quiet = buffer();
        mix_into(&mut quiet, 44100.0);
        let far = peak(&quiet, 0);

        assert!(far < near, "distant {far} must be quieter than near {near}");
        assert!(far > 0.0, "still audible inside range");
        reset();
    }

    #[test]
    fn tones_sustain_until_stopped_and_reset_clears_them() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        tone(7, 220.0, Wave::Saw, 0.4);
        let mut buf = buffer();
        mix_into(&mut buf, 44100.0);
        assert_eq!(live_counts().1, 1, "a tone sustains across buffers");
        assert!(peak(&buf, 0) > 0.0);

        tone_stop(7);
        for _ in 0..40 {
            let mut buf = buffer();
            mix_into(&mut buf, 44100.0);
        }
        assert_eq!(live_counts().1, 0, "a stopped tone releases and is dropped");

        tone(8, 220.0, Wave::Saw, 0.4);
        stop_all_tones();
        for _ in 0..40 {
            let mut buf = buffer();
            mix_into(&mut buf, 44100.0);
        }
        assert_eq!(live_counts().1, 0, "stop_all_tones releases everything");
        reset();
    }

    #[test]
    fn the_voice_cap_holds_under_a_runaway_script() {
        let _guard = super::SYNTH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        for _ in 0..200 {
            beep(440.0, 440.0, 1.0, Wave::Sine, 0.2, 0.0);
        }
        assert_eq!(live_counts().0, MAX_VOICES, "oldest voices are dropped");
        reset();
    }

    #[test]
    fn note_names_parse_to_concert_pitch() {
        // A4 is the tuning reference; C5 is three semitones above A4's octave.
        let a4 = note_freq("A4").unwrap();
        assert!((a4 - 440.0).abs() < 0.01, "A4 = {a4}");
        let a5 = note_freq("A5").unwrap();
        assert!((a5 - 880.0).abs() < 0.02, "A5 = {a5}");
        assert!(note_freq("H9").is_none(), "unknown tokens are rests");
    }
}
