//! The Tandem parameter catalogue: every parameter of a patch in canonical
//! order (219 global, 273 per scene x 2, the oscillator character last),
//! with its storage name, native range, default and which per-value flags
//! its kind carries. Built from rules, not tabulated by hand.
//!
//! Global index = position in [`Catalog::params`]; scene parameters also
//! have their scene-relative index (the modulation destination id within a
//! scene).

use crate::kit::core::SlotUnit;
use std::collections::HashMap;
use std::sync::OnceLock;

pub const N_GLOBAL: usize = 219;
pub const N_SCENE: usize = 273;
pub const N_PARAMS: usize = N_GLOBAL + 2 * N_SCENE + 1;
pub const N_FX: usize = 16;
pub const N_LFO: usize = 12;

/// What part of the patch a parameter belongs to (and so how its kind is
/// decided after the XML is read).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Global,
    Scene(u8),
    /// An oscillator's param0..6 slot: kind decided by the osc type.
    OscSlot { scene: u8, osc: u8, slot: u8 },
    /// An FX slot's p0..p11: kind decided by the FX type.
    FxSlot { fx: u8, slot: u8 },
}

#[derive(Clone, Debug)]
pub struct ParamInfo {
    /// Storage (XML element) name: `a_filter1_cutoff`, `fx3_p4`, `volume`.
    pub name: String,
    /// Scene-relative name (`filter1_cutoff`) or the global name.
    pub short: &'static str,
    pub owner: Owner,
    /// Index within the scene (scene parameters), else the global index.
    pub local: u16,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub int: bool,
    pub unit: SlotUnit,
    pub can_extend: bool,
    pub can_deactivate: bool,
    /// Default for an absent `deactivated` attribute (rule 6).
    pub default_deactivated: bool,
    pub can_temposync: bool,
    pub can_absolute: bool,
    pub has_deform: bool,
    pub about: &'static str,
}

pub struct Catalog {
    pub params: Vec<ParamInfo>,
    by_name: HashMap<String, usize>,
}

impl Catalog {
    pub fn index(&self, name: &str) -> Option<usize> {
        self.by_name.get(name).copied()
    }
    /// Global index of scene `s`'s parameter at scene index `i`.
    #[inline]
    pub fn scene_param(s: usize, i: usize) -> usize {
        N_GLOBAL + s * N_SCENE + i
    }
    pub const CHARACTER: usize = N_GLOBAL + 2 * N_SCENE;
}

pub fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(build)
}

/// Scene-relative indices (interop).
pub mod sc {
    pub const OCTAVE: usize = 0;
    pub const PITCH: usize = 1;
    pub const PORTAMENTO: usize = 2;
    /// osc o: type at OSC + 12 o, then octave, pitch, param0..6, keytrack, retrigger.
    pub const OSC: usize = 3;
    pub const OSC_STRIDE: usize = 12;
    pub const POLYMODE: usize = 39;
    pub const FM_SWITCH: usize = 40;
    pub const FM_DEPTH: usize = 41;
    pub const DRIFT: usize = 42;
    pub const NOISECOL: usize = 43;
    pub const KTRKROOT: usize = 44;
    pub const VOLUME: usize = 45;
    pub const PAN: usize = 46;
    pub const WIDTH: usize = 47;
    pub const SEND: usize = 48;
    /// mixer channel c (osc1 osc2 osc3 ring12 ring23 noise): level, mute, solo, route.
    pub const MIX: usize = 52;
    pub const LEVEL_PFG: usize = 76;
    pub const PBRANGE_UP: usize = 77;
    pub const PBRANGE_DN: usize = 78;
    pub const VCA_LEVEL: usize = 79;
    pub const VCA_VELSENSE: usize = 80;
    pub const FEEDBACK: usize = 81;
    pub const FB_CONFIG: usize = 82;
    pub const F_BALANCE: usize = 83;
    pub const LOWCUT: usize = 84;
    pub const WS_TYPE: usize = 85;
    pub const WS_DRIVE: usize = 86;
    pub const F1_TYPE: usize = 87;
    pub const F1_SUBTYPE: usize = 88;
    pub const F1_CUTOFF: usize = 89;
    pub const F1_RESO: usize = 90;
    pub const F1_ENVMOD: usize = 91;
    pub const F1_KEYTRACK: usize = 92;
    pub const F2_TYPE: usize = 93;
    pub const F2_SUBTYPE: usize = 94;
    pub const F2_CUTOFF: usize = 95;
    pub const F2_IS_OFFSET: usize = 96;
    pub const F2_RESO: usize = 97;
    pub const F2_LINK_RESO: usize = 98;
    pub const F2_ENVMOD: usize = 99;
    pub const F2_KEYTRACK: usize = 100;
    /// env e (0 amp, 1 filter): attack, attack_shape, decay, decay_shape, sustain, release, release_shape, mode.
    pub const ENV: usize = 101;
    pub const ENV_STRIDE: usize = 8;
    /// lfo l (0..5 voice, 6..11 scene): shape rate phase magnitude deform trigmode unipolar delay attack hold decay sustain release.
    pub const LFO: usize = 117;
    pub const LFO_STRIDE: usize = 13;
}

