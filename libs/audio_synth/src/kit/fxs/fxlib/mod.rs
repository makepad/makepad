//! The small-effects host (effect type 14): one slot runs one of 77
//! sub-effects ported from Chris Johnson's MIT-licensed effects collection
//! (credited in CREDITS.md; each ported file carries the MIT notice).
//!
//! Slot layout: p0 = sub-effect id (0..76, append-only, interop), p1..p11 =
//! the sub-effect's own parameters in its order: floats 0..1, or integral
//! choices 0..upper bound.
//!
//! Host behaviour: a 32-sample block runs as 8 sub-blocks of 4. Before each
//! sub-block every float parameter goes through a one-pole lag (0.016 per
//! sub-block) from clamp(v, 0, 1) and is handed to the sub-effect as its
//! normalised value; integral parameters are handed as value / upper bound
//! without lag. The sub-effect gets the sample rate at creation. For
//! DeRez (id 15) inputs with |x| <= 2e-15 are zeroed first. Effects are
//! 100 % wet (they carry their own dry/wet). Ring-out: always runs.
//!
//! Each sub-effect is a faithful port of the upstream float processing
//! path, including its double-precision maths, its denormal guard and its
//! 32-bit floating point dither with its own xorshift state. Upstream seeds
//! that state from the C library `rand()`; [`CRand`] reproduces that
//! generator (additive feedback, 34-word state) so runs are deterministic.

use crate::kit::core::{Lag, SlotDef, SlotValue};

pub mod adclip7;
pub mod air;
pub mod channel_grit;
pub mod bass_drive;
pub mod bit_glitter;
pub mod block_party;
pub mod bright_ambience2;
pub mod buss_colors4;
pub mod butter_comp2;
pub mod cabs;
pub mod capacitor;
pub mod chamber;
pub mod chrome_oxide;
pub mod cojones;
pub mod compresaturator;
pub mod crunchy_groove_wear;
pub mod de_bess;
pub mod deckwrecka;
pub mod density;
pub mod derez2;
pub mod drive;
pub mod drum_slam;
pub mod dub_center;
pub mod dub_sub;
pub mod dust_bunny;
pub mod fire_amp;
pub mod focus;
pub mod fracture;
pub mod galactic;
pub mod glitch_shifter;
pub mod groove_wear;
pub mod hard_vacuum;
pub mod hombre;
pub mod infinity;
pub mod iron_oxide5;
pub mod logical4;
pub mod loud;
pub mod mixer_eq;
pub mod mixer_drive;
pub mod matrix_verb;
pub mod melt;
pub mod mojo;
pub mod nc_seventeen;
pub mod noise;
pub mod nonlinear_space;
pub mod one_corner_clip;
pub mod pafnuty;
pub mod pocket_verbs;
pub mod point;
pub mod pop;
pub mod power_sag;
pub mod pressure4;
pub mod pyewacket;
pub mod single_ended_triode;
pub mod slew;
pub mod slew2;
pub mod spiral2;
pub mod star_child;
pub mod swell;
pub mod tape_dust;
pub mod to_tape6;
pub mod to_vinyl4;
pub mod triple_spread;
pub mod unbox;
pub mod vari_mu;
pub mod verbity;
pub mod voice_of_the_starship;
pub mod ybandpass;
pub mod yhighpass;
pub mod ylowpass;
pub mod ynotch;
pub mod zbandpass2;
pub mod zhighpass2;
pub mod zlowpass2;
pub mod znotch2;

