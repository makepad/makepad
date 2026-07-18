//! Game sound effects: a tiny polyphonic synthesizer.
//!
//! Games may not ship asset files — the whole art style is procedural — so
//! sound is procedural too. The Godot corpus proved the need the hard way:
//! given no audio API, the agent hand-built exactly this synth inside GDScript,
//! encoding WAV samples one by one. The engine provides it as a service
//! instead: a named bank of kid-game staples plus raw `beep`/`jingle`
//! primitives, mixed additively into the app's existing audio output callback.

use makepad_widgets::makepad_platform::audio::AudioBuffer;
use std::sync::Mutex;

/// Percussive envelope attack, long enough to avoid clicks.
const ATTACK_SECS: f32 = 0.004;
/// Voice cap: oldest voice is dropped, a stuck script can't build a wall of sound.
const MAX_VOICES: usize = 24;

#[derive(Clone, Copy, PartialEq)]
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

struct Voice {
    wave: Wave,
    freq_from: f32,
    freq_to: f32,
    len: f32,
    gain: f32,
    /// Seconds until the voice starts — how jingles sequence notes.
    delay: f32,
    t: f32,
    phase: f32,
    noise: u32,
}

/// Sustained voice cap — engine hums, wind, sirens. Small on purpose.
const MAX_TONES: usize = 6;
/// Parameter smoothing rate (≈30ms to target) so per-tick retuning from
/// `game.tone_set` glides instead of zipper-stepping.
const TONE_SMOOTH_RATE: f32 = 33.0;
const TONE_ATTACK_RATE: f32 = 60.0;
const TONE_RELEASE_RATE: f32 = 18.0;

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
    next_tone_id: u64,
}

/// Shared with the audio callback in main.rs. The script side only ever
/// pushes voices; the audio thread only ever advances and drops them.
static SYNTH: Mutex<Synth> = Mutex::new(Synth {
    voices: Vec::new(),
    tones: Vec::new(),
    next_tone_id: 0,
});

