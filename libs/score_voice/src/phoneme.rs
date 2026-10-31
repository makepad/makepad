//! The phoneme inventory (English, in IPA): what each phoneme is (vowel,
//! nasal, stop, ...), the vocal-tract resonances the built-in voices are
//! drawn from, its noise and burst spectra and how long it takes in a sung
//! syllable. A phoneme is a `u8` index into [`PHONEMES`].
//!
//! Vowel formants are the classic measured averages for adult men and women
//! (F1..F3); the consonants carry the formant *loci* their transitions head
//! for (labial low F2, alveolar mid, velar and palatal high), so a morph
//! from a vowel into a consonant moves the formants the way a mouth does.

pub type Ph = u8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Silence,
    Vowel,
    Nasal,
    /// l, r: vowel-like, a little weaker.
    Liquid,
    /// w, j: a quick vowel glide.
    Glide,
    Fricative,
    /// A closure, a burst, then aspiration (voiceless) or voicing.
    Stop,
    /// A stop released into a fricative (ʧ ʤ).
    Affricate,
    /// h: breath shaped by the vowel that follows.
    Aspirate,
    /// ɾ: a very short voiced dip.
    Flap,
    /// ʔ: a short silence.
    Glottal,
    /// The low murmur inside a voiced closure (internal).
    VoiceBar,
}

/// A noise spectrum: a peak, the level there (dB per band) and the slopes
/// below and above it (dB per octave).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseSpec {
    pub peak: f32,
    pub level: f32,
    pub below: f32,
    pub above: f32,
}

impl NoiseSpec {
    pub const NONE: NoiseSpec = NoiseSpec { peak: 1000.0, level: crate::bands::FLOOR_DB, below: 0.0, above: 0.0 };

    pub fn db_at(&self, hz: f32) -> f32 {
        if self.level <= crate::bands::FLOOR_DB {
            return crate::bands::FLOOR_DB;
        }
        let oct = (hz / self.peak).log2();
        self.level - if oct < 0.0 { -oct * self.below } else { oct * self.above }
    }
}

const fn ns(peak: f32, level: f32, below: f32, above: f32) -> NoiseSpec {
    NoiseSpec { peak, level, below, above }
}

#[derive(Clone, Copy, Debug)]
pub struct PhonemeInfo {
    /// IPA as Kokoro's phone timings write it.
    pub ipa: &'static str,
    pub class: Class,
    pub voiced: bool,
    /// F1..F4 of an adult male and female voice (Hz).
    pub male: [f32; 4],
    pub female: [f32; 4],
    /// Harmonic level relative to a full vowel (dB).
    pub gain: f32,
    /// An anti-resonance (nasals), Hz, or 0.
    pub zero: f32,
    /// Frication or breath.
    pub noise: NoiseSpec,
    /// The release burst of a stop or affricate.
    pub burst: NoiseSpec,
    /// Sung duration as an onset and as a coda (seconds). For a stop the
    /// onset is the closure, the burst and aspiration follow.
    pub onset: f32,
    pub coda: f32,
}

const N: NoiseSpec = NoiseSpec::NONE;

