//! The effects family (owner: SYNTHPORT-FX; airwindows: SYNTHPORT-AIRWIN;
//! nimbus: SYNTHPORT-TWIST): 32 type ids (0 = Off), one module per effect,
//! each with its slot definitions (what p0..p11 mean), per-type patch
//! migrations, its ring-out length and the stereo block processor. The
//! host calls `process` while the slot's input is present or its ring-out
//! has not run out, otherwise `process_only_control`.
//!
//! Every effect runs at the host rate in blocks of 32 samples, in place.
//! Constructors may allocate (delay lines, tables); `process` never does.

pub mod common;
pub mod fxlib;
pub mod texture;

pub mod audioin;
pub mod bonsai;
pub mod chorus;
pub mod chow;
pub mod combulator;
pub mod conditioner;
pub mod convolution;
pub mod delay;
pub mod distortion;
pub mod ensemble;
pub mod eq;
pub mod exciter;
pub mod flanger;
pub mod floaty;
pub mod freqshift;
pub mod geq;
pub mod mstool;
pub mod neuron;
pub mod phaser;
pub mod resonator;
pub mod reverb1;
pub mod reverb2;
pub mod ringmod;
pub mod rotary;
pub mod spring;
pub mod tape;
pub mod treemonster;
pub mod vocoder;
pub mod waveshaper;

#[cfg(test)]
mod tests;

use crate::kit::core::{SlotDef, SlotValue};

/// Display names, index = stored type id.
pub const TYPE_NAMES: [&str; 32] = [
    "Off",
    "Delay",
    "Reverb 1",
    "Phaser",
    "Rotary",
    "Distortion",
    "EQ",
    "Freq Shift",
    "Conditioner",
    "Chorus",
    "Vocoder",
    "Reverb 2",
    "Flanger",
    "Ring Mod",
    "Effect Library",
    "Neuron",
    "Graphic EQ",
    "Resonator",
    "Truncator",
    "Exciter",
    "Ensemble",
    "Combulator",
    "Texture",
    "Tape",
    "Treemonster",
    "Waveshaper",
    "Mid-Side Tool",
    "Spring Reverb",
    "Bonsai",
    "Audio In",
    "Floaty Delay",
    "Convolution",
];

/// The Airwindows host (lane AIRWIN) and the granular processor (lane TWIST).
pub const TYPE_FX_LIBRARY: u8 = 14;
pub const TYPE_TEXTURE: u8 = 22;

/// What the host hands an effect each block.
pub struct FxCtx<'a> {
    pub sr: f32,
    /// Host BPM / 120.
    pub tsr: f32,
    /// The host's audio input (base rate).
    pub audio_in: Option<(&'a [f32; 32], &'a [f32; 32])>,
    pub user_data: Option<&'a FxUserData>,
    /// For scene insert slots: the OTHER scene's output at its tap point
    /// (Audio In's "scene input").
    pub scene_in: Option<(&'a [f32; 32], &'a [f32; 32])>,
}

impl<'a> FxCtx<'a> {
    pub fn new(sr: f32) -> FxCtx<'a> {
        FxCtx { sr, tsr: 1.0, audio_in: None, user_data: None, scene_in: None }
    }
}

/// A slot's user data from the patch's block storage (key -> blob).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FxUserData {
    pub entries: Vec<(String, Vec<u8>)>,
}

impl FxUserData {
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_slice())
    }
    pub fn set(&mut self, key: &str, v: Vec<u8>) {
        if let Some(e) = self.entries.iter_mut().find(|(k, _)| k == key) {
            e.1 = v;
        } else {
            self.entries.push((key.to_string(), v));
        }
    }
}

