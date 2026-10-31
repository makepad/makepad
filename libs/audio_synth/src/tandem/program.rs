//! A patch compiled for playing: raw base values with their flags, the
//! three routing lists per the modulation rules (scene list, voice list,
//! global list), the oscillators' tables, the modulators' data, the FX
//! user data. Built off the audio thread; the engine swaps it in.

use super::catalog::{self, catalog, gl, sc, Catalog, Owner, N_FX, N_GLOBAL, N_LFO, N_PARAMS, N_SCENE};
use super::patch::{NonParam, Patch, PortaOpts};
use crate::kit::core::SlotValue;
use crate::kit::fxs::FxUserData;
use crate::kit::mods::{Mseg, StepSeq};
use crate::kit::oscs::RefTable;
use std::sync::Arc;

pub const N_SOURCES: usize = 41;

#[derive(Clone, Copy, Debug)]
pub struct Route {
    /// Scene-relative index (scene/voice lists) or global index (global list).
    pub dst: u16,
    pub src: u8,
    pub index: u8,
    pub depth: f32,
    pub src_scene: u8,
}

pub struct Program {
    pub values: Vec<SlotValue>,
    pub scene_routes: [Vec<Route>; 2],
    pub voice_routes: [Vec<Route>; 2],
    pub global_routes: Vec<Route>,
    /// Sources any routing of the scene (any list) reads.
    pub reads: [[bool; N_SOURCES]; 2],
    pub tables: [[Option<Arc<RefTable>>; 3]; 2],
    pub osc_extra: [[Vec<f32>; 3]; 2],
    pub stepseq: [[StepSeq; N_LFO]; 2],
    pub msegs: [[Mseg; N_LFO]; 2],
    pub has_formula: bool,
    /// Distinct formula scripts, compiled; which one each LFO plays.
    pub formulas: Vec<Arc<crate::kit::formula::Formula>>,
    pub formula_of: [[Option<u16>; N_LFO]; 2],
    /// What did not load (a formula that does not compile, ...), for the UI.
    pub problems: Vec<String>,
    pub extra_lfo_scaled: [[bool; N_LFO]; 2],
    pub macros: [(f32, bool); 8],
    pub porta: [PortaOpts; 2],
    pub nonparam: NonParam,
    pub correct_comb: bool,
    pub fx_user: Vec<Option<FxUserData>>,
    pub modwheel: [f32; 2],
    pub name: String,
    /// The patch's embedded tuning (scale + keyboard map), decoded.
    pub tuning: Option<Arc<crate::kit::tuning::Tuning>>,
}

impl Program {
    pub fn scene_value(&self, s: usize, i: usize) -> &SlotValue {
        &self.values[Catalog::scene_param(s, i)]
    }