/// Display names, index = stored id.
pub const SUB_NAMES: [&str; 77] = [
    "AD Clip", "Block Party", "Butter Comp", "Compresaturator", "Logical", "Mojo", "One Corner Clip", "Point",
    "Pop", "Pressure", "Pye Wacket", "Swell", "Vari-Mu", "Bit Glitter", "Crunchy Groove Wear", "DeRez",
    "Deck Wrecka", "Dust Bunny", "Groove Wear", "Noise", "Voice Of The Starship", "Bright Ambience", "Hombre",
    "Melt", "Pocket Verbs", "Star Child", "Channel Grit", "Bass Drive", "Cojones", "Density", "Drive", "Focus",
    "Fracture", "Hard Vacuum", "Loud", "NC-17", "Spiral", "Unbox", "De-Bess", "NoOp (was DeEss)",
    "Single-Ended Triode", "Iron Oxide", "To Tape", "Air", "Buss Colors", "Drum Slam", "Capacitor", "Slew 1",
    "Slew 2", "Galactic", "Infinity", "MatrixVerb", "Verbity", "Triple Spread", "Chamber", "Mixer Drive", "Mixer EQ",
    "NoOp (was Tube)", "Cabs", "Chrome Oxide", "Dub Sub", "Dub Center", "Fire Amp", "Glitch Shifter",
    "Nonlinear Space", "Pafnuty", "Power Sag", "Tape Dust", "To Vinyl", "YLowpass", "YBandpass", "YHighpass",
    "YNotch", "ZLowpass", "ZBandpass", "ZHighpass", "ZNotch",
];

/// Display group and order of each id (UI only; `None` = hidden NoOp).
pub const SUB_GROUPS: [(&str, Option<u16>); 77] = [
    ("Clipping", Some(10)), ("Dynamics", Some(20)), ("Dynamics", Some(30)), ("Dynamics", Some(40)),
    ("Dynamics", Some(50)), ("Saturation And More", Some(355)), ("Clipping", Some(70)), ("Dynamics", Some(80)),
    ("Dynamics", Some(90)), ("Dynamics", Some(100)), ("Dynamics", Some(110)), ("Dynamics", Some(120)),
    ("Dynamics", Some(130)), ("Lo-Fi", Some(140)), ("Lo-Fi", Some(150)), ("Lo-Fi", Some(160)), ("Lo-Fi", Some(170)),
    ("Noise", Some(180)), ("Lo-Fi", Some(190)), ("Noise", Some(200)), ("Noise", Some(210)), ("Ambience", Some(220)),
    ("Filter", Some(420)), ("Ambience", Some(240)), ("Ambience", Some(250)), ("Ambience", Some(260)),
    ("Saturation And More", Some(270)), ("Saturation And More", Some(280)), ("Saturation And More", Some(290)),
    ("Saturation And More", Some(300)), ("Saturation And More", Some(310)), ("Saturation And More", Some(320)),
    ("Saturation And More", Some(330)), ("Saturation And More", Some(340)), ("Saturation And More", Some(350)),
    ("Saturation And More", Some(360)), ("Saturation And More", Some(390)), ("Saturation And More", Some(395)),
    ("Filter", Some(415)), ("Filter", None), ("Saturation And More", Some(380)), ("Tape", Some(461)),
    ("Tape", Some(462)), ("Filter", Some(400)), ("Saturation And More", Some(285)), ("Dynamics", Some(46)),
    ("Filter", Some(410)), ("Clipping", Some(113)), ("Clipping", Some(114)), ("Ambience", Some(227)),
    ("Ambience", Some(230)), ("Ambience", Some(235)), ("Ambience", Some(265)), ("Stereo", Some(440)),
    ("Ambience", Some(223)), ("Saturation And More", Some(353)), ("Filter", Some(425)),
    ("Saturation And More", None), ("Filter", Some(405)), ("Tape", Some(460)), ("Filter", Some(422)),
    ("Filter", Some(421)), ("Saturation And More", Some(312)), ("Pitch", Some(500)), ("Ambience", Some(245)),
    ("Filter", Some(428)), ("Saturation And More", Some(362)), ("Noise", Some(205)), ("Lo-Fi", Some(195)),
    ("Filter", Some(430)), ("Filter", Some(431)), ("Filter", Some(432)), ("Filter", Some(433)), ("Filter", Some(434)),
    ("Filter", Some(435)), ("Filter", Some(436)), ("Filter", Some(437)),
];

