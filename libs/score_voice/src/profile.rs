//! A voice profile: one spectral shape per phoneme (the harmonic envelope,
//! the noise bands, the burst of a stop and the formant anchors a morph
//! warps between), plus the voice's home pitch. Built-in profiles are drawn
//! from the phoneme table's formants; analysed ones come from speech or a
//! vocal (see `analysis`). A profile is immutable and shared (`Arc`); its
//! stored form is about 4 KB (`encode` / `decode`).

use crate::bands::*;
use crate::phoneme::{self, Class, Ph, COUNT, PHONEMES};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Harmonic envelope (dB at the envelope grid), source tilt included.
    pub env: [f32; ENV_POINTS],
    /// Noise band levels (dB): frication, or the aperiodic part of a voiced
    /// sound.
    pub noise: [f32; NOISE_BANDS],
    /// A stop's release burst (dB per band).
    pub burst: [f32; NOISE_BANDS],
    /// F1..F4 (Hz): the anchors of formant-warping morphs.
    pub formants: [f32; 4],
}

impl Shape {
    pub const SILENT: Shape = Shape {
        env: [FLOOR_DB; ENV_POINTS],
        noise: [FLOOR_DB; NOISE_BANDS],
        burst: [FLOOR_DB; NOISE_BANDS],
        formants: [500.0, 1500.0, 2500.0, 3400.0],
    };
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceProfile {
    pub name: String,
    pub shapes: Vec<Shape>,
    /// The pitch the envelopes were made at (Hz): harmonic levels are
    /// normalised to it so loudness holds across the range.
    pub f0_ref: f32,
    /// Comfortable range, MIDI notes.
    pub range: [f32; 2],
    /// Default breathiness, 0..1.
    pub breath: f32,
}

/// The built-in voices: formant sets of an adult man or woman, scaled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Builtin {
    Soprano,
    Alto,
    Tenor,
    Baritone,
    Bass,
}

impl Builtin {
    pub const ALL: [Builtin; 5] = [Builtin::Soprano, Builtin::Alto, Builtin::Tenor, Builtin::Baritone, Builtin::Bass];

    pub fn name(self) -> &'static str {
        match self {
            Builtin::Soprano => "soprano",
            Builtin::Alto => "alto",
            Builtin::Tenor => "tenor",
            Builtin::Baritone => "baritone",
            Builtin::Bass => "bass",
        }
    }

    pub fn from_name(name: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.name() == name)
    }

    /// (female formant table, formant scale, f0_ref Hz, range, ring dB).
    fn params(self) -> (bool, f32, f32, [f32; 2], f32) {
        match self {
            Builtin::Soprano => (true, 1.04, 330.0, [60.0, 84.0], 2.0),
            Builtin::Alto => (true, 1.0, 220.0, [53.0, 77.0], 2.0),
            Builtin::Tenor => (false, 1.03, 165.0, [48.0, 72.0], 5.0),
            Builtin::Baritone => (false, 1.0, 123.0, [43.0, 67.0], 5.0),
            Builtin::Bass => (false, 0.95, 98.0, [38.0, 62.0], 5.0),
        }
    }
}

/// Level of a pole pair at `f` (dB), unity at DC.
fn pole_db(f: f32, formant: f32, bw: f32) -> f32 {
    let h = bw * 0.5;
    let num = formant * formant + h * h;
    let den = (((f - formant).powi(2) + h * h) * ((f + formant).powi(2) + h * h)).sqrt();
    20.0 * (num / den).log10()
}

fn formant_bandwidths(formants: &[f32; 4]) -> [f32; 5] {
    // Wider at higher formants and at higher F1 (more damping).
    [60.0 + formants[0] * 0.06, 80.0 + formants[1] * 0.02, 120.0 + formants[2] * 0.02, 200.0, 260.0]
}