macro_rules! fx_types {
    ($( $id:literal => $var:ident ( $m:ident :: $t:ident ) ),* $(,)?) => {
        /// One slot's effect state.
        pub enum Fx {
            Off,
            FxLibrary(Box<fxlib::FxLibrary>),
            Texture(Box<texture::Texture>),
            $( $var(Box<$m::$t>), )*
        }

        impl Fx {
            /// May allocate (delay lines etc.).
            pub fn new(ty: u8, sr: f32) -> Fx {
                match ty {
                    $( $id => Fx::$var(Box::new($m::$t::new(sr))), )*
                    TYPE_FX_LIBRARY => Fx::FxLibrary(Box::new(fxlib::FxLibrary::new(sr))),
                    TYPE_TEXTURE => Fx::Texture(Box::new(texture::Texture::new(sr))),
                    _ => Fx::Off,
                }
            }

            pub fn ty(&self) -> u8 {
                match self {
                    Fx::Off => 0,
                    Fx::FxLibrary(_) => TYPE_FX_LIBRARY,
                    Fx::Texture(_) => TYPE_TEXTURE,
                    $( Fx::$var(_) => $id, )*
                }
            }

            pub fn reset(&mut self) {
                match self {
                    Fx::Off => {}
                    Fx::FxLibrary(f) => f.reset(),
                    Fx::Texture(f) => f.reset(),
                    $( Fx::$var(f) => f.reset(), )*
                }
            }

            /// One block in place.
            pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
                match self {
                    Fx::Off => {}
                    Fx::FxLibrary(f) => f.process(p, ctx, l, r),
                    Fx::Texture(f) => f.process(p, ctx, l, r),
                    $( Fx::$var(f) => f.process(p, ctx, l, r), )*
                }
            }

            /// The control-rate part of a block while the slot rings out
            /// silent (LFO phases keep running, no audio).
            pub fn process_only_control(&mut self, p: &[SlotValue; 12], ctx: &FxCtx) {
                match self {
                    Fx::Off => {}
                    Fx::FxLibrary(f) => f.process_only_control(p, ctx),
                    Fx::Texture(f) => f.process_only_control(p, ctx),
                    $( Fx::$var(f) => f.process_only_control(p, ctx), )*
                }
            }
        }

        /// What p0..p11 mean for a type.
        pub fn slot_defs(ty: u8) -> [SlotDef; 12] {
            match ty {
                $( $id => $m::slot_defs(), )*
                TYPE_FX_LIBRARY => fxlib::slot_defs(),
                TYPE_TEXTURE => texture::slot_defs(),
                _ => [SlotDef::UNUSED; 12],
            }
        }

        /// Per-type patch fix-ups after load (`rev` = the patch's streaming
        /// revision).
        pub fn migrate(ty: u8, rev: i32, p: &mut [SlotValue; 12]) {
            match ty {
                $( $id => $m::migrate(rev, p), )*
                TYPE_FX_LIBRARY => fxlib::on_patch_load(p),
                TYPE_TEXTURE => texture::migrate(rev, p),
                _ => {}
            }
        }

        /// Ring-out length in blocks at `sr` (< 0 = always run).
        pub fn ringout_blocks_at(ty: u8, p: &[SlotValue; 12], sr: f32) -> i32 {
            match ty {
                $( $id => $m::ringout_blocks(p, sr), )*
                TYPE_FX_LIBRARY | TYPE_TEXTURE => -1,
                _ => 0,
            }
        }

        fn type_flag_defaults(ty: u8, p: &mut [SlotValue; 12]) {
            match ty {
                $( $id => $m::init_defaults(p), )*
                _ => {}
            }
        }
    };
}

fx_types! {
    1 => Delay(delay::Delay),
    2 => Reverb1(reverb1::Reverb1),
    3 => Phaser(phaser::Phaser),
    4 => Rotary(rotary::Rotary),
    5 => Distortion(distortion::Distortion),
    6 => Eq(eq::Eq),
    7 => FreqShift(freqshift::FreqShift),
    8 => Conditioner(conditioner::Conditioner),
    9 => Chorus(chorus::Chorus),
    10 => Vocoder(vocoder::Vocoder),
    11 => Reverb2(reverb2::Reverb2),
    12 => Flanger(flanger::Flanger),
    13 => RingMod(ringmod::RingMod),
    15 => Neuron(neuron::Neuron),
    16 => GraphicEq(geq::GraphicEq),
    17 => Resonator(resonator::Resonator),
    18 => Chow(chow::Chow),
    19 => Exciter(exciter::Exciter),
    20 => Ensemble(ensemble::Ensemble),
    21 => Combulator(combulator::Combulator),
    23 => Tape(tape::Tape),
    24 => Treemonster(treemonster::Treemonster),
    25 => Waveshaper(waveshaper::Waveshaper),
    26 => MsTool(mstool::MsTool),
    27 => SpringReverb(spring::SpringReverb),
    28 => Bonsai(bonsai::Bonsai),
    29 => AudioIn(audioin::AudioIn),
    30 => FloatyDelay(floaty::FloatyDelay),
    31 => Convolution(convolution::Convolution),
}