/// The id whose inputs are zeroed below 2e-15.
pub const DEREZ_ID: u8 = 15;
/// The reference zeroes DeRez's inputs with |x| <= 2e-15 before the sub-effect runs (its bit-depth reduction
/// otherwise turns denormal-guard noise into steps); mirrored so presets render as heard.
pub const QUIRK_DEREZ_ZERO_TINY_INPUTS: bool = true;
/// The reference's patch loader reads an integral sub-effect parameter through a float view of the stored int,
/// which leaves it at 0 after a load; mirrored by [`on_patch_load`] so loaded presets sound as their authors heard
/// them (unverified against the reference app, see the spec's open question).
pub const QUIRK_INTEGRAL_PARAMS_RESET_ON_LOAD: bool = true;
/// Parameter slots after the id.
pub const MAX_PARAMS: usize = 11;
/// Sub-block length.
pub const SUB_BLOCK: usize = 4;
/// Lag rate per sub-block.
pub const PARAM_LAG: f32 = 0.016;

/// One ported sub-effect.
pub trait Sub: Send {
    /// Store parameter `i` (normalised 0..1, as the host hands it).
    fn set_param(&mut self, i: usize, v: f32);
    /// Process one sub-block in place (stereo in, stereo out).
    fn process(&mut self, l: &mut [f32], r: &mut [f32]);
}

macro_rules! subs {
    ($($id:literal => $m:ident :: $t:ident,)*) => {
        fn defs(id: u8) -> &'static [SlotDef] {
            match id {
                $($id => $m::PARAMS,)*
                _ => &[],
            }
        }
        fn make(id: u8, sr: f64, rng: &mut CRand) -> Option<Box<dyn Sub>> {
            match id {
                $($id => Some($m::$t::new(sr, rng)),)*
                _ => None,
            }
        }
    };
}

subs! {
    0 => adclip7::AdClip7,
    1 => block_party::BlockParty,
    2 => butter_comp2::ButterComp2,
    3 => compresaturator::Compresaturator,
    4 => logical4::Logical4,
    5 => mojo::Mojo,
    6 => one_corner_clip::OneCornerClip,
    7 => point::Point,
    8 => pop::Pop,
    9 => pressure4::Pressure4,
    10 => pyewacket::Pyewacket,
    11 => swell::Swell,
    12 => vari_mu::VariMu,
    13 => bit_glitter::BitGlitter,
    14 => crunchy_groove_wear::CrunchyGrooveWear,
    15 => derez2::DeRez2,
    16 => deckwrecka::Deckwrecka,
    17 => dust_bunny::DustBunny,
    18 => groove_wear::GrooveWear,
    19 => noise::Noise,
    20 => voice_of_the_starship::VoiceOfTheStarship,
    21 => bright_ambience2::BrightAmbience2,
    22 => hombre::Hombre,
    23 => melt::Melt,
    24 => pocket_verbs::PocketVerbs,
    25 => star_child::StarChild,
    26 => channel_grit::ChannelGrit,
    27 => bass_drive::BassDrive,
    28 => cojones::Cojones,
    29 => density::Density,
    30 => drive::Drive,
    31 => focus::Focus,
    32 => fracture::Fracture,
    33 => hard_vacuum::HardVacuum,
    34 => loud::Loud,
    35 => nc_seventeen::NcSeventeen,
    36 => spiral2::Spiral2,
    37 => unbox::UnBox,
    38 => de_bess::DeBess,
    40 => single_ended_triode::SingleEndedTriode,
    41 => iron_oxide5::IronOxide5,
    42 => to_tape6::ToTape6,
    43 => air::Air,
    44 => buss_colors4::BussColors4,
    45 => drum_slam::DrumSlam,
    46 => capacitor::Capacitor,
    47 => slew::Slew,
    48 => slew2::Slew2,
    49 => galactic::Galactic,
    50 => infinity::Infinity,
    51 => matrix_verb::MatrixVerb,
    52 => verbity::Verbity,
    53 => triple_spread::TripleSpread,
    54 => chamber::Chamber,
    55 => mixer_drive::MixerDrive,
    56 => mixer_eq::MixerEq,
    58 => cabs::Cabs,
    59 => chrome_oxide::ChromeOxide,
    60 => dub_sub::DubSub,
    61 => dub_center::DubCenter,
    62 => fire_amp::FireAmp,
    63 => glitch_shifter::GlitchShifter,
    64 => nonlinear_space::NonlinearSpace,
    65 => pafnuty::Pafnuty,
    66 => power_sag::PowerSag,
    67 => tape_dust::TapeDust,
    68 => to_vinyl4::ToVinyl4,
    69 => ylowpass::YLowpass,
    70 => ybandpass::YBandpass,
    71 => yhighpass::YHighpass,
    72 => ynotch::YNotch,
    73 => zlowpass2::ZLowpass2,
    74 => zbandpass2::ZBandpass2,
    75 => zhighpass2::ZHighpass2,
    76 => znotch2::ZNotch2,
}