const fn v(ipa: &'static str, male: [f32; 3], female: [f32; 3]) -> PhonemeInfo {
    PhonemeInfo {
        ipa, class: Class::Vowel, voiced: true, male: [male[0], male[1], male[2], 3400.0], female: [female[0], female[1], female[2], 4000.0],
        gain: 0.0, zero: 0.0, noise: N, burst: N, onset: 0.0, coda: 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
const fn c(ipa: &'static str, class: Class, voiced: bool, male: [f32; 4], gain: f32, zero: f32, noise: NoiseSpec, burst: NoiseSpec, onset: f32, coda: f32) -> PhonemeInfo {
    // Consonant loci of a female voice sit about 15% higher.
    let female = [male[0] * 1.1, male[1] * 1.15, male[2] * 1.15, male[3] * 1.15];
    PhonemeInfo { ipa, class, voiced, male, female, gain, zero, noise, burst, onset, coda }
}

pub const SIL: Ph = 0;
pub const VOICEBAR: Ph = 43;

/// The inventory. Ids are stable: voice profiles store one shape per id.
pub const PHONEMES: [PhonemeInfo; 44] = [
    c("", Class::Silence, false, [500.0, 1500.0, 2500.0, 3400.0], -120.0, 0.0, N, N, 0.0, 0.0),
    v("i", [270.0, 2290.0, 3010.0], [310.0, 2790.0, 3310.0]),
    v("ɪ", [390.0, 1990.0, 2550.0], [430.0, 2480.0, 3070.0]),
    v("e", [476.0, 2089.0, 2691.0], [536.0, 2530.0, 3047.0]),
    v("ɛ", [530.0, 1840.0, 2480.0], [610.0, 2330.0, 2990.0]),
    v("æ", [660.0, 1720.0, 2410.0], [860.0, 2050.0, 2850.0]),
    v("a", [750.0, 1300.0, 2500.0], [900.0, 1500.0, 2900.0]),
    v("ɑ", [730.0, 1090.0, 2440.0], [850.0, 1220.0, 2810.0]),
    v("ɒ", [620.0, 950.0, 2400.0], [700.0, 1050.0, 2800.0]),
    v("ɔ", [570.0, 840.0, 2410.0], [590.0, 920.0, 2710.0]),
    v("o", [497.0, 910.0, 2459.0], [555.0, 1035.0, 2828.0]),
    v("ʊ", [440.0, 1020.0, 2240.0], [470.0, 1160.0, 2680.0]),
    v("u", [300.0, 870.0, 2240.0], [370.0, 950.0, 2670.0]),
    v("ʌ", [640.0, 1190.0, 2390.0], [760.0, 1400.0, 2780.0]),
    v("ə", [500.0, 1500.0, 2500.0], [550.0, 1700.0, 2850.0]),
    v("ɜ", [520.0, 1400.0, 2450.0], [580.0, 1600.0, 2850.0]),
    v("ɝ", [490.0, 1350.0, 1690.0], [500.0, 1640.0, 1960.0]),
    // Stops: labial, alveolar, velar; closure, then burst and aspiration.
    c("p", Class::Stop, false, [300.0, 800.0, 2200.0, 3300.0], -120.0, 0.0, N, ns(700.0, -30.0, 0.0, 6.0), 0.05, 0.06),
    c("b", Class::Stop, true, [300.0, 800.0, 2200.0, 3300.0], -120.0, 0.0, N, ns(700.0, -36.0, 0.0, 8.0), 0.04, 0.05),
    c("t", Class::Stop, false, [350.0, 1750.0, 2600.0, 3400.0], -120.0, 0.0, N, ns(4500.0, -26.0, 12.0, 3.0), 0.05, 0.06),
    c("d", Class::Stop, true, [350.0, 1750.0, 2600.0, 3400.0], -120.0, 0.0, N, ns(4000.0, -32.0, 12.0, 6.0), 0.04, 0.05),
    c("k", Class::Stop, false, [320.0, 2100.0, 2500.0, 3400.0], -120.0, 0.0, N, ns(2000.0, -26.0, 18.0, 12.0), 0.05, 0.06),
    c("ɡ", Class::Stop, true, [320.0, 2100.0, 2500.0, 3400.0], -120.0, 0.0, N, ns(2000.0, -32.0, 18.0, 12.0), 0.04, 0.05),
    // Fricatives.
    c("f", Class::Fricative, false, [300.0, 900.0, 2200.0, 3300.0], -120.0, 0.0, ns(4000.0, -50.0, 6.0, 0.0), N, 0.08, 0.09),
    c("v", Class::Fricative, true, [300.0, 900.0, 2200.0, 3300.0], -18.0, 0.0, ns(4000.0, -55.0, 6.0, 0.0), N, 0.06, 0.07),
    c("θ", Class::Fricative, false, [320.0, 1500.0, 2500.0, 3400.0], -120.0, 0.0, ns(5000.0, -52.0, 6.0, 0.0), N, 0.08, 0.09),
    c("ð", Class::Fricative, true, [320.0, 1500.0, 2500.0, 3400.0], -16.0, 0.0, ns(5000.0, -57.0, 6.0, 0.0), N, 0.05, 0.06),
    c("s", Class::Fricative, false, [350.0, 1750.0, 2600.0, 3400.0], -120.0, 0.0, ns(6300.0, -34.0, 20.0, 6.0), N, 0.10, 0.11),
    c("z", Class::Fricative, true, [350.0, 1750.0, 2600.0, 3400.0], -18.0, 0.0, ns(6300.0, -41.0, 20.0, 6.0), N, 0.07, 0.08),
    c("ʃ", Class::Fricative, false, [300.0, 2100.0, 2500.0, 3400.0], -120.0, 0.0, ns(3150.0, -33.0, 24.0, 9.0), N, 0.10, 0.11),
    c("ʒ", Class::Fricative, true, [300.0, 2100.0, 2500.0, 3400.0], -18.0, 0.0, ns(3150.0, -40.0, 24.0, 9.0), N, 0.07, 0.08),
    c("h", Class::Aspirate, false, [500.0, 1500.0, 2500.0, 3400.0], -120.0, 0.0, N, N, 0.06, 0.0),
    // Nasals: a low murmur with an anti-resonance.
    c("m", Class::Nasal, true, [280.0, 1000.0, 2200.0, 3300.0], -8.0, 900.0, N, N, 0.07, 0.10),
    c("n", Class::Nasal, true, [280.0, 1700.0, 2600.0, 3400.0], -8.0, 1400.0, N, N, 0.07, 0.10),
    c("ŋ", Class::Nasal, true, [280.0, 2000.0, 2700.0, 3500.0], -9.0, 3000.0, N, N, 0.07, 0.10),
    // Liquids and glides.
    c("l", Class::Liquid, true, [360.0, 1300.0, 2700.0, 3400.0], -4.0, 0.0, N, N, 0.06, 0.08),
    c("ɹ", Class::Liquid, true, [350.0, 1100.0, 1600.0, 3200.0], -4.0, 0.0, N, N, 0.06, 0.08),
    c("w", Class::Glide, true, [300.0, 700.0, 2200.0, 3300.0], -5.0, 0.0, N, N, 0.06, 0.06),
    c("j", Class::Glide, true, [280.0, 2250.0, 2900.0, 3500.0], -4.0, 0.0, N, N, 0.06, 0.06),
    // Affricates: a stop closure released into frication.
    c("ʧ", Class::Affricate, false, [300.0, 2100.0, 2500.0, 3400.0], -120.0, 0.0, ns(3150.0, -33.0, 24.0, 9.0), ns(4000.0, -28.0, 12.0, 6.0), 0.05, 0.06),
    c("ʤ", Class::Affricate, true, [300.0, 2100.0, 2500.0, 3400.0], -18.0, 0.0, ns(3150.0, -40.0, 24.0, 9.0), ns(4000.0, -34.0, 12.0, 6.0), 0.04, 0.05),
    c("ɾ", Class::Flap, true, [350.0, 1750.0, 2600.0, 3400.0], -10.0, 0.0, N, N, 0.03, 0.03),
    c("ʔ", Class::Glottal, false, [500.0, 1500.0, 2500.0, 3400.0], -120.0, 0.0, N, N, 0.03, 0.03),
    c("", Class::VoiceBar, true, [180.0, 1200.0, 2500.0, 3400.0], -24.0, 0.0, N, N, 0.0, 0.0),
];

pub const COUNT: usize = PHONEMES.len();

pub fn info(ph: Ph) -> &'static PhonemeInfo {
    &PHONEMES[(ph as usize).min(COUNT - 1)]
}

pub fn is_vowel(ph: Ph) -> bool {
    info(ph).class == Class::Vowel
}

/// The phoneme a consonant's frication comes from (an affricate's second half).
pub fn frication_of(ph: Ph) -> Ph {
    match info(ph).ipa {
        "ʧ" => by_ipa("ʃ").unwrap(),
        "ʤ" => by_ipa("ʒ").unwrap(),
        _ => ph,
    }
}

pub fn by_ipa(ipa: &str) -> Option<Ph> {
    PHONEMES.iter().position(|p| !p.ipa.is_empty() && p.ipa == ipa).map(|i| i as Ph)
}

/// Parse a pronunciation: IPA as Kokoro writes it (its single-letter
/// diphthongs `A I O W Y Q` too), stress and length marks ignored, spaces
/// ignored. Unknown symbols are skipped. Diphthongs become two vowels (the
/// second is the offglide).
pub fn parse(pron: &str) -> Vec<Ph> {
    let mut out = Vec::new();
    let chars: Vec<char> = pron.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        let two = |a: &str, b: &str, out: &mut Vec<Ph>| {
            out.push(by_ipa(a).unwrap());
            out.push(by_ipa(b).unwrap());
        };
        match ch {
            'A' => two("e", "ɪ", &mut out),
            'I' => two("a", "ɪ", &mut out),
            'O' => two("o", "ʊ", &mut out),
            'W' => two("a", "ʊ", &mut out),
            'Y' => two("ɔ", "ɪ", &mut out),
            'Q' => two("ə", "ʊ", &mut out),
            'ᵊ' | 'ɐ' | 'ɨ' | 'ᵻ' => out.push(by_ipa("ə").unwrap()),
            'ɚ' => out.push(by_ipa("ɝ").unwrap()),
            'r' | 'ɻ' => out.push(by_ipa("ɹ").unwrap()),
            'g' => out.push(by_ipa("ɡ").unwrap()),
            'y' => out.push(by_ipa("j").unwrap()),
            'ʦ' => {
                out.push(by_ipa("t").unwrap());
                out.push(by_ipa("s").unwrap());
            }
            'T' => out.push(by_ipa("ɾ").unwrap()),
            'ɫ' => out.push(by_ipa("l").unwrap()),
            'c' => out.push(by_ipa("k").unwrap()),
            'ç' | 'x' | 'χ' => out.push(by_ipa("h").unwrap()),
            _ => {
                let mut s = [0u8; 4];
                if let Some(ph) = by_ipa(ch.encode_utf8(&mut s)) {
                    out.push(ph);
                }
                // Stress, length, spaces and anything unknown are skipped.
            }
        }
    }
    out
}

/// A pronunciation back as IPA (for display and tests).
pub fn to_ipa(phs: &[Ph]) -> String {
    phs.iter().map(|p| info(*p).ipa).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_ids_and_parsing() {
        assert_eq!(PHONEMES[VOICEBAR as usize].class, Class::VoiceBar);
        assert_eq!(to_ipa(&parse("həlˈO")), "həloʊ");
        assert_eq!(to_ipa(&parse("sˈIn")), "saɪn");
        assert_eq!(to_ipa(&parse("lˈʌv")), "lʌv");
        assert_eq!(to_ipa(&parse("ʧˈɝʧ")), "ʧɝʧ");
        for (i, p) in PHONEMES.iter().enumerate() {
            if !p.ipa.is_empty() {
                assert_eq!(by_ipa(p.ipa), Some(i as Ph), "{}", p.ipa);
            }
        }
    }
}