/// The harmonic envelope of a formant set: a cascade of resonances over a
/// glottal source falling 6 dB an octave (the lip radiation's rise
/// included), an optional anti-resonance, and a "ring" near 3 kHz (the
/// cluster that makes a trained voice carry).
pub fn formant_envelope(formants: [f32; 4], zero: f32, ring_db: f32, extra_lowpass: f32) -> [f32; ENV_POINTS] {
    let bws = formant_bandwidths(&formants);
    let f5 = formants[3] + 750.0;
    std::array::from_fn(|i| {
        let f = env_hz(i);
        let mut db = 0.0;
        for (k, fk) in formants.iter().chain(std::iter::once(&f5)).enumerate() {
            db += pole_db(f, *fk, bws[k]);
        }
        if zero > 0.0 {
            db -= pole_db(f, zero, 150.0);
        }
        // Source: -6 dB/oct above 150 Hz.
        db -= 6.0 * (1.0 + (f / 150.0).powi(2)).sqrt().log2();
        // Ring: a broad lift centred at 3 kHz.
        let oct = (f / 3000.0).log2();
        db += ring_db * (-oct * oct * 8.0).exp();
        if extra_lowpass > 0.0 && f > 600.0 {
            db -= extra_lowpass * (f / 600.0).log2();
        }
        db
    })
}

fn noise_from_spec(spec: &phoneme::NoiseSpec, scale: f32) -> [f32; NOISE_BANDS] {
    let spec = phoneme::NoiseSpec { peak: spec.peak * scale, ..*spec };
    std::array::from_fn(|b| spec.db_at(NOISE_CENTERS[b]).max(FLOOR_DB))
}

/// The aperiodic part of a voiced sound: the envelope's level less 30 dB
/// above 2 kHz, less 42 dB below.
pub fn aperiodic_noise(env: &[f32; ENV_POINTS], offset: f32) -> [f32; NOISE_BANDS] {
    std::array::from_fn(|b| {
        let fc = NOISE_CENTERS[b];
        env_at(env, fc) + offset + if fc >= 2000.0 { -30.0 } else { -42.0 }
    })
}

impl VoiceProfile {
    pub fn builtin(which: Builtin) -> VoiceProfile {
        let (female, scale, f0_ref, range, ring) = which.params();
        let mut shapes = Vec::with_capacity(COUNT);
        for info in PHONEMES.iter() {
            let base = if female { info.female } else { info.male };
            let formants = base.map(|f| f * scale);
            let voiced_obstruent = info.voiced && matches!(info.class, Class::Fricative | Class::Stop | Class::Affricate | Class::VoiceBar);
            let mut env = formant_envelope(formants, info.zero * scale, ring, if voiced_obstruent { 18.0 } else { 0.0 });
            // An unvoiced phoneme's envelope is never sung (its harmonic gain
            // is zero) but a morph toward it moves the formants to its loci.
            let gain = match info.class {
                Class::Vowel => 0.0,
                Class::Stop | Class::Affricate if info.voiced => -24.0,
                _ if !info.voiced => -10.0,
                _ => info.gain,
            };
            for e in env.iter_mut() {
                *e += gain;
            }
            let noise = match info.class {
                Class::Fricative | Class::Affricate => noise_from_spec(&info.noise, if female { 1.1 } else { 1.0 }),
                Class::Vowel | Class::Nasal | Class::Liquid | Class::Glide | Class::Flap => aperiodic_noise(&env, if info.class == Class::Vowel { 0.0 } else { -6.0 }),
                _ => [FLOOR_DB; NOISE_BANDS],
            };
            let burst = noise_from_spec(&info.burst, if female { 1.1 } else { 1.0 });
            shapes.push(Shape { env, noise, burst, formants });
        }
        let mut profile = VoiceProfile { name: which.name().to_string(), shapes, f0_ref, range, breath: 0.15 };
        profile.normalise(-14.0);
        profile
    }

    /// Shift every level so the loudest /ɑ/ harmonic sits at `peak_db`.
    pub fn normalise(&mut self, peak_db: f32) {
        let ah = phoneme::by_ipa("ɑ").unwrap() as usize;
        let peak = self.shapes[ah].env.iter().cloned().fold(FLOOR_DB, f32::max);
        let shift = peak_db - peak;
        for shape in &mut self.shapes {
            for v in shape.env.iter_mut().chain(shape.noise.iter_mut()).chain(shape.burst.iter_mut()) {
                if *v > FLOOR_DB {
                    *v = (*v + shift).max(FLOOR_DB);
                }
            }
        }
    }

