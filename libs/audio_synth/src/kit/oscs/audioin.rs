//! Audio Input (type 4): the host input (upsampled to fs_os) with channel
//! balance and gain, then low / high cut. The scene-B "Scene A" terms need
//! scene A's output, which the block does not carry yet; they are silent.

use super::sine::{high_cut, low_cut};
use super::{Cuts, OscBlock};
use crate::kit::core::{db_to_linear, SlotDef, SlotUnit, SlotValue};

pub fn slot_defs() -> [SlotDef; 7] {
    [
        SlotDef { about: "-1 left only .. +1 right only", ..SlotDef::float("Audio In Channel", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef::float("Audio In Gain", -48.0, 48.0, 0.0, SlotUnit::Db),
        SlotDef { about: "scene B only", ..SlotDef::float("Scene A Channel", -1.0, 1.0, 0.0, SlotUnit::Percent) },
        SlotDef { about: "scene B only", ..SlotDef::float("Scene A Gain", -48.0, 48.0, 0.0, SlotUnit::Db) },
        SlotDef { about: "scene B only", ..SlotDef::float("Scene A Mix", 0.0, 1.0, 0.0, SlotUnit::Percent) },
        low_cut(),
        high_cut(),
    ]
}

pub fn migrate(rev: i32, s: &mut [SlotValue; 7]) {
    if rev <= 12 {
        s[5].v = -60.0;
        s[5].deactivated = true;
        s[6].v = 70.0;
        s[6].deactivated = true;
    }
}

#[derive(Default)]
pub struct AudioIn {
    cuts: Cuts,
}

impl AudioIn {
    pub fn init(&mut self) {
        self.cuts.reset();
    }

    pub fn process(&mut self, b: &OscBlock, l: &mut [f32; 64], r: &mut [f32; 64]) {
        let g = db_to_linear(b.slots[1].v);
        let ch = b.slots[0].v.clamp(-1.0, 1.0);
        let (gl, gr) = (g * (1.0 - ch), g * (1.0 + ch));
        // Scene A terms: silent until the block carries scene A's output.
        let mix = 0.0f32;
        let mut ol = [0.0f32; 64];
        let mut or = [0.0f32; 64];
        if let Some((il, ir)) = b.audio_in {
            for k in 0..64 {
                ol[k] = (1.0 - mix) * gl * il[k];
                or[k] = (1.0 - mix) * gr * ir[k];
            }
        }
        if b.stereo {
            *l = ol;
            *r = or;
        } else {
            for k in 0..64 {
                l[k] = ol[k] + or[k];
            }
        }
        self.cuts.process(&b.slots[5], &b.slots[6], b.fs_os, b.stereo, l, r);
    }
}
