//! Formula LFOs in a Tandem engine: one runner per distinct script the
//! program's LFOs use, over both scenes (a script's globals, tables, random
//! generator and error state belong to its text, so every instance of it,
//! in either scene, shares the runner), an instance per LFO slot that plays
//! it, and the block inputs the scripts read.

use super::catalog::{sc, N_LFO};
use super::program::Program;
use crate::kit::formula::{FormulaInputs, Runner, VoiceInputs};
use crate::kit::mods::{Lfo, ModCtx};

/// Voices a scene can hold (instances are made for each).
const VOICES: usize = super::scene::MAX_VOICES;

#[derive(Default)]
pub struct FormulaBank {
    pub runners: Vec<Runner>,
    /// Scene, LFO slot (0..5 voice, 6..11 scene) -> its runner.
    pub of_lfo: [[Option<usize>; N_LFO]; 2],
}

impl FormulaBank {
    /// The runners `prog` needs (allocates: when a program is installed,
    /// never per note). The program already holds each distinct text once.
    pub fn for_program(prog: &Program) -> FormulaBank {
        let mut bank = FormulaBank::default();
        for s in 0..2 {
            for l in 0..N_LFO {
                bank.of_lfo[s][l] = prog.formula_of[s][l].map(|fi| fi as usize);
            }
        }
        for (fi, f) in prog.formulas.iter().enumerate() {
            let slots = |range: std::ops::Range<usize>| (0..2).map(|s| range.clone().filter(|l| bank.of_lfo[s][*l] == Some(fi)).count()).sum::<usize>();
            bank.runners.push(Runner::new(f.clone(), VOICES * slots(0..6) + slots(6..N_LFO)));
        }
        bank
    }

    pub fn acquire(&mut self, s: usize, lfo: usize) -> Option<usize> {
        let r = self.of_lfo[s][lfo]?;
        self.runners[r].acquire()
    }

    pub fn release(&mut self, s: usize, lfo: usize, k: usize) {
        if let Some(r) = self.of_lfo[s][lfo] {
            self.runners[r].release(k);
        }
    }

    /// Start instance `k` of scene `s`'s LFO `lfo` (its attack).
    pub fn start(&mut self, s: usize, lfo: usize, k: usize, inputs: &FormulaInputs) {
        if let Some(r) = self.of_lfo[s][lfo] {
            self.runners[r].start(k, inputs);
        }
    }

    /// One block of instance `k`: its 16 sources and retrigger requests into
    /// the LFO (after its `process_block`).
    #[allow(clippy::too_many_arguments)]
    pub fn process(&mut self, s: usize, lfo: usize, k: usize, inputs: &FormulaInputs, magnitude: f32, unipolar: bool, out: &mut Lfo) {
        let Some(r) = self.of_lfo[s][lfo] else { return };
        let o = self.runners[r].process(k, inputs);
        let eg = out.env_level();
        let a = magnitude.clamp(-3.0, 3.0);
        let mut v = [0.0f32; 16];
        o.outputs(eg, a, unipolar, &mut v);
        out.set_formula_outputs(&v, o.retrigger_aeg, o.retrigger_feg);
    }

    /// A run-time stop of scene `s`'s LFO `lfo` script (shown on the modulator).
    pub fn error(&self, s: usize, lfo: usize) -> Option<String> {
        let r = self.of_lfo[s][lfo]?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tandem::Patch;

    /// One script in both scenes: one runner, and its process globals are shared across the scenes.
    #[test]
    fn one_runner_per_script_across_scenes() {
        let text = "function process(state)\n    if count == nil then count = 0 end\n    count = count + 1\n    state.output = count / 100\n    return state\nend\n";
        let mut p = Patch::init();
        for s in ["a", "b"] {
            p.set(&format!("{s}_lfo6_shape"), 9.0);
        }
        p.formulae[0][6] = Some(text.to_string());
        p.formulae[1][6] = Some(text.to_string());
        let prog = Program::compile(&p);
        assert!(prog.problems.is_empty(), "{:?}", prog.problems);
        let mut bank = FormulaBank::for_program(&prog);
        assert_eq!(bank.runners.len(), 1);
        assert_eq!(bank.of_lfo[0][6], bank.of_lfo[1][6]);
        let (ka, kb) = (bank.acquire(0, 6).unwrap(), bank.acquire(1, 6).unwrap());
        assert_ne!(ka, kb);
        let inp = FormulaInputs { samplerate: 48000.0, block_size: 32.0, ..FormulaInputs::default() };
        bank.start(0, 6, ka, &inp);
        bank.start(1, 6, kb, &inp);
        let r = bank.of_lfo[0][6].unwrap();
        assert!((bank.runners[r].process(ka, &inp).outputs[0] - 0.01).abs() < 1e-6);
        // Scene B's instance sees the count scene A's left.
        let o = bank.runners[r].process(kb, &inp);
        assert!((o.outputs[0] - 0.02).abs() < 1e-6, "{:?}", o.outputs);
    }
}