/// The parameters of a sub-effect (names, ranges, defaults = the upstream
/// constructor's, integral choices). At most [`MAX_PARAMS`]; NoOps have none.
pub fn sub_param_defs(id: u8) -> Vec<SlotDef> {
    defs(id).to_vec()
}

/// The slot values a user selection of `id` loads (p0 = id, then the
/// sub-effect's defaults, unused trailing slots int 0).
pub fn sub_defaults(id: u8) -> [SlotValue; 12] {
    let mut p = [SlotValue::int(0); 12];
    p[0] = SlotValue::int(id as i32);
    for (i, d) in defs(id).iter().take(MAX_PARAMS).enumerate() {
        p[i + 1] = d.default_value();
    }
    p
}

/// Patch-load fix-up of one slot's values (call after reading p0..p11 of a type-14 slot from a patch).
pub fn on_patch_load(p: &mut [SlotValue; 12]) {
    let id = p[0].v.round();
    if !(0.0..77.0).contains(&id) {
        return;
    }
    if QUIRK_INTEGRAL_PARAMS_RESET_ON_LOAD {
        for (i, d) in defs(id as u8).iter().take(MAX_PARAMS).enumerate() {
            if d.int {
                p[i + 1] = SlotValue::int(0);
            }
        }
    }
}

/// A float sub-effect parameter (0..1) with its upstream default.
pub const fn pf(name: &'static str, default: f32) -> SlotDef {
    SlotDef::float(name, 0.0, 1.0, default, crate::kit::core::SlotUnit::None)
}

/// The C library `rand()` as upstream calls it (the additive feedback
/// generator with a 34-word warm-up, outputs 0..=[`CRand::MAX`]).
#[derive(Clone, Debug)]
pub struct CRand {
    r: [u32; 34],
    i: usize,
}

impl CRand {
    pub const MAX: u32 = 2_147_483_647;

    pub fn new(seed: u32) -> CRand {
        let mut r = [0u32; 34];
        let seed = if seed == 0 { 1 } else { seed };
        r[0] = seed;
        for i in 1..31 {
            let hi = (r[i - 1] as i32) / 127_773;
            let lo = (r[i - 1] as i32) % 127_773;
            let mut word = 16_807i32.wrapping_mul(lo).wrapping_sub(2_836i32.wrapping_mul(hi));
            if word < 0 {
                word = word.wrapping_add(2_147_483_647);
            }
            r[i] = word as u32;
        }
        for i in 31..34 {
            r[i] = r[i - 31];
        }
        let mut g = CRand { r, i: 0 };
        for _ in 34..344 {
            g.step();
        }
        g
    }

    #[inline]
    fn step(&mut self) -> u32 {
        // r[k] = r[k-31] + r[k-3], ring of 34
        let k = self.i;
        let v = self.r[(k + 34 - 31) % 34].wrapping_add(self.r[(k + 34 - 3) % 34]);
        self.r[k] = v;
        self.i = (k + 1) % 34;
        v
    }

    /// One `rand()` value.
    #[inline]
    pub fn next(&mut self) -> u32 {
        self.step() >> 1
    }

