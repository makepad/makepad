//! Formula LFOs in a Tandem scene: one runner per distinct script the
//! scene's LFOs use (its globals, tables, random generator and error state
//! belong to the text, so every instance of it shares the runner), an
//! instance per LFO slot that plays it, and the block inputs the scripts
//! read.

use super::catalog::{sc, N_LFO};
use super::program::Program;
use crate::kit::formula::{FormulaInputs, Runner, VoiceInputs};
use crate::kit::mods::{Lfo, ModCtx};

/// Voices a scene can hold (instances are made for each).
const VOICES: usize = super::scene::MAX_VOICES;

#[derive(Default)]
pub struct FormulaBank {
    pub runners: Vec<Runner>,
    /// LFO slot (0..5 voice, 6..11 scene) -> its runner.
    pub of_lfo: [Option<usize>; N_LFO],
}

impl FormulaBank {
    /// The runners scene `s` of `prog` needs (allocates: when a program is
    /// installed, never per note).
    pub fn for_scene(prog: &Program, s: usize) -> FormulaBank {
        let mut bank = FormulaBank::default();
        let mut texts: Vec<usize> = Vec::new();
        for l in 0..N_LFO {
            if let Some(fi) = prog.formula_of[s][l] {
                let r = match texts.iter().position(|t| *t == fi as usize) {
                    Some(r) => r,
                    None => {
                        texts.push(fi as usize);
                        texts.len() - 1
                    }
                };
                bank.of_lfo[l] = Some(r);
            }
        }
        for (r, fi) in texts.iter().enumerate() {
            let voice_slots = (0..6).filter(|l| bank.of_lfo[*l] == Some(r)).count();
            let scene_slots = (6..N_LFO).filter(|l| bank.of_lfo[*l] == Some(r)).count();
            bank.runners.push(Runner::new(prog.formulas[*fi].clone(), VOICES * voice_slots + scene_slots));
        }
        bank
    }

    pub fn acquire(&mut self, lfo: usize) -> Option<usize> {
        let r = self.of_lfo[lfo]?;
        self.runners[r].acquire()
    }

    pub fn release(&mut self, lfo: usize, k: usize) {
        if let Some(r) = self.of_lfo[lfo] {
            self.runners[r].release(k);
        }
    }

    /// Start instance `k` of LFO `lfo` (its attack).
    pub fn start(&mut self, lfo: usize, k: usize, inputs: &FormulaInputs) {
        if let Some(r) = self.of_lfo[lfo] {
            self.runners[r].start(k, inputs);
        }
    }

    /// One block of instance `k`: its 16 sources and retrigger requests into
    /// the LFO (after its `process_block`).
    pub fn process(&mut self, lfo: usize, k: usize, inputs: &FormulaInputs, magnitude: f32, unipolar: bool, out: &mut Lfo) {
        let Some(r) = self.of_lfo[lfo] else { return };
        let o = self.runners[r].process(k, inputs);
        let eg = out.env_level();
        let a = magnitude.clamp(-3.0, 3.0);
        let mut v = [0.0f32; 16];
        o.outputs(eg, a, unipolar, &mut v);
        out.set_formula_outputs(&v, o.retrigger_aeg, o.retrigger_feg);
    }

    /// A run-time stop of LFO `lfo`'s script (shown on the modulator).
    pub fn error(&self, lfo: usize) -> Option<String> {
        let r = self.of_lfo[lfo]?;
        self.runners[r].error().map(|e| format!("formula stopped (line {}): {}", e.line, e.message))
    }
}

/// The inputs a script reads for LFO `l` this block (`eff`: its scene or voice values).
#[allow(clippy::too_many_arguments)]
pub fn inputs(prog: &Program, l: usize, eff: &[f32], lfo: &Lfo, ctx: &ModCtx, macros: [f32; 8], voice: Option<VoiceInputs>) -> FormulaInputs {
    let b = sc::LFO + sc::LFO_STRIDE * l;
    let t = |v: f32| ((v + 8.0) / 13.0).clamp(0.0, 1.0);
    let (phase, cycle) = lfo.phase();
    FormulaInputs {
        samplerate: ctx.fs as f64,
        block_size: crate::kit::core::BLOCK as f64,
        eg: [t(eff[b + 7]), t(eff[b + 8]), t(eff[b + 9]), t(eff[b + 10]), eff[b + 11].clamp(0.0, 1.0), t(eff[b + 12])],
        rate: eff[b + 1],
        startphase: eff[b + 2],
        deform: eff[b + 4],
        amplitude: eff[b + 3],
        tempo: ctx.tsr * 120.0,
        songpos: ctx.song_pos_beats,
        intphase: cycle as f64,
        phase: phase as f32,
        lfo_id: l as f64,
        poly_limit: prog.values[super::catalog::gl::POLYLIMIT].v as f64,
        scene_mode: prog.values[super::catalog::gl::SCENEMODE].v as f64,
        play_mode: eff[sc::POLYMODE] as f64,
        split_point: prog.values[super::catalog::gl::SPLITKEY].v as f64,
        macros,
        voice,
        ..FormulaInputs::default()
    }
}
