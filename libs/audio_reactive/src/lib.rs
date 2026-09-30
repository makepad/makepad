//! `makepad-audio-reactive`: the played sound as an input for looks and
//! games.
//!
//! * [`AudioReactive`] analyses whatever the host plays (mono or stereo
//!   samples at their rate, a frame's worth at a time) into a small float
//!   texture: a scrolling log spectrogram (~5.5 s), the waveform (~1.4 s)
//!   and the stereo side, plus the signals a look reads without a texture
//!   read: smoothed band levels, auto-gained levels, onsets (kick, snare,
//!   hat), loudness against its recent maximum, a short/long energy ratio,
//!   and song form (a drop after a breakdown, the section's energy). One
//!   analyser per app: every consumer gets the same [`AudioFrame`].
//! * [`bind_audio`] puts a frame onto any draw call whose shader declares
//!   the uniforms (by name: a shader that declares none is untouched).
//! * [`SPLASH`] is those declarations and the reading helpers as Splash
//!   shader fields (`audio_fft(f, age)`, `audio_wave(t)`,
//!   `audio_wave2(t)`), for a runtime shader (a kit, a material, a game's
//!   look) to include.
//! * [`bed`] is a deterministic music bed for hosts that have no music (a
//!   thumbnail baker, a gallery, tests, an attract mode).
//!
//! Silence is a valid picture: with nothing played every value is 0, the
//! same as an unbound texture, so a look carries its own idle floor.

pub mod analysis;
pub mod bed;

pub use analysis::{bind_audio, AudioFrame, AudioReactive, AUDIO_BINS, AUDIO_SIDE_ROWS, AUDIO_SPEC_ROWS, AUDIO_TEX_H, AUDIO_TEX_W, AUDIO_WAVE_ROWS};

/// The audio input as Splash shader fields: the texture, its uniforms, and
/// the three reading helpers. Paste it into a runtime draw shader or
/// material (`mod.draw.MyLook = mod.draw.DrawQuad{ <SPLASH> ... }`) and bind
/// frames with [`bind_audio`].
///
/// * `self.audio_fft(f, age)` — spectrum magnitude 0..1; `f` log frequency
///   0..1 (30 Hz .. 16 kHz), `age` 0..1 back in time (0 = now, 1 ≈ 5.5 s).
/// * `self.audio_wave(t)` — the waveform -1..1; `t` 0..1 across the kept
///   window (1 = newest).
/// * `self.audio_wave2(t)` — the stereo pair (L, R) at `t`.
/// * `self.audio_env` (bass, mid, high, rms), `self.audio_hit` (kick,
///   snare, hat, loud), `self.audio_norm` (auto-gained levels),
///   `self.audio_form` (drop, section, 0, 0).
pub const SPLASH: &str = r#"
        audio_tex: texture_2d(float)
        audio_dim: uniform(vec4(256.0, 256.0, 0.0, 0.0))
        audio_meta: uniform(vec4(256.0, 384.0, 64.0, 0.0213))
        audio_env: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        audio_hit: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        audio_norm: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        audio_form: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        audio_fft: fn(f: float, age: float) -> float {
            let rows = max(self.audio_dim.y, 1.0)
            let x = (clamp(f, 0.0, 1.0) * (self.audio_dim.x - 1.0) + 0.5) / self.audio_meta.x
            let back = clamp(age, 0.0, 1.0) * (rows - 1.0)
            let row = modf(self.audio_dim.z - back + rows * 2.0, rows)
            return self.audio_tex.sample_nearest(vec2(x, (row + 0.5) / self.audio_meta.y), 0.0).x
        }
        audio_wave: fn(t: float) -> float {
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + row + 0.5) / self.audio_meta.y
            return self.audio_tex.sample_nearest(vec2((col + 0.5) / self.audio_meta.x, y), 0.0).x
        }
        audio_wave2: fn(t: float) -> vec2 {
            let m = self.audio_wave(t)
            let bins = max(self.audio_dim.x, 1.0)
            let wrows = max(self.audio_meta.z, 1.0)
            let back = (1.0 - clamp(t, 0.0, 1.0)) * (bins * wrows - 1.0)
            let pos = (bins - 1.0) - back
            let ro = floor(pos / bins)
            let col = pos - ro * bins
            let row = modf(self.audio_dim.w + ro + wrows * 4.0, wrows)
            let y = (self.audio_dim.y + wrows + row + 0.5) / self.audio_meta.y
            let sd = self.audio_tex.sample_nearest(vec2((col + 0.5) / self.audio_meta.x, y), 0.0).x
            return vec2(m + sd, m - sd)
        }
"#;