    pub fn shape(&self, ph: Ph) -> &Shape {
        &self.shapes[(ph as usize).min(self.shapes.len() - 1)]
    }

    /// A named built-in (`soprano`, `alto`, `tenor`, `baritone`, `bass`).
    pub fn named(name: &str) -> Option<VoiceProfile> {
        Builtin::from_name(name).map(VoiceProfile::builtin)
    }

    const MAGIC: &'static [u8; 8] = b"MKVOX\x01\0\0";

    /// The stored form: levels as half-dB bytes, formants as u16 Hz.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.shapes.len() * 100);
        out.extend_from_slice(Self::MAGIC);
        let name = self.name.as_bytes();
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name);
        for v in [self.f0_ref, self.range[0], self.range[1], self.breath] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&(self.shapes.len() as u16).to_le_bytes());
        out.push(ENV_POINTS as u8);
        out.push(NOISE_BANDS as u8);
        for shape in &self.shapes {
            out.extend(shape.env.iter().map(|v| db_to_code(*v)));
            out.extend(shape.noise.iter().map(|v| db_to_code(*v)));
            out.extend(shape.burst.iter().map(|v| db_to_code(*v)));
            for f in shape.formants {
                out.extend_from_slice(&(f.round().clamp(0.0, 65535.0) as u16).to_le_bytes());
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<VoiceProfile, String> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8], String> {
            let s = bytes.get(at..at + n).ok_or("voice profile: truncated")?;
            at += n;
            Ok(s)
        };
        if take(8)? != Self::MAGIC {
            return Err("voice profile: not an MKVOX v1 file".into());
        }
        let name_len = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
        let name = String::from_utf8(take(name_len)?.to_vec()).map_err(|_| "voice profile: bad name")?;
        let mut f = [0.0f32; 4];
        for v in f.iter_mut() {
            *v = f32::from_le_bytes(take(4)?.try_into().unwrap());
        }
        let count = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
        let (points, bands) = (take(1)?[0] as usize, take(1)?[0] as usize);
        if points != ENV_POINTS || bands != NOISE_BANDS {
            return Err("voice profile: another band layout".into());
        }
        let mut shapes = Vec::with_capacity(COUNT);
        for _ in 0..count {
            let env = take(ENV_POINTS)?;
            let env = std::array::from_fn(|i| code_to_db(env[i]));
            let noise = take(NOISE_BANDS)?;
            let noise = std::array::from_fn(|i| code_to_db(noise[i]));
            let burst = take(NOISE_BANDS)?;
            let burst = std::array::from_fn(|i| code_to_db(burst[i]));
            let fm = take(8)?;
            let formants = std::array::from_fn(|i| u16::from_le_bytes([fm[i * 2], fm[i * 2 + 1]]) as f32);
            shapes.push(Shape { env, noise, burst, formants });
        }
        // A profile from an older, shorter inventory: the rest silent.
        while shapes.len() < COUNT {
            shapes.push(Shape::SILENT);
        }
        Ok(VoiceProfile { name, shapes, f0_ref: f[0], range: [f[1], f[2]], breath: f[3] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_envelopes_peak_at_their_formants_and_round_trip() {
        let p = VoiceProfile::builtin(Builtin::Baritone);
        let ah = p.shape(phoneme::by_ipa("ɑ").unwrap());
        // The envelope's largest peak below 1000 Hz sits near F1 = 730 Hz.
        let (mut best, mut at) = (FLOOR_DB, 0.0);
        for i in 0..ENV_POINTS {
            let f = env_hz(i);
            if f < 1000.0 && ah.env[i] > best {
                best = ah.env[i];
                at = f;
            }
        }
        assert!((at - 730.0).abs() < 60.0, "F1 peak at {at}");
        let bytes = p.encode();
        assert!(bytes.len() < 5000, "{}", bytes.len());
        let back = VoiceProfile::decode(&bytes).unwrap();
        assert_eq!(back.name, "baritone");
        for (a, b) in p.shapes.iter().zip(&back.shapes) {
            for (x, y) in a.env.iter().zip(&b.env) {
                assert!((x - y).abs() <= 0.26 || *x < -109.0, "{x} {y}");
            }
        }
    }
}