/// Start a sustained tone; returns its id for `tone_set`/`tone_stop`.
pub fn tone(freq: f32, wave: Wave, gain: f32) -> u64 {
    let Ok(mut synth) = SYNTH.lock() else { return 0 };
    if synth.tones.len() >= MAX_TONES {
        synth.tones.remove(0);
    }
    synth.next_tone_id += 1;
    let id = synth.next_tone_id;
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
    id
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

/// A rebuilt world must never inherit a stuck engine hum: called on every
/// eval/reset from GameWorld::reset_content.
pub fn stop_all_tones() {
    let Ok(mut synth) = SYNTH.lock() else { return };
    for tone in synth.tones.iter_mut() {
        tone.releasing = true;
    }
}

/// One tone, optionally gliding from `freq` to `to` over its length.
pub fn beep(freq: f32, to: f32, secs: f32, wave: Wave, gain: f32, delay: f32) {
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

/// The kid-game staple bank. Names match what generated games reach for —
/// the Godot corpus invented jump/shoot/zap/grab/angry/calm/rescue/shove/
/// board/win on its own; coin/hurt/lose round out the obvious asks.
pub fn play_named(name: &str, pitch: f32) -> bool {
    let p = pitch.clamp(0.25, 4.0);
    match name {
        "jump" => beep(260.0 * p, 540.0 * p, 0.12, Wave::Square, 0.22, 0.0),
        "shoot" => beep(880.0 * p, 180.0 * p, 0.09, Wave::Square, 0.20, 0.0),
        "zap" => {
            beep(1200.0 * p, 90.0 * p, 0.18, Wave::Saw, 0.22, 0.0);
            beep(600.0, 600.0, 0.10, Wave::Noise, 0.12, 0.0);
        }
        "grab" => beep(320.0 * p, 180.0 * p, 0.12, Wave::Sine, 0.25, 0.0),
        "angry" => beep(150.0 * p, 90.0 * p, 0.25, Wave::Square, 0.22, 0.0),
        "calm" => beep(390.0 * p, 520.0 * p, 0.20, Wave::Sine, 0.20, 0.0),
        "rescue" => jingle("E5 G5", 0.09, Wave::Triangle, 0.22),
        "shove" => beep(200.0, 200.0, 0.06, Wave::Noise, 0.30, 0.0),
        "board" => beep(220.0 * p, 330.0 * p, 0.11, Wave::Sine, 0.22, 0.0),
        "coin" => jingle("B5 E6", 0.07, Wave::Triangle, 0.20),
        "hurt" => beep(300.0 * p, 120.0 * p, 0.15, Wave::Saw, 0.22, 0.0),
        "win" => jingle("C5 E5 G5 C6", 0.10, Wave::Triangle, 0.22),
        "lose" => jingle("E4 C4 A3", 0.14, Wave::Square, 0.20),
        // Both invented by the corpus AI when the bank lacked them.
        "squeak" => beep(900.0 * p, 1400.0 * p, 0.08, Wave::Sine, 0.18, 0.0),
        "roar" => {
            beep(220.0 * p, 60.0 * p, 0.5, Wave::Saw, 0.28, 0.0);
            beep(300.0, 300.0, 0.35, Wave::Noise, 0.14, 0.0);
        }
        // The menagerie bank, matching the game's sfx.gd recipes: bark/clank
        // are two-note squares, whip a fast up-sweep, moo a slow low glide.
        "bark" => {
            beep(520.0 * p, 520.0 * p, 0.06, Wave::Square, 0.30, 0.0);
            beep(340.0 * p, 340.0 * p, 0.06, Wave::Square, 0.30, 0.06);
        }
        "moo" => beep(200.0 * p, 150.0 * p, 0.35, Wave::Square, 0.18, 0.0),
        "clank" => {
            beep(980.0 * p, 980.0 * p, 0.07, Wave::Square, 0.30, 0.0);
            beep(300.0 * p, 300.0 * p, 0.07, Wave::Square, 0.30, 0.07);
        }
        "whip" => beep(420.0 * p, 1500.0 * p, 0.09, Wave::Square, 0.25, 0.0),
        _ => return false,
    }
    true
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
        let mut sample = 0.0f32;
        for tone in synth.tones.iter_mut() {
            // Smooth toward targets; ramp level up (attack) or down (release).
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
            let raw = match tone.wave {
                Wave::Sine => (tone.phase * std::f32::consts::TAU).sin(),
                Wave::Square => {
                    if tone.phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Saw => 2.0 * tone.phase - 1.0,
                Wave::Triangle => 1.0 - 4.0 * (tone.phase - 0.5).abs(),
                Wave::Noise => {
                    tone.noise ^= tone.noise << 13;
                    tone.noise ^= tone.noise >> 17;
                    tone.noise ^= tone.noise << 5;
                    (tone.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
                }
            };
            sample += raw * tone.level * tone.gain;
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
            let raw = match voice.wave {
                Wave::Sine => (voice.phase * std::f32::consts::TAU).sin(),
                Wave::Square => {
                    if voice.phase < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                Wave::Saw => 2.0 * voice.phase - 1.0,
                Wave::Triangle => 1.0 - 4.0 * (voice.phase - 0.5).abs(),
                Wave::Noise => {
                    voice.noise ^= voice.noise << 13;
                    voice.noise ^= voice.noise >> 17;
                    voice.noise ^= voice.noise << 5;
                    (voice.noise as f32 / u32::MAX as f32) * 2.0 - 1.0
                }
            };
            let attack = (voice.t / ATTACK_SECS).min(1.0);
            let envelope = attack * (1.0 - u) * (1.0 - u);
            sample += raw * envelope * voice.gain;
            voice.t += dt;
        }
        if sample != 0.0 {
            let sample = sample.clamp(-0.9, 0.9);
            for channel in 0..channels {
                output.channel_mut(channel)[frame] += sample;
            }
        }
    }
    synth.voices.retain(|v| v.delay > 0.0 || v.t < v.len);
    synth.tones.retain(|t| !(t.releasing && t.level <= 0.0));
}
