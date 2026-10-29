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
