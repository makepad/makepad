//! Audio input mixer (type 29): sums the slot's own input, the host audio
//! input and (in scene insert slots) the other scene's output, each with a
//! channel selector, a pan and a level; then width and mix.

use super::common::*;
use super::FxCtx;
use crate::kit::core::{db_to_linear, SlotDef, SlotValue};

pub fn slot_defs() -> [SlotDef; 12] {
    [
        bipolar("Audio Input Channel", 0.0),
        bipolar("Audio Input Pan", 0.0),
        db("Audio Input Level", -96.0, 0.0, -96.0),
        bipolar("Effect Input Channel", 0.0),
        bipolar("Effect Input Pan", 0.0),
        db("Effect Input Level", -96.0, 0.0, 0.0),
        bipolar("Scene Input Channel", 0.0),
        bipolar("Scene Input Pan", 0.0),
        db("Scene Input Level", -96.0, 0.0, -96.0),
        bipolar("Width", 1.0),
        percent("Mix", 1.0),
        unused(),
    ]
}

pub fn init_defaults(_p: &mut [SlotValue; 12]) {}

pub fn migrate(_rev: i32, _p: &mut [SlotValue; 12]) {}

pub fn ringout_blocks(_p: &[SlotValue; 12], _sr: f32) -> i32 {
    -1
}

/// One source's conditioning: channel gains, pan, level (all ramped).
#[derive(Clone, Copy, Default)]
struct Source {
    gl: Line,
    gr: Line,
    ll: Line,
    rr: Line,
    lvl: Line,
}

impl Source {
    fn set(&mut self, chan: f32, pan: f32, level_db: f32) {
        let (gl, gr) = if chan < 0.0 { (1.0, 1.0 + chan) } else { (1.0 - chan, 1.0) };
        self.gl.set_target(gl);
        self.gr.set_target(gr);
        self.ll.set_target(if pan < 0.0 { 1.0 } else { 1.0 - pan });
        self.rr.set_target(if pan > 0.0 { 1.0 } else { 1.0 + pan });
        self.lvl.set_target(db_to_linear(level_db));
    }
    /// Accumulate the conditioned source into (wl, wr).
    fn add(&self, sl: &[f32; 32], sr: &[f32; 32], wl: &mut Buf, wr: &mut Buf) {
        for k in 0..32 {
            let l = sl[k] * self.gl.at(k);
            let r = sr[k] * self.gr.at(k);
            let (ll, rr) = (self.ll.at(k), self.rr.at(k));
            let g = self.lvl.at(k);
            wl[k] += (ll * l + (1.0 - rr) * r) * g;
            wr[k] += (rr * r + (1.0 - ll) * l) * g;
        }
    }
}

pub struct AudioIn {
    audio: Source,
    effect: Source,
    scene: Source,
    width: Line,
    mix: Line,
}

impl AudioIn {
    pub fn new(_sr: f32) -> AudioIn {
        AudioIn {
            audio: Source::default(),
            effect: Source::default(),
            scene: Source::default(),
            width: Line::new(),
            mix: Line::new(),
        }
    }

    pub fn reset(&mut self) {}

    pub fn process(&mut self, p: &[SlotValue; 12], ctx: &FxCtx, l: &mut [f32; 32], r: &mut [f32; 32]) {
        self.audio.set(p[0].v, p[1].v, p[2].v);
        self.effect.set(p[3].v, p[4].v, p[5].v);
        self.scene.set(p[6].v, p[7].v, p[8].v);
        let mut wl = [0f32; 32];
        let mut wr = [0f32; 32];
        self.effect.add(l, r, &mut wl, &mut wr);
        if let Some((al, ar)) = ctx.audio_in {
            self.audio.add(al, ar, &mut wl, &mut wr);
        }
        if let Some((sl, sr)) = ctx.scene_in {
            self.scene.add(sl, sr, &mut wl, &mut wr);
        }
        self.width.set_target_smoothed(p[9].v);
        apply_width(&mut wl, &mut wr, &self.width);
        self.mix.set_target_smoothed(p[10].v);
        self.mix.fade_2_blocks(l, r, &wl, &wr);
    }

    pub fn process_only_control(&mut self, _p: &[SlotValue; 12], _ctx: &FxCtx) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::fxs::{default_values, FxCtx};

    fn sig(seed: f32) -> [f32; 32] {
        let mut x = [0f32; 32];
        for (k, s) in x.iter_mut().enumerate() {
            *s = (k as f32 * seed).sin() * 0.5;
        }
        x
    }

    #[test]
    fn defaults_pass_effect_input() {
        let p = default_values(29);
        let mut fx = AudioIn::new(48000.0);
        let ctx = FxCtx::new(48000.0);
        let (mut l, mut r) = (sig(0.3), sig(0.7));
        let (l0, r0) = (l, r);
        fx.process(&p, &ctx, &mut l, &mut r);
        for k in 0..32 {
            assert!((l[k] - l0[k]).abs() < 1e-6 && (r[k] - r0[k]).abs() < 1e-6, "{k}");
        }
    }

    #[test]
    fn audio_input_level() {
        let mut p = default_values(29);
        p[5].v = -96.0;
        let (al, ar) = (sig(0.2), sig(0.4));
        let mut ctx = FxCtx::new(48000.0);
        ctx.audio_in = Some((&al, &ar));
        let mut fx = AudioIn::new(48000.0);
        let (mut l, mut r) = ([0f32; 32], [0f32; 32]);
        fx.process(&p, &ctx, &mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|x| x.abs() < 1e-4));
        p[2].v = 0.0;
        let mut fx = AudioIn::new(48000.0);
        let (mut l, mut r) = ([0f32; 32], [0f32; 32]);
        fx.process(&p, &ctx, &mut l, &mut r);
        for k in 0..32 {
            assert!((l[k] - al[k]).abs() < 1e-6 && (r[k] - ar[k]).abs() < 1e-6);
        }
    }
}