    /// A generator for a sub-effect that calls `rand()` while it runs.
    pub fn fork(&mut self) -> CRand {
        CRand::new(self.next())
    }

    /// The upstream dither-state seeding:
    /// `fpd = 1; while (fpd < 16386) fpd = rand() * UINT32_MAX;`
    pub fn fpd(&mut self) -> u32 {
        let mut f = 1u32;
        while f < 16386 {
            f = self.next().wrapping_mul(u32::MAX);
        }
        f
    }
}

/// The exponent C `frexpf` reports (x = m 2^e, 0.5 <= |m| < 1; 0 for zero
/// and non-finite values).
#[inline]
pub fn frexpf_exp(x: f32) -> i32 {
    if x == 0.0 || !x.is_finite() {
        return 0;
    }
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xff) as i32;
    if e == 0 {
        // subnormal: scale into the normal range first
        let y = x * 33_554_432.0; // 2^25
        let e2 = ((y.to_bits() >> 23) & 0xff) as i32;
        return e2 - 126 - 25;
    }
    e - 126
}

/// The exponent C `frexp` reports for a double.
#[inline]
pub fn frexp_exp(x: f64) -> i32 {
    if x == 0.0 || !x.is_finite() {
        return 0;
    }
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32;
    if e == 0 {
        let y = x * 18_014_398_509_481_984.0; // 2^54
        let e2 = ((y.to_bits() >> 52) & 0x7ff) as i32;
        return e2 - 1022 - 54;
    }
    e - 1022
}

/// One xorshift step of a dither state.
#[inline]
pub fn xorshift(fpd: &mut u32) -> u32 {
    let mut x = *fpd;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *fpd = x;
    x
}

/// The upstream "32 bit floating point dither" of one channel:
/// frexpf exponent, one xorshift step, add
/// `(fpd - 0x7fffffff) * 5.5e-36 * 2^(e+62)`.
#[inline]
pub fn dither32(x: f64, fpd: &mut u32) -> f64 {
    let e = frexpf_exp(x as f32);
    xorshift(fpd);
    x + ((*fpd as f64) - 2_147_483_647.0) * 5.5e-36 * pow2i(e + 62)
}

/// 2^e as `pow(2, e)` gives it for an int exponent.
#[inline]
pub fn pow2i(e: i32) -> f64 {
    2f64.powi(e)
}

/// The upstream denormal guard: |x| < 1.18e-23 becomes fpd * 1.18e-17.
#[inline]
pub fn denorm(x: f64, fpd: u32) -> f64 {
    if x.abs() < 1.18e-23 {
        fpd as f64 * 1.18e-17
    } else {
        x
    }
}

/// C's `(int)x` for a double (truncation toward zero, saturating).
#[inline]
pub fn to_int(x: f64) -> i32 {
    x as i32
}

/// The effect slot.
pub struct FxLibrary {
    sr: f32,
    id: i32,
    sub: Option<Box<dyn Sub>>,
    defs: &'static [SlotDef],
    lags: [Lag; MAX_PARAMS],
    rng: CRand,
}

impl FxLibrary {
    pub fn new(sr: f32) -> FxLibrary {
        FxLibrary { sr, id: -1, sub: None, defs: &[], lags: [Lag::new(PARAM_LAG); MAX_PARAMS], rng: CRand::new(1) }
    }

    /// The sub-effect id running now (-1 before the first block).
    pub fn id(&self) -> i32 {
        self.id
    }

    /// Drop the sub-effect state (the next block recreates it).
    pub fn reset(&mut self) {
        self.id = -1;
        self.sub = None;
    }

    /// Build the sub-effect `p0` names (allocates: called by `Fx::prepare`
    /// off the render path; a later change of p0 rebuilds in `process`).
    pub fn prepare(&mut self, p: &[SlotValue; 12]) {
        self.select(p);
    }

