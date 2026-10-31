//! The kit's effects rack SLOT: an ordered chain (`fx: [@delay @reverb]`)
//! over the summed voices, each effect once. The effects are the shared
//! family ([`super::fxs`], the same code the Tandem layout plays); this
//! module only maps each family type to schema parameters (one group
//! `fx_<kind>`, keys `<kind>_<slot>` in the slot's raw units) and runs the
//! chain. Every slot starts from the type's menu "Init" values and flags
//! ([`fxs::init_values`]); the patch sets the values, the flags stay. The
//! one effect outside the family is `comp`, the crate's bus compressor.

use super::core::{SlotValue, BLOCK};
use super::fxs::{self, Fx, FxCtx};
use super::schema::{Builder, Curve, Unit::{self, Db, Ms, Percent}, Values};
use crate::dynamics::Compressor;

/// Rack kinds (index = the patch's chain entry): name and family type id
/// (0 = the compressor).
pub const KINDS: &[(&str, u8)] = &[
    ("delay", 1),
    ("reverb", 2),
    ("phaser", 3),
    ("rotary", 4),
    ("distortion", 5),
    ("eq", 6),
    ("freq_shift", 7),
    ("conditioner", 8),
    ("chorus", 9),
    ("reverb2", 11),
    ("flanger", 12),
    ("ring_mod", 13),
    ("library", fxs::TYPE_FX_LIBRARY),
    ("neuron", 15),
    ("graphic_eq", 16),
    ("resonator", 17),
    ("truncator", 18),
    ("exciter", 19),
    ("ensemble", 20),
    ("combulator", 21),
    ("texture", fxs::TYPE_TEXTURE),
    ("tape", 23),
    ("treemonster", 24),
    ("waveshaper", 25),
    ("mid_side", 26),
    ("spring", 27),
    ("bonsai", 28),
    ("floaty", 30),
    ("convolution", 31),
    ("comp", 0),
];

/// A kind's index by name.
pub fn kind(name: &str) -> Option<u8> {
    KINDS.iter().position(|k| k.0 == name).map(|i| i as u8)
}

/// Every kind's name, space separated.
pub fn kind_names() -> String {
    KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(" ")
}

/// Where a kind's parameters sit: per family slot the schema index (None:
/// the slot is unused by the type).
#[derive(Clone, Debug, Default)]
pub struct KindIx {
    pub slots: [Option<usize>; 12],
}

#[derive(Clone, Debug, Default)]
pub struct FxIx {
    pub kinds: Vec<KindIx>,
    pub comp_threshold: usize,
    pub comp_ratio: usize,
    pub comp_attack: usize,
    pub comp_release: usize,
    pub comp_gain: usize,
    pub comp_mix: usize,
}

/// `Pre-Delay` -> `pre_delay`.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

pub fn declare(b: &mut Builder) -> FxIx {
    let mut f = FxIx::default();
    for (kind, ty) in KINDS {
        if *ty == 0 {
            f.kinds.push(KindIx::default());
            continue;
        }
        let name = fxs::TYPE_NAMES[*ty as usize];
        b.group(&format!("fx_{kind}"), leak(format!("{name} (the shared effects family; values in the slot's raw units)")));
        let init = fxs::init_values(*ty, false);
        let mut ix = KindIx::default();
        let mut taken: Vec<String> = Vec::new();
        for (k, d) in fxs::slot_defs(*ty).iter().enumerate() {
            if !d.used() {
                continue;
            }
            let mut s = slug(d.name);
            if s.is_empty() || taken.contains(&s) {
                s = format!("{s}{}p{k}", if s.is_empty() { "" } else { "_" });
            }
            taken.push(s.clone());
            // Choices by name (slugged, `@slow`); a long list (the library's 77 sub-effects) stays a number.
            let choices = d.int && !d.choices.is_empty() && d.choices.len() <= 16 && d.min == 0.0 && d.choices.len() == d.max as usize + 1;
            let (curve, unit) = d.unit.schema(d.int, choices);
            let curve = if d.int && d.min == 0.0 && d.max == 1.0 && !choices { Curve::Bool } else { curve };
            let about = if !d.about.is_empty() {
                d.about
            } else if d.int && d.choices.len() > 16 {
                leak(format!("{} = {}, {} = {}, ... (tandem_params fx_type {ty} names them all)", d.min, d.choices[0], d.min + 1.0, d.choices.get(1).copied().unwrap_or("")))
            } else {
                d.unit.about()
            };
            let default = init[k].v.clamp(d.min, d.max);
            let options: &'static [&'static str] = if choices { Box::leak(d.choices.iter().map(|c| leak(slug(c))).collect::<Vec<_>>().into_boxed_slice()) } else { &[] };
            ix.slots[k] = Some(b.add(format!("{kind}_{s}"), d.name, d.min, d.max, default, curve, unit, options, false, about));
        }
        f.kinds.push(ix);
    }
    b.group("fx_comp", "bus compressor");
    f.comp_threshold = b.num("comp_threshold".into(), "thresh", -60.0, 0.0, -18.0, Db, "");
    f.comp_ratio = b.log("comp_ratio".into(), "ratio", 1.0, 20.0, 4.0, Unit::None, "");
    f.comp_attack = b.log("comp_attack".into(), "attack", 0.1, 200.0, 5.0, Ms, "");
    f.comp_release = b.log("comp_release".into(), "release", 5.0, 2000.0, 120.0, Ms, "");
    f.comp_gain = b.num("comp_gain".into(), "gain", -12.0, 24.0, 0.0, Db, "");
    f.comp_mix = b.num("comp_mix".into(), "mix", 0.0, 1.0, 1.0, Percent, "");
    f
}

