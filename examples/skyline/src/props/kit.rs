//! The props kit: the part types, the instance and part records the generators emit, the builders
//! they share (box, gable, beacon, the town kit, lights), and the part shader that draws them.
//!
//! A prop instance stands either in its own upright plane facing the camera `d` m away, or along a
//! straight line in the plan (a road, a power line, a bridge) seen in true perspective; its parts
//! are simple shapes in metres in that frame (the page's `PT_*` types). The page drew them with a
//! per-pixel interpreter over a packed texture; here every part is an instance of one part shader
//! whose vertex function places its bounding quad, and whose pixel function is the page's `ppDraw`
//! for that type.
//!
//! Owner after the foundation: stage C1. The records below follow the page's `add()` / `part()`
//! and may be reshaped by C1; the generators (stage C2) emit through [`Ctx`].
#![allow(dead_code)] // the records are emitted and drawn by stages C1 and C2
use crate::params::Params;
use crate::zones::Zones;
use makepad_widgets::makepad_draw::*;

/// The props' eye height above the ground (m), and metres per city block.
pub const EYE: f64 = 6.3;
pub const BLOCK: f64 = 70.0;

/// Part types (the page's `PT`; see its part comments for each type's `a`, `b`, `c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Pt {
    Seg = 0, Trap = 1, Ell = 2, Lat = 3, Ring = 4, Light = 5, Beacon = 6, Win = 7, Arc = 8, Band = 9, LRow = 10, Plume = 20, Gable = 21,
    Wash = 30, Box = 31, GProf = 32, Prof = 40, PBand = 41, Slr = 42, Fan = 43, Rep = 50, Clip = 51, Beam = 60, LBox = 61, CLight = 62,
}

/// One prop instance (the page's `add(o)`): where it stands, where it goes in the painter's order
/// and what may hide it. Its parts are `parts[p0 .. p0 + np]` of the [`PropSet`].
#[derive(Clone, Debug)]
pub struct Instance {
    /// Slot 0 turbines (on range `range`), 1 behind the city, 2 the props layer; view 0 the scenery
    /// view (off-centre warp), 1 the city camera.
    pub slot: u8, pub range: u8, pub view: u8,
    /// Hidden by nearer city surfaces / mountain ranges / the industry; is industry; a line frame (else a plane).
    pub hide_c: bool, pub hide_m: bool, pub hide_i: bool, pub is_ind: bool, pub line: bool,
    /// Plane: bearing, distance (m), foot elevation (rad; None: the flat ground's), half-width (m).
    pub uc: f64, pub d: f64, pub base: Option<f64>, pub hw: f64,
    /// Line: plan origin (m), direction angle, length (m), margin along it (m).
    pub o: [f64; 2], pub ang: f64, pub len: f64, pub smar: f64,
    /// Top (m); body tone distance (m, 0: per pixel); own seed; extra azimuth margin for glows (m); its parts.
    pub top: f64, pub td: f64, pub seed: u32, pub glow: f64, pub p0: usize, pub np: usize,
}

impl Default for Instance {
    fn default() -> Self {
        Self { slot: 2, range: 0, view: 0, hide_c: false, hide_m: false, hide_i: false, is_ind: false, line: false, uc: 0.0, d: 1.0, base: None,
            hw: 0.0, o: [0.0; 2], ang: 0.0, len: 0.0, smar: 0.0, top: 10.0, td: 0.0, seed: 0, glow: 0.0, p0: 0, np: 0 }
    }
}

/// One part: its type, `c` = (lateral offset of a line-frame part, s0, cv0), and `a`, `b`.
#[derive(Clone, Copy, Debug)]
pub struct Part { pub t: Pt, pub c: [f64; 3], pub a: [f64; 4], pub b: [f64; 4] }

/// Every prop of a parameter set.
#[derive(Default)]
pub struct PropSet { pub inst: Vec<Instance>, pub parts: Vec<Part> }

/// What a generator gets (the page's `ctx`): the zones, the current parameters and the emitters.
pub struct Ctx<'a> { pub z: &'a Zones, pub p: &'a Params, pub set: &'a mut PropSet }

impl Ctx<'_> {
    /// Starts an instance; the parts that follow are its own.
    pub fn add(&mut self, mut i: Instance) -> &mut Instance {
        if !i.line && i.base.is_none() { i.base = Some(-EYE / i.d); }
        (i.p0, i.np) = (self.set.parts.len(), 0);
        self.set.inst.push(i);
        self.set.inst.last_mut().unwrap()
    }
    /// A part of the current instance in its plane (`c` = (0, 0, 1)).
    pub fn part(&mut self, t: Pt, a: [f64; 4], b: [f64; 4]) { self.part_c(t, a, b, [0.0, 0.0, 1.0]); }
    /// A part with its line-frame placement `c`.
    pub fn part_c(&mut self, t: Pt, a: [f64; 4], b: [f64; 4], c: [f64; 3]) {
        self.set.parts.push(Part { t, c, a, b });
        if let Some(i) = self.set.inst.last_mut() { i.np += 1; }
    }
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    // One part per instance: a vertex function places its bounding quad, the pixel function is the
    // page's ppDraw for its type (body coverage to fb0 in the prop's tone, light to fb1).
    mod.draw.DrawSkyPart = mod.std.set_type_default() do #(DrawSkyPart::script_shader(vm)){
        ..mod.draw.DrawQuad,
        ..mod.draw.SkyScene,
        fragment: fn(){
            self.fb0 = vec4(0.0, 0.0, 0.0, 0.0)
            self.fb1 = vec4(0.0, 0.0, 0.0, 0.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawSkyPart { #[deref] pub draw_super: DrawQuad }