    /// The scene's raw base values (N_SCENE of them).
    pub fn scene_base(&self, s: usize) -> impl Iterator<Item = f32> + '_ {
        let b = Catalog::scene_param(s, 0);
        self.values[b..b + N_SCENE].iter().map(|v| v.v)
    }

    pub fn global_base(&self) -> impl Iterator<Item = f32> + '_ {
        self.values[..N_GLOBAL].iter().map(|v| v.v)
    }

    pub fn compile(p: &Patch) -> Program {
        let c = catalog();
        let mut scene_routes: [Vec<Route>; 2] = Default::default();
        let mut voice_routes: [Vec<Route>; 2] = Default::default();
        let mut global_routes = Vec::new();
        let mut reads = [[false; N_SOURCES]; 2];
        for r in &p.routings {
            if r.muted || r.source as usize >= N_SOURCES || r.dest as usize >= N_PARAMS {
                continue;
            }
            let info = &c.params[r.dest as usize];
            let route = |dst: u16, src_scene: u8| Route { dst, src: r.source, index: r.source_index.clamp(0, 255) as u8, depth: r.depth, src_scene };
            match info.owner {
                Owner::Scene(s) | Owner::OscSlot { scene: s, .. } => {
                    let s = s as usize;
                    reads[s][r.source as usize] = true;
                    let rt = route(info.local, s as u8);
                    if catalog::source_is_scene_level(r.source) {
                        scene_routes[s].push(rt);
                    } else {
                        voice_routes[s].push(rt);
                    }
                }
                Owner::Global | Owner::FxSlot { .. } => {
                    let scene = if catalog::source_is_per_scene(r.source) { r.source_scene.min(1) } else { 0 };
                    reads[scene as usize][r.source as usize] = true;
                    global_routes.push(route(r.dest, scene));
                }
            }
        }
        let tables = std::array::from_fn(|s| {
            std::array::from_fn(|o| {
                let ty = p.osc_type(s, o);
                match &p.tables[s][o] {
                    Some(t) if t.flags & super::patch::WT_INT16 != 0 => Some(Arc::new(RefTable::from_i16(t.samples_per_frame, t.frames, t.flags, &t.data_i16))),
                    Some(t) => Some(Arc::new(RefTable::from_frames(t.samples_per_frame, t.frames, t.flags, &t.data_f32))),
                    None if ty == 2 || ty == 7 => Some(RefTable::shared_default().clone()),
                    None => None,
                }
            })
        });
        let stepseq = std::array::from_fn(|s| std::array::from_fn(|l| super::convert::stepseq(&p.stepseq[s][l])));
        let msegs = std::array::from_fn(|s| std::array::from_fn(|l| super::convert::mseg(p.msegs[s][l].as_ref(), l >= 6)));
        let has_formula = (0..2).any(|s| (0..N_LFO).any(|l| p.scene(s, sc::LFO + sc::LFO_STRIDE * l).i() == 9));
        let mut formulas = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        let mut formula_of = [[None; N_LFO]; 2];
        let mut problems = Vec::new();
        for s in 0..2 {
            for l in 0..N_LFO {
                if p.scene(s, sc::LFO + sc::LFO_STRIDE * l).i() != 9 {
                    continue;
                }
                let text = p.formulae[s][l].clone().unwrap_or_else(|| crate::kit::formula::DEFAULT_SCRIPT.to_string());
                let fi = match texts.iter().position(|t| *t == text) {
                    Some(i) => Some(i),
                    None => match crate::kit::formula::Formula::compile(&text) {
                        Ok(f) => {
                            texts.push(text);
                            formulas.push(f);
                            Some(formulas.len() - 1)
                        }
                        Err(e) => {
                            problems.push(format!("scene {} LFO {}: formula cannot load (line {}): {}", ["A", "B"][s], l + 1, e.line, e.message));
                            None
                        }
                    },
                };
                formula_of[s][l] = fi.map(|i| i as u16);
            }
        }
        let mut fx_user: Vec<Option<FxUserData>> = (0..N_FX).map(|_| None).collect();
        for (slot, entries) in &p.blocks.fx {
            if *slot < N_FX {
                fx_user[*slot] = Some(FxUserData { entries: entries.clone() });
            }
        }
        let _ = gl::FX;
        let tuning = p.tuning.as_ref().and_then(|t| {
            let text = |b64: &str| makepad_base64::base64_decode(b64.as_bytes()).ok().map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let scl = text(&t.scl_b64);
            if scl.trim().is_empty() {
                return None;
            }
            match crate::kit::tuning::Tuning::from_text(&t.mapping_name, &scl, &text(&t.kbm_b64)) {
                Ok(t) => Some(Arc::new(t)),
                Err(e) => {
                    problems.push(format!("the patch's tuning cannot load: {e}"));
                    None
                }
            }
        });
        Program {
            values: p.values.clone(),
            scene_routes,
            voice_routes,
            global_routes,
            reads,
            tables,
            osc_extra: std::array::from_fn(|s| std::array::from_fn(|o| p.osc_extra[s][o].extra.clone())),
            stepseq,
            msegs,
            has_formula,
            formulas,
            formula_of,
            problems,
            extra_lfo_scaled: std::array::from_fn(|s| std::array::from_fn(|l| p.extra_lfo[s][l] != 0)),
            macros: std::array::from_fn(|i| (p.macros[i].v, p.macros[i].bipolar)),
            porta: p.porta,
            nonparam: p.nonparam,
            correct_comb: p.correct_comb,
            fx_user,
            modwheel: p.modwheel,
            name: p.name.clone(),
            tuning,
        }
    }
}