/// One family effect of the rack and the flags its slots keep.
struct Slot {
    kind: u8,
    fx: Fx,
    init: [SlotValue; 12],
    /// Blocks since the input was last present (ring-out).
    quiet: i32,
}

/// The rack's state: the chain's effects, made when the patch changes
/// ([`Rack::set_chain`], which allocates; [`Rack::process`] never does).
pub struct Rack {
    rate: f32,
    slots: Vec<Slot>,
    comp: Compressor,
    comp_key: (f32, f32, f32, f32),
}

fn family(kind: u8) -> u8 {
    KINDS.get(kind as usize).map_or(0, |k| k.1)
}

impl Rack {
    pub fn new(rate: f32) -> Rack {
        Rack { rate, slots: Vec::new(), comp: Compressor::new(rate, -18.0, 4.0, 0.005, 0.12), comp_key: (f32::NAN, 0.0, 0.0, 0.0) }
    }

    fn params(slot: &Slot, v: &(impl Values + ?Sized), x: &FxIx) -> [SlotValue; 12] {
        let mut p = slot.init;
        for (k, ix) in x.kinds[slot.kind as usize].slots.iter().enumerate() {
            if let Some(i) = ix {
                p[k].v = v.val(*i);
            }
        }
        p
    }

    /// Make the effects of `chain` (indices into [`KINDS`]) for the patch
    /// values `v`; effects the rack already runs keep their state.
    /// Allocates for new effects (and a new Library sub-effect).
    pub fn set_chain(&mut self, chain: &[u8], v: &(impl Values + ?Sized), x: &FxIx) {
        let mut slots = Vec::with_capacity(chain.len());
        for &kind in chain {
            let ty = family(kind);
            if ty == 0 {
                continue;
            }
            let slot = match self.slots.iter().position(|u| u.kind == kind) {
                Some(i) => self.slots.swap_remove(i),
                None => Slot { kind, fx: Fx::new(ty, self.rate), init: fxs::init_values(ty, false), quiet: 0 },
            };
            slots.push(slot);
        }
        for slot in &mut slots {
            let p = Self::params(slot, v, x);
            slot.fx.prepare(&p, self.rate, None);
        }
        self.slots = slots;
    }

    pub fn clear(&mut self) {
        self.slots.iter_mut().for_each(|u| u.fx.reset());
    }

    /// Run `chain` over one block in place (the effects come from the
    /// last [`Rack::set_chain`]; `beats_per_sec` is the tempo).
    pub fn process(&mut self, chain: &[u8], v: &(impl Values + ?Sized), x: &FxIx, beats_per_sec: f32, l: &mut [f32; BLOCK], r: &mut [f32; BLOCK]) {
        let mut ctx = FxCtx::new(self.rate);
        ctx.tsr = beats_per_sec * 0.5;
        let mut next = 0;
        for &kind in chain {
            match KINDS.get(kind as usize) {
                Some((_, 0)) => self.comp(v, x, l, r),
                Some(_) => {
                    let Some(slot) = self.slots.get_mut(next).filter(|u| u.kind == kind) else { continue };
                    next += 1;
                    let p = Self::params(slot, v, x);
                    let present = l.iter().chain(r.iter()).any(|s| s.abs() > 1e-9);
                    slot.quiet = if present { 0 } else { slot.quiet.saturating_add(1) };
                    let ring = slot.fx.ringout_blocks(&p, self.rate);
                    if ring < 0 || slot.quiet <= ring {
                        slot.fx.process(&p, &ctx, l, r);
                    } else {
                        slot.fx.process_only_control(&p, &ctx);
                    }
                }
                None => {}
            }
        }
    }
    fn comp(&mut self, v: &(impl Values + ?Sized), x: &FxIx, l: &mut [f32], r: &mut [f32]) {
        let key = (v.val(x.comp_threshold), v.val(x.comp_ratio), v.val(x.comp_attack), v.val(x.comp_release));
        if key != self.comp_key {
            self.comp = Compressor::new(self.rate, key.0, key.1, key.2 * 0.001, key.3 * 0.001);
            self.comp_key = key;
        }
        self.comp.makeup_db = v.val(x.comp_gain);
        let mix = v.val(x.comp_mix);
        for i in 0..l.len() {
            let (a, b) = self.comp.process(l[i], r[i]);
            l[i] += (a - l[i]) * mix;
            r[i] += (b - r[i]) * mix;
        }
    }
}
