//! Patch data and effective values into the modulators' parameter structs.

use super::catalog::sc;
use super::patch::{MsegData, StepSeqData};
use super::program::Program;
use crate::kit::mods::{AdsrParams, LfoParams, LfoShape, Mseg, MsegSegment, SegmentType, StepSeq};

pub fn stepseq(d: &StepSeqData) -> StepSeq {
    StepSeq { steps: d.steps, loop_start: d.loop_start, loop_end: d.loop_end, shuffle: d.shuffle, trigmask: d.trigmask }
}

/// An LFO's MSEG (the default one for its kind when the patch has none).
pub fn mseg(d: Option<&MsegData>, is_scene: bool) -> Mseg {
    let Some(d) = d else { return Mseg::default_for(is_scene) };
    let mut m = Mseg::empty();
    m.segments = d
        .segments
        .iter()
        .map(|g| MsegSegment {
            duration: g.duration,
            v0: g.v0,
            nv1: g.nv1,
            cpduration: g.cpduration,
            cpv: g.cpv,
            ty: SegmentType::from_id(g.ty),
            use_deform: g.use_deform,
            invert_deform: g.invert_deform,
            retrigger_feg: g.retrigger_feg,
            retrigger_aeg: g.retrigger_aeg,
        })
        .collect();
    m.endpoint_locked = d.endpoint_mode == 1;
    m.lfo_mode = d.edit_mode == 1;
    m.loop_mode = crate::kit::mods::mseg::LoopMode::from_id(d.loop_mode);
    m.set_loop_markers(d.loop_start_point, d.loop_end_point, d.legacy_loop_start, d.legacy_loop_end);
    m.h_snap_default = d.hsnap_default;
    m.v_snap_default = d.vsnap_default;
    m.h_snap = d.hsnap;
    m.v_snap = d.vsnap;
    m.axis_width = d.axis_width;
    m.axis_start = d.axis_start;
    m.rebuild();
    m
}

/// Envelope `e` (0 amp, 1 filter) of scene `s` from effective values.
pub fn adsr(p: &Program, s: usize, e: usize, eff: &[f32]) -> AdsrParams {
    let b = sc::ENV + sc::ENV_STRIDE * e;
    let f = |k: usize| p.scene_value(s, b + k);
    AdsrParams {
        attack: eff[b],
        attack_shape: eff[b + 1].round() as i32,
        decay: eff[b + 2],
        decay_shape: eff[b + 3].round() as i32,
        sustain: eff[b + 4],
        release: eff[b + 5],
        release_shape: eff[b + 6].round() as i32,
        mode: eff[b + 7].round() as i32,
        gated_release: e == 0 && f(5).deform != 0,
        attack_sync: f(0).temposync,
        decay_sync: f(2).temposync,
        release_sync: f(5).temposync,
    }
}

/// LFO `l` (0..5 voice, 6..11 scene) of scene `s` from effective values.
pub fn lfo(p: &Program, s: usize, l: usize, eff: &[f32]) -> LfoParams {
    let b = sc::LFO + sc::LFO_STRIDE * l;
    let f = |k: usize| p.scene_value(s, b + k);
    LfoParams {
        shape: LfoShape::from_id(eff[b].round() as i32),
        rate: eff[b + 1],
        rate_sync: f(1).temposync,
        rate_deactivated: f(1).deactivated,
        phase: eff[b + 2],
        phase_extend: f(2).extend,
        magnitude: eff[b + 3],
        magnitude_extend: f(3).extend,
        deform: eff[b + 4],
        deform_type: f(4).deform,
        trigmode: eff[b + 5].round() as i32,
        retrigger_every_note: f(5).deform == 1,
        unipolar: eff[b + 6] > 0.5,
        delay: eff[b + 7],
        delay_sync: f(7).temposync,
        delay_deactivated: f(7).deactivated,
        attack: eff[b + 8],
        attack_sync: f(8).temposync,
        hold: eff[b + 9],
        hold_sync: f(9).temposync,
        decay: eff[b + 10],
        decay_sync: f(10).temposync,
        sustain: eff[b + 11],
        release: eff[b + 12],
        release_sync: f(12).temposync,
        release_unmodulated: f(12).v,
        extra_amplitude: p.extra_lfo_scaled[s][l],
    }
}