/// Global indices (interop).
pub mod gl {
    pub const RETURN: usize = 0;
    pub const VOLUME: usize = 4;
    pub const SCENE_ACTIVE: usize = 5;
    pub const SCENEMODE: usize = 6;
    pub const SPLITKEY: usize = 7;
    pub const FX_DISABLE: usize = 8;
    pub const POLYLIMIT: usize = 9;
    pub const FX_BYPASS: usize = 10;
    /// slot s: type at FX + 13 s, then p0..p11.
    pub const FX: usize = 11;
    pub const FX_STRIDE: usize = 13;
}

pub const MIX_NAMES: [&str; 6] = ["o1", "o2", "o3", "ring12", "ring23", "noise"];

struct B {
    params: Vec<ParamInfo>,
}

#[derive(Clone, Copy)]
struct K {
    min: f32,
    max: f32,
    default: f32,
    int: bool,
    unit: SlotUnit,
    ext: bool,
    deact: Option<bool>,
    sync: bool,
    abs: bool,
    deform: bool,
}

const fn f(min: f32, max: f32, default: f32, unit: SlotUnit) -> K {
    K { min, max, default, int: false, unit, ext: false, deact: None, sync: false, abs: false, deform: false }
}
const fn i(min: f32, max: f32, default: f32) -> K {
    K { min, max, default, int: true, unit: SlotUnit::None, ext: false, deact: None, sync: false, abs: false, deform: false }
}
impl K {
    const fn ext(mut self) -> K {
        self.ext = true;
        self
    }
    /// Deactivatable, with the default for an absent attribute.
    const fn deact(mut self, default: bool) -> K {
        self.deact = Some(default);
        self
    }
    const fn sync(mut self) -> K {
        self.sync = true;
        self
    }
    const fn abs(mut self) -> K {
        self.abs = true;
        self
    }
    const fn deform(mut self) -> K {
        self.deform = true;
        self
    }
}

use SlotUnit as U;
const PERCENT: K = f(0.0, 1.0, 0.0, U::Percent);
const BIPOLAR: K = f(-1.0, 1.0, 0.0, U::Percent);
const BOOL: K = i(0.0, 1.0, 0.0);
const ENVTIME: K = f(-8.0, 5.0, 0.0, U::Log2Secs).sync();
const AMP: K = f(0.0, 1.0, 1.0, U::Amp);
const CUTOFF: K = f(-60.0, 70.0, 3.0, U::NoteA440).ext();
const SEMI7: K = f(-7.0, 7.0, 0.0, U::Semis).ext();
const DB48: K = f(-48.0, 48.0, 0.0, U::Db);