    fn select(&mut self, p: &[SlotValue; 12]) {
        let id = p[0].v.round() as i32;
        if id == self.id {
            return;
        }
        self.id = id;
        let uid = if (0..77).contains(&id) { id as u8 } else { 255 };
        self.defs = defs(uid);
        // a sub-effect change is a type change: construct (allocates)
        self.sub = make(uid, self.sr as f64, &mut self.rng);
        self.lags = [Lag::new(PARAM_LAG); MAX_PARAMS];
    }

    fn feed_params(&mut self, p: &[SlotValue; 12]) {
        let Some(sub) = self.sub.as_mut() else { return };
        for (i, d) in self.defs.iter().take(MAX_PARAMS).enumerate() {
            let v = if d.int {
                let ub = d.max.max(0.0);
                if ub > 0.0 { p[i + 1].v.round().clamp(0.0, ub) / ub } else { 0.0 }
            } else {
                self.lags[i].set(p[i + 1].v.clamp(0.0, 1.0));
                self.lags[i].process()
            };
            sub.set_param(i, v);
        }
    }

    /// One block (in place). `_ctx` is the effect context; nothing here
    /// needs it.
    pub fn process<C>(&mut self, p: &[SlotValue; 12], _ctx: &C, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.select(p);
        if self.sub.is_none() {
            return; // NoOp ids and unknown ids pass through
        }
        if QUIRK_DEREZ_ZERO_TINY_INPUTS && self.id == DEREZ_ID as i32 {
            for x in l.iter_mut().chain(r.iter_mut()) {
                if x.abs() <= 2e-15 {
                    *x = 0.0;
                }
            }
        }
        for (lc, rc) in l.chunks_mut(SUB_BLOCK).zip(r.chunks_mut(SUB_BLOCK)) {
            self.feed_params(p);
            if let Some(sub) = self.sub.as_mut() {
                sub.process(lc, rc);
            }
        }
    }