impl Fx {
    /// Ring-out length in blocks for this instance (Convolution depends on
    /// its loaded impulse response).
    pub fn ringout_blocks(&self, p: &[SlotValue; 12], sr: f32) -> i32 {
        match self {
            Fx::Convolution(c) => c.ringout(p),
            f => ringout_blocks_at(f.ty(), p, sr),
        }
    }

    /// Load or re-prepare the slot's user data (Convolution's impulse
    /// response). Allocates: call off the audio thread, after `new`, after a
    /// patch load and whenever `needs_prepare` turns true.
    pub fn prepare(&mut self, p: &[SlotValue; 12], sr: f32, ud: Option<&FxUserData>) {
        match self {
            Fx::Convolution(c) => c.prepare(p, sr, ud),
            // The sub-effect is built here, off the render path.
            Fx::FxLibrary(a) => a.prepare(p),
            _ => {}
        }
    }

    /// True when a non-modulatable change (Convolution's Size / Start /
    /// Reverse) wants `prepare` called.
    pub fn needs_prepare(&self) -> bool {
        match self {
            Fx::Convolution(c) => c.needs_prepare(),
            _ => false,
        }
    }
}

/// Ring-out length in blocks at 48 kHz (< 0 = always run).
pub fn ringout_blocks(ty: u8, p: &[SlotValue; 12]) -> i32 {
    ringout_blocks_at(ty, p, 48000.0)
}

/// The 12 values a type starts with when the user selects it (unused slots
/// int 0, flags at their defaults: e.g. default-deactivated cuts).
pub fn default_values(ty: u8) -> [SlotValue; 12] {
    let defs = slot_defs(ty);
    let mut p = [SlotValue::int(0); 12];
    for (v, d) in p.iter_mut().zip(defs.iter()) {
        if d.used() {
            *v = d.default_value();
        }
    }
    type_flag_defaults(ty, &mut p);
    p
}