impl B {
    fn add(&mut self, name: String, short: &'static str, owner: Owner, local: usize, k: K, about: &'static str) {
        self.params.push(ParamInfo {
            name,
            short,
            owner,
            local: local as u16,
            min: k.min,
            max: k.max,
            default: k.default,
            int: k.int,
            unit: k.unit,
            can_extend: k.ext,
            can_deactivate: k.deact.is_some(),
            default_deactivated: k.deact.unwrap_or(false),
            can_temposync: k.sync,
            can_absolute: k.abs,
            has_deform: k.deform,
            about,
        });
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn build() -> Catalog {
    let mut b = B { params: Vec::with_capacity(N_PARAMS) };
    // ---- global ----
    for n in 0..4 {
        let name = format!("volume_FX{}", n + 1);
        b.add(name.clone(), leak(name), Owner::Global, n, AMP, "return level of send FX");
    }
    b.add("volume".into(), "volume", Owner::Global, 4, f(-48.0, 0.0, 0.0, U::Db), "global volume");
    b.add("scene_active".into(), "scene_active", Owner::Global, 5, i(0.0, 1.0, 0.0), "scene shown in the editor");
    b.add("scenemode".into(), "scenemode", Owner::Global, 6, i(0.0, 3.0, 0.0), "0 single, 1 key split, 2 dual, 3 channel split");
    b.add("splitkey".into(), "splitkey", Owner::Global, 7, i(0.0, 127.0, 60.0), "split key / split channel");
    b.add("fx_disable".into(), "fx_disable", Owner::Global, 8, i(0.0, 65535.0, 0.0), "bit s disables FX slot s");
    b.add("polylimit".into(), "polylimit", Owner::Global, 9, i(2.0, 64.0, 16.0), "voices per scene");
    b.add("fx_bypass".into(), "fx_bypass", Owner::Global, 10, i(0.0, 3.0, 0.0), "0 all FX, 1 no send, 2 inserts only, 3 none");
    for s in 0..N_FX {
        let base = gl::FX + gl::FX_STRIDE * s;
        let name = format!("fx{}_type", s + 1);
        b.add(name.clone(), leak(name), Owner::Global, base, i(0.0, 31.0, 0.0), "FX type");
        for k in 0..12 {
            let name = format!("fx{}_p{}", s + 1, k);
            // Kind decided by the FX type after reading.
            b.add(name.clone(), leak(name), Owner::FxSlot { fx: s as u8, slot: k as u8 }, base + 1 + k, f(0.0, 1.0, 0.0, U::None), "");
        }
    }
    debug_assert_eq!(b.params.len(), N_GLOBAL);
    // ---- scenes ----
    for s in 0..2u8 {
        let pre = if s == 0 { "a_" } else { "b_" };
        let add = |b: &mut B, short: String, local: usize, k: K, about: &'static str| {
            debug_assert_eq!(b.params.len(), N_GLOBAL + s as usize * N_SCENE + local, "{short}");
            b.add(format!("{pre}{short}"), leak(short), Owner::Scene(s), local, k, about);
        };
        add(&mut b, "octave".into(), sc::OCTAVE, i(-3.0, 3.0, 0.0), "scene octave");
        add(&mut b, "pitch".into(), sc::PITCH, SEMI7, "scene pitch, semitones (extended x12)");
        add(&mut b, "portamento".into(), sc::PORTAMENTO, f(-8.0, 2.0, -8.0, U::Log2Secs).sync(), "glide time (-8 = off)");
        for o in 0..3 {
            let base = sc::OSC + sc::OSC_STRIDE * o;
            add(&mut b, format!("osc{}_type", o + 1), base, i(0.0, 11.0, 0.0), "oscillator type");
            add(&mut b, format!("osc{}_octave", o + 1), base + 1, i(-3.0, 3.0, 0.0), "");
            add(&mut b, format!("osc{}_pitch", o + 1), base + 2, SEMI7.abs(), "semitones (extended x12; absolute: 10 Hz per unit)");
            for k in 0..7 {
                let short = format!("osc{}_param{}", o + 1, k);
                let local = base + 3 + k;
                debug_assert_eq!(b.params.len(), N_GLOBAL + s as usize * N_SCENE + local);
                b.add(format!("{pre}{short}"), leak(short), Owner::OscSlot { scene: s, osc: o as u8, slot: k as u8 }, local, f(0.0, 1.0, 0.0, U::None), "");
            }
            add(&mut b, format!("osc{}_keytrack", o + 1), base + 10, i(0.0, 1.0, 1.0), "");
            add(&mut b, format!("osc{}_retrigger", o + 1), base + 11, BOOL, "");
        }
        add(&mut b, "polymode".into(), sc::POLYMODE, i(0.0, 5.0, 0.0), "0 poly, 1 mono, 2 mono single trigger, 3 mono fingered porta, 4 mono ST + FP, 5 latch");
        add(&mut b, "fm_switch".into(), sc::FM_SWITCH, i(0.0, 3.0, 0.0), "0 off, 1 2>1, 2 3>2>1, 3 2>1<3");
        add(&mut b, "fm_depth".into(), sc::FM_DEPTH, f(-48.0, 24.0, -24.0, U::Db), "FM depth");
        add(&mut b, "drift".into(), sc::DRIFT, PERCENT.ext(), "oscillator drift");
        add(&mut b, "noisecol".into(), sc::NOISECOL, BIPOLAR.deform(), "noise colour");
        add(&mut b, "ktrkroot".into(), sc::KTRKROOT, i(0.0, 127.0, 60.0), "keytrack root key");
        add(&mut b, "volume".into(), sc::VOLUME, f(0.0, 1.0, 0.890899, U::Amp).deact(false), "scene volume");
        add(&mut b, "pan".into(), sc::PAN, BIPOLAR, "");
        add(&mut b, "pan2".into(), sc::WIDTH, f(-1.0, 1.0, 1.0, U::Percent), "width");
        for k in 0..4 {
            add(&mut b, format!("send_fx_{}", k + 1), sc::SEND + k, f(0.0, 1.5874011, 0.0, U::Amp), "send level");
        }
        for (c, m) in MIX_NAMES.iter().enumerate() {
            let base = sc::MIX + 4 * c;
            // Ring levels carry the combinator mode, the noise level none (its colour has the deform).
            let level = if c == 3 || c == 4 { AMP.deform() } else { AMP };
            add(&mut b, format!("level_{m}"), base, level, "");
            add(&mut b, format!("mute_{m}"), base + 1, i(0.0, 1.0, if c == 0 { 0.0 } else { 1.0 }), "");
            add(&mut b, format!("solo_{m}"), base + 2, BOOL, "");
            add(&mut b, format!("route_{m}"), base + 3, i(0.0, 2.0, 1.0), "0 filter 1, 1 both, 2 filter 2");
        }
        add(&mut b, "level_pfg".into(), sc::LEVEL_PFG, DB48, "pre-filter gain");
        add(&mut b, "pbrange_up".into(), sc::PBRANGE_UP, i(0.0, 24.0, 2.0).ext(), "bend up range (extended: cents)");
        add(&mut b, "pbrange_dn".into(), sc::PBRANGE_DN, i(0.0, 24.0, 2.0).ext(), "bend down range");
        add(&mut b, "vca_level".into(), sc::VCA_LEVEL, DB48, "VCA gain");
        add(&mut b, "vca_velsense".into(), sc::VCA_VELSENSE, f(-48.0, 0.0, 0.0, U::Db), "velocity to gain");
        add(&mut b, "feedback".into(), sc::FEEDBACK, BIPOLAR.ext().deform(), "filter block feedback");
        add(&mut b, "fb_config".into(), sc::FB_CONFIG, i(0.0, 7.0, 0.0), "0 serial1 1 serial2 2 serial3 3 dual1 4 dual2 5 stereo 6 ring 7 wide");
        add(&mut b, "f_balance".into(), sc::F_BALANCE, BIPOLAR, "filter balance");
        add(&mut b, "lowcut".into(), sc::LOWCUT, f(-72.0, 15.0, -72.0, U::NoteA440).deact(false).deform(), "scene highpass (deform: slope)");
        add(&mut b, "ws_type".into(), sc::WS_TYPE, i(0.0, 42.0, 0.0).deact(false), "waveshaper");
        add(&mut b, "ws_drive".into(), sc::WS_DRIVE, f(-24.0, 24.0, 0.0, U::Db).ext(), "waveshaper drive");
        for u in 0..2 {
            let n = u + 1;
            let base = if u == 0 { sc::F1_TYPE } else { sc::F2_TYPE };
            add(&mut b, format!("filter{n}_type"), base, i(0.0, 35.0, 0.0).deact(false), "filter type");
            add(&mut b, format!("filter{n}_subtype"), base + 1, i(0.0, 15.0, 0.0), "");
            add(&mut b, format!("filter{n}_cutoff"), base + 2, CUTOFF, "cutoff");
            if u == 0 {
                add(&mut b, "filter1_resonance".into(), base + 3, PERCENT, "");
                add(&mut b, "filter1_envmod".into(), base + 4, f(-96.0, 96.0, 0.0, U::Semis), "filter EG amount");
                add(&mut b, "filter1_keytrack".into(), base + 5, BIPOLAR, "");
            } else {
                add(&mut b, "f2_cf_is_offset".into(), base + 3, BOOL, "filter 2 cutoff relative to filter 1");
                add(&mut b, "filter2_resonance".into(), base + 4, PERCENT, "");
                add(&mut b, "f2_link_resonance".into(), base + 5, BOOL, "");
                add(&mut b, "filter2_envmod".into(), base + 6, f(-96.0, 96.0, 0.0, U::Semis), "");
                add(&mut b, "filter2_keytrack".into(), base + 7, BIPOLAR, "");
            }
        }
        for e in 0..2 {
            let base = sc::ENV + sc::ENV_STRIDE * e;
            let n = e + 1;
            add(&mut b, format!("env{n}_attack"), base, f(-8.0, 5.0, -8.0, U::Log2Secs).sync(), "");
            add(&mut b, format!("env{n}_attack_shape"), base + 1, i(0.0, 2.0, 1.0), "");
            add(&mut b, format!("env{n}_decay"), base + 2, f(-8.0, 5.0, -2.0, U::Log2Secs).sync(), "");
            add(&mut b, format!("env{n}_decay_shape"), base + 3, i(0.0, 2.0, 1.0), "");
            add(&mut b, format!("env{n}_sustain"), base + 4, f(0.0, 1.0, 1.0, U::Percent), "");
            let rel = f(-8.0, 5.0, -5.0, U::Log2Secs).sync();
            add(&mut b, format!("env{n}_release"), base + 5, rel.deform(), "deform: gated release");
            add(&mut b, format!("env{n}_release_shape"), base + 6, i(0.0, 2.0, 2.0), "");
            add(&mut b, format!("env{n}_mode"), base + 7, i(0.0, 1.0, 0.0), "0 digital, 1 analog");
        }
        for l in 0..N_LFO {
            let base = sc::LFO + sc::LFO_STRIDE * l;
            add(&mut b, format!("lfo{l}_shape"), base, i(0.0, 9.0, 0.0), "0 sine 1 tri 2 square 3 saw 4 noise 5 S&H 6 envelope 7 step seq 8 MSEG 9 formula");
            add(&mut b, format!("lfo{l}_rate"), base + 1, f(-7.0, 9.0, 0.0, U::Log2Hz).sync().deact(false), "");
            add(&mut b, format!("lfo{l}_phase"), base + 2, PERCENT.ext(), "start phase / shuffle");
            add(&mut b, format!("lfo{l}_magnitude"), base + 3, f(0.0, 1.0, 1.0, U::Percent).ext(), "amplitude");
            add(&mut b, format!("lfo{l}_deform"), base + 4, BIPOLAR.deform(), "");
            add(&mut b, format!("lfo{l}_trigmode"), base + 5, i(0.0, 2.0, 1.0).deform(), "0 freerun 1 keytrigger 2 random");
            add(&mut b, format!("lfo{l}_unipolar"), base + 6, BOOL, "");
            add(&mut b, format!("lfo{l}_delay"), base + 7, f(-8.0, 5.0, -8.0, U::Log2Secs).sync().deact(false), "");
            add(&mut b, format!("lfo{l}_attack"), base + 8, f(-8.0, 5.0, -8.0, U::Log2Secs).sync(), "");
            add(&mut b, format!("lfo{l}_hold"), base + 9, f(-8.0, 5.0, -8.0, U::Log2Secs).sync(), "");
            add(&mut b, format!("lfo{l}_decay"), base + 10, ENVTIME, "");
            add(&mut b, format!("lfo{l}_sustain"), base + 11, f(0.0, 1.0, 1.0, U::Percent), "");
            add(&mut b, format!("lfo{l}_release"), base + 12, f(-8.0, 5.0, 5.0, U::Log2Secs).sync(), "5 = forever");
        }
    }
    b.add("character".into(), "character", Owner::Global, N_GLOBAL + 2 * N_SCENE, i(0.0, 2.0, 1.0), "0 warm 1 neutral 2 bright");
    assert_eq!(b.params.len(), N_PARAMS);
    let by_name = b.params.iter().enumerate().map(|(i, p)| (p.name.clone(), i)).collect();
    Catalog { params: b.params, by_name }
}

/// Modulation sources (interop ids).
pub const SOURCE_NAMES: [&str; 41] = [
    "off", "vel", "keytrk", "pat", "at", "pb", "mw", "macro_1", "macro_2", "macro_3", "macro_4", "macro_5", "macro_6", "macro_7", "macro_8", "aeg", "feg", "vlfo_1",
    "vlfo_2", "vlfo_3", "vlfo_4", "vlfo_5", "vlfo_6", "slfo_1", "slfo_2", "slfo_3", "slfo_4", "slfo_5", "slfo_6", "timbre", "rel_vel", "rand_bi", "rand_uni",
    "alt_bi", "alt_uni", "breath", "expr", "sus", "lowest_key", "highest_key", "latest_key",
];

/// Scene-level sources go to a scene's scene list.
pub fn source_is_scene_level(src: u8) -> bool {
    matches!(src, 0 | 4 | 5 | 6 | 7..=14 | 23..=28 | 35..=40)
}

/// Sources that exist once per scene (they carry `source_scene` on global routings).
pub fn source_is_per_scene(src: u8) -> bool {
    matches!(src, 15..=28 | 38..=40)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_interop_indices() {
        let c = catalog();
        assert_eq!(c.params.len(), N_PARAMS);
        assert_eq!(c.index("fx1_type"), Some(11));
        assert_eq!(c.index("fx16_p11"), Some(218));
        assert_eq!(c.index("a_octave"), Some(N_GLOBAL));
        assert_eq!(c.params[Catalog::scene_param(0, sc::POLYMODE)].name, "a_polymode");
        assert_eq!(c.params[Catalog::scene_param(1, sc::LOWCUT)].name, "b_lowcut");
        assert_eq!(c.params[Catalog::scene_param(0, sc::F2_KEYTRACK)].name, "a_filter2_keytrack");
        assert_eq!(c.params[Catalog::scene_param(0, sc::ENV + 8)].name, "a_env2_attack");
        assert_eq!(c.params[Catalog::scene_param(0, sc::LFO)].name, "a_lfo0_shape");
        assert_eq!(c.params[Catalog::scene_param(1, N_SCENE - 1)].name, "b_lfo11_release");
        assert_eq!(c.params[Catalog::CHARACTER].name, "character");
        assert_eq!(c.params[Catalog::scene_param(0, sc::OSC + 12 + 3)].name, "a_osc2_param0");
        assert_eq!(c.params[Catalog::scene_param(0, sc::MIX + 4 * 5)].name, "a_level_noise");
    }
}