    /// A block without audio: the parameter lags still move.
    pub fn process_only_control<C>(&mut self, p: &[SlotValue; 12], _ctx: &C) {
        self.select(p);
        for _ in 0..(32 / SUB_BLOCK) {
            self.feed_params(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48000.0;

    fn render_at(p: &[SlotValue; 12], blocks: usize, seed: u32, sr: f32) -> (Vec<f32>, Vec<f32>) {
        let mut fx = FxLibrary::new(sr);
        let mut noise = crate::dsp::Rng::new(seed);
        let (mut outl, mut outr) = (Vec::new(), Vec::new());
        let mut n = 0usize;
        for _ in 0..blocks {
            let mut l = [0.0f32; 32];
            let mut r = [0.0f32; 32];
            for i in 0..32 {
                let t = n as f32 / sr;
                let s = 0.3 * (2.0 * std::f32::consts::PI * 220.0 * t).sin();
                l[i] = s + 0.1 * noise.bipolar();
                r[i] = 0.8 * s + 0.1 * noise.bipolar();
                n += 1;
            }
            fx.process(p, &(), &mut l, &mut r);
            outl.extend_from_slice(&l);
            outr.extend_from_slice(&r);
        }
        (outl, outr)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| (*v as f64) * (*v as f64)).sum::<f64>() / x.len() as f64).sqrt() as f32
    }

    /// Dub Sub and Dub Center: their three output levels default to the centre of a -1..1 range.
    const SILENT_AT_DEFAULTS: [u8; 2] = [60, 61];

    #[test]
    fn every_sub_effect_survives_random_settings() {
        let mut bad = Vec::new();
        let mut g = crate::dsp::Rng::new(99);
        for id in 0..77u8 {
            let defs = sub_param_defs(id);
            for set in 0..10 {
                let mut p = sub_defaults(id);
                for (i, d) in defs.iter().enumerate() {
                    let u = match set {
                        0 => 0.0,
                        1 => 1.0,
                        _ => g.unit(),
                    };
                    p[i + 1] = if d.int { SlotValue::int((u * d.max).round() as i32) } else { SlotValue::float(u) };
                }
                for sr in [SR, 96000.0] {
                    let run = std::panic::catch_unwind(|| render_at(&p, 400, 3, sr));
                    match run {
                        Err(_) => bad.push(format!("{} ({}) set {} panicked @ {}", SUB_NAMES[id as usize], id, set, sr)),
                        Ok((l, r)) => {
                            if !l.iter().chain(r.iter()).all(|v| v.is_finite()) {
                                bad.push(format!("{} ({}) set {} not finite @ {}", SUB_NAMES[id as usize], id, set, sr));
                            }
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    fn check_defaults(id: u8, sr: f32) -> Result<(), String> {
        let blocks = (sr as usize) / 32; // one second
        let name = SUB_NAMES[id as usize];
        let p = sub_defaults(id);
        let run = std::panic::catch_unwind(|| render_at(&p, blocks, 7, sr));
        let Ok((l, r)) = run else { return Err(format!("{} ({}) panicked at {} Hz", name, id, sr)) };
        if !l.iter().chain(r.iter()).all(|v| v.is_finite()) {
            return Err(format!("{} ({}) not finite", name, id));
        }
        let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        if peak >= 8.0 {
            return Err(format!("{} ({}) peak {}", name, id, peak));
        }
        let lvl = rms(&l[l.len() / 2..]).max(rms(&r[r.len() / 2..]));
        if SILENT_AT_DEFAULTS.contains(&id) {
            // every output gain sits at its centre (0) by default: dither-level only
            if lvl > 1e-6 {
                return Err(format!("{} ({}) should be silent at defaults: rms {}", name, id, lvl));
            }
        } else if lvl <= 1e-4 {
            return Err(format!("{} ({}) silent at defaults: rms {}", name, id, lvl));
        }
        let (l2, r2) = render_at(&p, blocks, 7, sr);
        if l != l2 || r != r2 {
            return Err(format!("{} ({}) not deterministic", name, id));
        }
        Ok(())
    }

    #[test]
    fn every_sub_effect_at_defaults_is_finite_sane_and_deterministic() {
        let mut bad = Vec::new();
        for sr in [SR, 44100.0, 96000.0] {
            for id in 0..77u8 {
                if let Err(e) = check_defaults(id, sr) {
                    bad.push(format!("{} @ {}", e, sr));
                }
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    /// Sub-effects whose dry/wet at 0 still differs from the input: Fire Amp scales its dry by the output level.
    const DRY_NOT_UNITY: [u8; 1] = [62];

    #[test]
    fn dry_wet_at_zero_is_unity() {
        let mut bad = Vec::new();
        for id in 0..77u8 {
            let defs = sub_param_defs(id);
            let Some(k) = defs.iter().position(|d| d.name == "Dry/Wet" || d.name == "Dry/Wet ") else { continue };
            let mut p = sub_defaults(id);
            p[k + 1] = SlotValue::float(0.0);
            let mut fx = FxLibrary::new(SR);
            let mut worst = 0.0f32;
            let mut noise = crate::dsp::Rng::new(5);
            for _ in 0..200 {
                let mut l = [0.0f32; 32];
                let mut r = [0.0f32; 32];
                for i in 0..32 {
                    l[i] = 0.25 * noise.bipolar();
                    r[i] = 0.25 * noise.bipolar();
                }
                let (l0, r0) = (l, r);
                fx.process(&p, &(), &mut l, &mut r);
                for i in 0..32 {
                    worst = worst.max((l[i] - l0[i]).abs()).max((r[i] - r0[i]).abs());
                }
            }
            let expect = !DRY_NOT_UNITY.contains(&id);
            if expect != (worst < 1e-6) {
                bad.push(format!("{} ({}): dry/wet 0 max error {}", SUB_NAMES[id as usize], id, worst));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn defs_fit_the_slots() {
        for id in 0..77u8 {
            let d = sub_param_defs(id);
            assert!(d.len() <= MAX_PARAMS, "{}", SUB_NAMES[id as usize]);
            for s in &d {
                assert!(s.used());
                assert!(s.default >= s.min && s.default <= s.max, "{} {}", SUB_NAMES[id as usize], s.name);
                if !s.int {
                    assert!(s.min == 0.0 && s.max == 1.0);
                }
            }
            let has = d.is_empty() == matches!(id, 39 | 57);
            assert!(has, "{} params missing", SUB_NAMES[id as usize]);
        }
    }

    #[test]
    fn noops_are_exact_pass_through() {
        for id in [39u8, 57] {
            let p = sub_defaults(id);
            let mut fx = FxLibrary::new(SR);
            let mut l = [0.0f32; 32];
            let mut r = [0.0f32; 32];
            for i in 0..32 {
                l[i] = (i as f32 * 0.1).sin();
                r[i] = -(i as f32 * 0.07).cos();
            }
            let (l0, r0) = (l, r);
            fx.process(&p, &(), &mut l, &mut r);
            assert_eq!(l, l0);
            assert_eq!(r, r0);
        }
    }

    #[test]
    fn crand_matches_the_c_library_sequence() {
        // the first outputs of rand() after srand(1)
        let mut g = CRand::new(1);
        let first: Vec<u32> = (0..5).map(|_| g.next()).collect();
        assert_eq!(first, vec![1804289383, 846930886, 1681692777, 1714636915, 1957747793]);
    }

    #[test]
    fn frexp_exponents() {
        assert_eq!(frexpf_exp(1.0), 1);
        assert_eq!(frexpf_exp(0.5), 0);
        assert_eq!(frexpf_exp(0.75), 0);
        assert_eq!(frexpf_exp(-3.0), 2);
        assert_eq!(frexpf_exp(0.0), 0);
        assert_eq!(frexpf_exp(f32::from_bits(1)), -148);
        assert_eq!(frexp_exp(1.0), 1);
        assert_eq!(frexp_exp(f64::from_bits(1)), -1073);
    }

    #[test]
    fn integral_params_are_handed_as_fractions_without_lag() {
        struct Probe(std::sync::Arc<std::sync::Mutex<Vec<(usize, f32)>>>);
        impl Sub for Probe {
            fn set_param(&mut self, i: usize, v: f32) {
                self.0.lock().unwrap().push((i, v));
            }
            fn process(&mut self, _l: &mut [f32], _r: &mut [f32]) {}
        }
        static DEFS: [SlotDef; 2] = [SlotDef::float("a", 0.0, 1.0, 0.5, crate::kit::core::SlotUnit::None), SlotDef::choice("m", &["x", "y", "z"], 0.0)];
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut fx = FxLibrary::new(SR);
        fx.id = 0;
        fx.defs = &DEFS;
        fx.sub = Some(Box::new(Probe(log.clone())));
        let mut p = [SlotValue::int(0); 12];
        p[1] = SlotValue::float(0.25);
        p[2] = SlotValue::int(1);
        let (mut l, mut r) = ([0.0f32; 32], [0.0f32; 32]);
        fx.process(&p, &(), &mut l, &mut r);
        p[1] = SlotValue::float(1.0);
        p[2] = SlotValue::int(2);
        fx.process(&p, &(), &mut l, &mut r);
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 32);
        assert_eq!(log[0], (0, 0.25)); // first value snaps
        assert_eq!(log[1], (1, 0.5));
        let a1 = log[16].1;
        assert!((a1 - (0.25 + 0.016 * 0.75)).abs() < 1e-6); // one lag step
        assert_eq!(log[17], (1, 1.0)); // no lag
    }
}

/// The slot kinds of the host: p0 the sub-effect, p1..p11 its parameters
/// (0..1 each; integral ones hold their choice index, see [`sub_param_defs`]).
pub fn slot_defs() -> [SlotDef; 12] {
    let mut d = [SlotDef::float("param", 0.0, 1.0, 0.0, crate::kit::core::SlotUnit::Percent); 12];
    d[0] = SlotDef { can_deactivate: true, ..SlotDef::choice("effect", &SUB_NAMES, 0.0) };
    d
}