/// The 12 slots an effect of type `ty` starts with when picked from the
/// menu (the reference's "Init" snapshot: interop values and flags over
/// [`default_values`]; `send`: a send slot, whose final mix is 1).
pub fn init_values(ty: u8, send: bool) -> [SlotValue; 12] {
    let mut p = default_values(ty);
    let vals: &[f32] = match ty {
        6 => &[0.0, -27.251785, 2.0, 0.0, 6.0, 2.0, 0.0, 54.964317, 2.223215],
        19 => &[0.5, 0.5, 0.5, 0.5, 0.5],
        16 => &[0.0; 11],
        17 => &[-18.6305, 0.75, 0.793701, 8.03822, 0.75, 0.793701, 35.9013, 0.75, 0.793701, 1.0, 0.0, 1.0],
        28 => &[0.0, -6.0, 0.25, 0.0, 0.0, 0.5, 0.5, 0.0, 0.25, 0.0, 1.0],
        5 => &[0.0, 3.81786, 1.944644, 38.421448, 0.0, 0.0, 0.0, 3.0, 2.03125, 30.535736],
        15 => &[0.0, 0.5, 0.5, 1.0, 0.0, 70.0, 0.5, 0.0, -2.0, 0.0, 0.0, 0.0],
        23 => &[0.85, 0.5, 0.5, 0.0, 30.0, 1.0, 0.1, 0.1, 0.0, 0.0, 0.0, 1.0],
        25 => &[-60.0, 70.0, 0.0, 0.0, 0.0, -60.0, 70.0, 0.0, 1.0],
        21 => &[0.0, -9.0, 0.25, -0.25, 0.75, 0.0, 0.793701, 0.793701, 0.793701, 0.25, -0.25, 1.0],
        7 => &[0.0, 1.0, -8.0, 0.0, 1.0],
        13 => &[0.0, 60.0, 0.2, 1.0, 0.3, 0.7, -60.0, 70.0, 1.0],
        24 => &[-24.0, 0.5, -60.0, 70.0, 0.0, 0.5, 1.0, 1.0],
        10 => &[0.0, -96.0, 0.0, 0.0, 0.0, 20.0, -15.4741, 49.0958, 0.0, 0.0, 0.0, 1.0],
        9 => &[-7.885717, -2.85, 0.669643, 0.0, -23.625008, 65.6625, 0.5],
        20 => &[45.2326448, -2.47393, 1.0, 2.46467, 0.6, 2.0, 40.0, 0.0, 0.0, 1.0, 1.0, 45.2326448],
        12 => &[1.0, 0.0, -2.0, 0.75, 3.0, 60.0, 12.0, 0.25, 0.2, 0.0, 1.0],
        3 => &[0.0, 0.5, 0.0, -2.0, 1.0, 0.5, 0.5, 0.0, 8.0, 1.0],
        4 => &[0.0, 0.0, 0.0, 0.7, 0.0, 0.0, 0.0, 1.0],
        31 => &[0.0, 1.0, 0.0, 0.0, 3.0, 0.0, -60.0, 70.0, 0.5],
        1 => &[-1.0, -1.0, 0.0, 0.0, -14.105346, 48.289276, -1.179463, 0.0, 0.0, 0.0, 0.25],
        30 => &[-1.73699, 1.0, 0.5, 20.0, 0.5, 0.0, 0.0, 0.0, 0.0, -60.0, 0.5],
        2 => &[-4.0, 2.0, 0.5, 1.0, 0.5, 3.0, 3.0, 0.0, 3.0, 0.5],
        11 => &[-3.0, 0.0, 1.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.5],
        27 => &[0.5, 0.5, 1.0, 0.5, 0.5, 0.0, 0.0, 0.5],
        14 => &[46.0, 1.0, 0.0, 1.0],
        29 => &[0.0, 0.0, -96.0, 0.0, 0.0, 0.0, 0.0, 0.0, -96.0, 1.0, 1.0],
        8 => &[0.0, 0.0, 1.0, 0.0, -6.0, 0.0, 0.0, 0.0, -60.0],
        26 => &[0.0, -60.0, 0.0, -6.63049, 70.0, -60.0, 0.0, 50.2131, 70.0, 0.0, 0.0, 0.0],
        22 => &[0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5],
        18 => &[-24.0, 10.0, 0.0, 1.0],
        _ => &[],
    };
    for (k, v) in vals.iter().enumerate() {
        p[k].v = *v;
    }
    // The final mix slot of the types with a send variant.
    let mix = match ty {
        9 => Some(6),
        3 => Some(6),
        31 => Some(8),
        1 => Some(10),
        30 => Some(10),
        2 => Some(9),
        11 => Some(9),
        27 => Some(7),
        _ => None,
    };
    if let (Some(m), true) = (mix, send) {
        p[m].v = 1.0;
    }
    let deact: &[usize] = match ty {
        25 => &[0, 1],
        31 => &[3, 4, 6, 7],
        8 => &[8],
        26 => &[1, 2, 4, 5, 6, 8],
        _ => &[],
    };
    for k in deact {
        p[*k].deactivated = true;
    }
    if ty == 1 {
        p[0].temposync = true;
        p[1].temposync = true;
    }
    if ty == 20 {
        p[11].deactivated = false;
    }
    p
}

/// Clamp every used slot into its type's range (after load, before
/// `migrate`). A slot whose extended range is on keeps its value (the
/// extended ranges of e.g. Combulator's absolute frequencies or Resonator's
/// widened bands are wider than the plain one).
pub fn clamp_values(ty: u8, p: &mut [SlotValue; 12]) {
    let defs = slot_defs(ty);
    for (v, d) in p.iter_mut().zip(defs.iter()) {
        if d.used() && !(d.can_extend && v.extend) {
            v.v = v.v.clamp(d.min, d.max);
        }
    }
}
