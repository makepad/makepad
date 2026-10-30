//! The Tandem engine: two scenes, the scene-mode note routing, the global
//! modulation list, the 16 effect slots (4 inserts per scene, 4 sends with
//! returns, 4 globals) with bypass modes and ring-out, global volume, the
//! patch-change fade and the master clip. Renders any buffer length in
//! internal 32-sample blocks (events land on block boundaries).

use super::catalog::{gl, sc, Catalog, N_FX, N_GLOBAL};
use super::patch::Patch;
use super::program::Program;
use super::scene::{Scene, SceneCtx, CTL_AT, CTL_BREATH, CTL_EXPR, CTL_MW, CTL_PB, CTL_SUS};
use crate::kit::core::{amp_to_linear, db_to_linear, BlockLine, SlotValue, BLOCK, BLOCK_OS};
use crate::kit::fxs::{ringout_blocks, Fx, FxCtx};
use super::formula::FormulaBank;
use crate::kit::tuning::Tuning;
use crate::kit::mods::ModCtx;
use std::sync::Arc;

/// Slot storage indices of each chain, in processing order.
pub const CHAIN_A: [usize; 4] = [0, 1, 8, 9];
pub const CHAIN_B: [usize; 4] = [2, 3, 10, 11];
pub const CHAIN_SEND: [usize; 4] = [4, 5, 12, 13];
pub const CHAIN_GLOBAL: [usize; 4] = [6, 7, 14, 15];

/// By default MIDI channel 2 plays only scene A and channel 3 only scene B.
pub const CHANNEL_2_3_SCENES: bool = true;

pub struct Tandem {
    fs: f32,
    prog: Arc<Program>,
    pending: Option<Arc<Program>>,
    scenes: [Scene; 2],
    fx: Vec<Fx>,
    ringout: [i32; N_FX],
    /// The formula runners: one per distinct script, shared by both scenes.
    formulas: FormulaBank,
    /// The user's tuning (None: standard).
    tuning: Option<Arc<Tuning>>,
    /// Play a patch's own embedded tuning when it has one ("override the
    /// tuning on patch load").
    patch_tuning: bool,
    global_eff: Vec<f32>,
    send_lines: [[BlockLine; 4]; 2],
    return_lines: [BlockLine; 4],
    volume_line: BlockLine,
    fade: f32,
    fading_out: bool,
    macros: [f32; 8],
    bpm: f64,
    song_beats: f64,
    out_l: [f32; BLOCK],
    out_r: [f32; BLOCK],
    pos: usize,
    pub mpe: bool,
    held_for_retrigger: Vec<(u8, i32, f32)>,
    audio_in: [[f32; BLOCK]; 2],
    audio_in_os: [[f32; BLOCK_OS]; 2],
    in_hb: crate::kit::core::HalfBand,
}

impl Tandem {
    pub fn new(fs: f32) -> Tandem {
        let prog = Arc::new(Program::compile(&Patch::init()));
        let mut t = Tandem {
            fs,
            prog: prog.clone(),
            pending: None,
            scenes: [Scene::new(0, fs), Scene::new(1, fs)],
            fx: (0..N_FX).map(|_| Fx::new(0, fs)).collect(),
            ringout: [0; N_FX],
            formulas: FormulaBank::default(),
            tuning: None,
            patch_tuning: false,
            global_eff: vec![0.0; N_GLOBAL],
            send_lines: Default::default(),
            return_lines: Default::default(),
            volume_line: BlockLine::default(),
            fade: 1.0,
            fading_out: false,
            macros: [0.0; 8],
            bpm: 120.0,
            song_beats: 0.0,
            out_l: [0.0; BLOCK],
            out_r: [0.0; BLOCK],
            pos: BLOCK,
            mpe: false,
            held_for_retrigger: Vec::with_capacity(128),
            audio_in: [[0.0; BLOCK]; 2],
            audio_in_os: [[0.0; BLOCK_OS]; 2],
            in_hb: crate::kit::core::HalfBand::new(12, true),
        };
        t.install(prog);
        t
    }

    /// Sounding voices of a scene.
    pub fn voices(&self, scene: usize) -> usize {
        self.scenes[scene].voices.iter().filter(|v| v.active()).count()
    }

    /// Keys of a scene's gated (held, not released) voices.
    pub fn gated_keys(&self, scene: usize) -> Vec<i32> {
        self.scenes[scene].voices.iter().filter(|v| v.active() && v.gate).map(|v| v.key).collect()
    }

    pub fn sample_rate(&self) -> f32 {
        self.fs
    }

    pub fn program(&self) -> &Arc<Program> {
        &self.prog
    }

    /// Load a patch now (allocates: call off the audio thread, or use
    /// [`Tandem::queue_program`]).
    pub fn load(&mut self, patch: &Patch) {
        self.install(Arc::new(Program::compile(patch)));
    }

    /// Play a compiled program from the next block (no fade).
    pub fn set_program(&mut self, prog: Arc<Program>) {
        self.install(prog);
    }

    /// Queue a compiled program: the output fades out, the program swaps,
    /// held keys retrigger.
    pub fn queue_program(&mut self, prog: Arc<Program>) {
        self.pending = Some(prog);
        self.fading_out = true;
    }

    fn install(&mut self, prog: Arc<Program>) {
        // FX slots of a new type are made here (allocation off the render path when loading).
        for s in 0..N_FX {
            let ty = prog.values[gl::FX + gl::FX_STRIDE * s].i().clamp(0, 31) as u8;
            if self.fx[s].ty() != ty {
                self.fx[s] = Fx::new(ty, self.fs);
            } else {
                self.fx[s].reset();
            }
            let b = gl::FX + gl::FX_STRIDE * s + 1;
            let p: [SlotValue; 12] = std::array::from_fn(|k| prog.values[b + k]);
            self.fx[s].prepare(&p, self.fs, prog.fx_user[s].as_ref());
            self.ringout[s] = 0;
        }
        self.formulas = FormulaBank::for_program(&prog);
        for sc_ in &mut self.scenes {
            sc_.set_program(&prog, &mut self.formulas);
        }
        for m in 0..8 {
            self.macros[m] = prog.macros[m].0;
        }
        self.global_eff.iter_mut().zip(prog.global_base()).for_each(|(e, b)| *e = b);
        self.prog = prog;
    }

    /// The tuning the keys play (None: standard 12-TET). How it applies
    /// follows the patch's tuning mode: retune the keys' notes (the
    /// default) or every pitch.
    pub fn set_tuning(&mut self, tuning: Option<Arc<Tuning>>) {
        self.tuning = tuning;
    }

    /// Whether a patch's embedded tuning replaces the user's while it plays.
    pub fn set_patch_tuning(&mut self, on: bool) {
        self.patch_tuning = on;
    }

    /// The tuning playing now.
    pub fn tuning(&self) -> Option<&Arc<Tuning>> {
        active_arc(&self.tuning, &self.prog, self.patch_tuning)
    }

    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm > 1.0 {
            self.bpm = bpm;
        }
    }

    pub fn set_macro(&mut self, i: usize, v: f32) {
        if let Some(m) = self.macros.get_mut(i) {
            *m = v;
        }
    }

    /// Stereo host input for the audio-in oscillator / effect.
    pub fn set_audio_in(&mut self, l: &[f32], r: &[f32]) {
        for i in 0..BLOCK {
            self.audio_in[0][i] = l.get(i).copied().unwrap_or(0.0).clamp(-8.0, 8.0);
            self.audio_in[1][i] = r.get(i).copied().unwrap_or(0.0).clamp(-8.0, 8.0);
        }
    }

    fn scene_mask(&self, channel: u8, key: i32) -> [bool; 2] {
        let mode = self.prog.values[gl::SCENEMODE].i();
        let split = self.prog.values[gl::SPLITKEY].i();
        if CHANNEL_2_3_SCENES && !self.mpe && mode != 3 {
            match channel {
                1 => return [true, false],
                2 => return [false, true],
                _ => {}
            }
        }
        match mode {
            1 => [key < split, key >= split],
            2 => [true, true],
            3 => {
                let a = (channel as i32) < split / 8 + 1;
                [a, !a]
            }
            _ => {
                let active = self.prog.values[gl::SCENE_ACTIVE].i() == 1;
                [!active, active]
            }
        }
    }

    fn ctx_parts(&self) -> (ModCtx, u8, usize) {
        let mod_ctx = ModCtx { fs: self.fs, block_secs: BLOCK as f32 / self.fs, tsr: (self.bpm / 120.0) as f32, song_pos_beats: self.song_beats };
        let character = self.prog.values[Catalog::CHARACTER].i().clamp(0, 2) as u8;
        let polylimit = self.prog.values[gl::POLYLIMIT].i().clamp(2, 64) as usize;
        (mod_ctx, character, polylimit)
    }

    pub fn note_on(&mut self, channel: u8, key: u8, velocity: u8) {
        if velocity == 0 {
            return self.note_off(channel, key, 64);
        }
        self.held_for_retrigger.retain(|(c, k, _)| !(*c == channel && *k == key as i32));
        self.held_for_retrigger.push((channel, key as i32, velocity as f32 / 127.0));
        let mask = self.scene_mask(channel, key as i32);
        let (mod_ctx, character, polylimit) = self.ctx_parts();
        let prog = &self.prog;
        for s in 0..2 {
            if !mask[s] {
                continue;
            }
            let np = &prog.nonparam;
            let cx = SceneCtx { prog, fs: self.fs, mod_ctx, character, macros: &self.macros, polylimit, audio_in_os: None, tuning: active_tuning(&self.tuning, prog, self.patch_tuning), retune_all: prog.nonparam.tuning_mode == 0 };
            self.scenes[s].note_on(&cx, &mut self.formulas, channel, key as i32, velocity as f32 / 127.0, np.mono_priority[s], np.mono_envelope[s], np.repeated_key[s]);
        }
    }

    pub fn note_off(&mut self, channel: u8, key: u8, velocity: u8) {
        self.held_for_retrigger.retain(|(c, k, _)| !(*c == channel && *k == key as i32));
        let (mod_ctx, character, polylimit) = self.ctx_parts();
        let prog = &self.prog;
        for s in 0..2 {
            let cx = SceneCtx { prog, fs: self.fs, mod_ctx, character, macros: &self.macros, polylimit, audio_in_os: None, tuning: active_tuning(&self.tuning, prog, self.patch_tuning), retune_all: prog.nonparam.tuning_mode == 0 };
            self.scenes[s].note_off(&cx, &mut self.formulas, channel, key as i32, velocity as f32 / 127.0, prog.nonparam.mono_priority[s]);
        }
    }

    /// Pitch bend, -8192..8191.
    pub fn pitch_bend(&mut self, channel: u8, value: i32) {
        let v = (value as f32 / 8192.0).clamp(-1.0, 1.0);
        for s in &mut self.scenes {
            if self.mpe && channel != 0 {
                s.chan_bend[channel as usize & 15] = v;
            } else {
                s.ctl[CTL_PB].set_target(v);
            }
        }
    }

    pub fn channel_pressure(&mut self, channel: u8, value: u8) {
        let v = value as f32 / 127.0;
        for s in &mut self.scenes {
            if self.mpe && channel != 0 {
                s.chan_pressure[channel as usize & 15] = v;
            } else {
                s.ctl[CTL_AT].set_target(v);
            }
        }
    }

    pub fn poly_pressure(&mut self, channel: u8, key: u8, value: u8) {
        for s in &mut self.scenes {
            s.poly_at[channel as usize & 15][key as usize & 127] = value as f32 / 127.0;
        }
    }

    pub fn control_change(&mut self, channel: u8, cc: u8, value: u8) {
        let v = value as f32 / 127.0;
        let detach = self.prog.nonparam.detach_sustain;
        for s in &mut self.scenes {
            match cc {
                1 => s.ctl[CTL_MW].set_target(v),
                2 => s.ctl[CTL_BREATH].set_target(v),
                11 => s.ctl[CTL_EXPR].set_target(v),
                64 => {
                    s.ctl[CTL_SUS].set_target(v);
                    if !detach {
                        s.set_sustain(value >= 64);
                    }
                }
                66 => s.set_sostenuto(value >= 64),
                74 if self.mpe => {
                    let t = if value > 64 { (value as f32 - 64.0) / 63.0 } else { (value as f32 - 64.0) / 64.0 };
                    s.chan_timbre[channel as usize & 15] = t;
                }
                120 => s.all_sound_off(&mut self.formulas),
                123 => s.all_notes_off(),
                _ => {}
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        self.held_for_retrigger.clear();
        for s in &mut self.scenes {
            s.all_notes_off();
        }
    }

    /// Render `l.len()` samples (stereo).
    pub fn render(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len().min(r.len());
        let mut i = 0;
        while i < n {
            if self.pos >= BLOCK {
                self.block();
                self.pos = 0;
            }
            let take = (BLOCK - self.pos).min(n - i);
            l[i..i + take].copy_from_slice(&self.out_l[self.pos..self.pos + take]);
            r[i..i + take].copy_from_slice(&self.out_r[self.pos..self.pos + take]);
            self.pos += take;
            i += take;
        }
    }

    fn fx_params(&self, slot: usize) -> [SlotValue; 12] {
        let b = gl::FX + gl::FX_STRIDE * slot + 1;
        std::array::from_fn(|k| {
            let mut v = self.prog.values[b + k];
            v.v = self.global_eff[b + k];
            v
        })
    }

    fn run_chain(&mut self, chain: &[usize; 4], input_present: bool, l: &mut [f32; BLOCK], r: &mut [f32; BLOCK]) -> bool {
        let disable = self.prog.values[gl::FX_DISABLE].i();
        let tsr = (self.bpm / 120.0) as f32;
        let mut any = false;
        for &s in chain {
            if self.fx[s].ty() == 0 || disable & (1 << s) != 0 {
                continue;
            }
            let p = self.fx_params(s);
            if input_present {
                self.ringout[s] = 0;
            } else {
                self.ringout[s] += 1;
            }
            let d = ringout_blocks(self.fx[s].ty(), &p);
            let ain = (&self.audio_in[0], &self.audio_in[1]);
            let ctx = FxCtx { sr: self.fs, tsr, audio_in: Some(ain), user_data: self.prog.fx_user[s].as_ref(), scene_in: None };
            if d < 0 || self.ringout[s] < d || self.ringout[s] == 0 {
                self.fx[s].process(&p, &ctx, l, r);
                any = true;
            } else {
                self.fx[s].process_only_control(&p, &ctx);
            }
        }
        any || input_present
    }

    fn block(&mut self) {
        // Patch-change fade.
        if self.fading_out {
            self.fade -= 0.05;
            if self.fade <= 0.0 {
                self.fade = 0.0;
                if let Some(p) = self.pending.take() {
                    self.install(p);
                    let held = std::mem::take(&mut self.held_for_retrigger);
                    for (c, k, v) in &held {
                        self.note_on(*c, *k as u8, (*v * 127.0).round() as u8);
                    }
                }
                self.fading_out = false;
            }
        } else if self.fade < 1.0 {
            self.fade = (self.fade + 0.05).min(1.0);
        }
        let (mod_ctx, character, polylimit) = self.ctx_parts();
        // Audio input: 2x for the oscillators.
        {
            let (a, b) = (self.audio_in[0], self.audio_in[1]);
            let (mut ul, mut ur) = ([0.0; BLOCK_OS], [0.0; BLOCK_OS]);
            self.in_hb.upsample_into(&a, &b, &mut ul, &mut ur, 1.0);
            self.audio_in_os = [ul, ur];
        }
        // Scenes: sources, scene copy, scene LFOs.
        let prog_arc = self.prog.clone();
        let prog: &Program = &prog_arc;
        let playing = match prog.values[gl::SCENEMODE].i() {
            0 => {
                let a = prog.values[gl::SCENE_ACTIVE].i() == 1;
                [!a || self.scenes[0].active_voices > 0, a || self.scenes[1].active_voices > 0]
            }
            _ => [true, true],
        };
        let ain = (&self.audio_in_os[0], &self.audio_in_os[1]);
        for s in 0..2 {
            let cx = SceneCtx { prog, fs: self.fs, mod_ctx, character, macros: &self.macros, polylimit, audio_in_os: Some(ain), tuning: active_tuning(&self.tuning, prog, self.patch_tuning), retune_all: prog.nonparam.tuning_mode == 0 };
            self.scenes[s].begin_block(&cx, &mut self.formulas);
        }
        // Global list (scene LFOs are this block's).
        for (e, b) in self.global_eff.iter_mut().zip(prog.global_base()) {
            *e = b;
        }
        for r in &prog.global_routes {
            let sc_ = &self.scenes[r.src_scene as usize];
            let v = if (23..=28).contains(&r.src) && r.index >= 3 { sc_.slfo16[r.src as usize - 23][(r.index as usize).min(15)] } else { sc_.sources[r.src as usize][(r.index as usize).min(2)] };
            if let Some(d) = self.global_eff.get_mut(r.dst as usize) {
                *d += r.depth * v;
            }
        }
        // Voices.
        let np = prog.nonparam;
        for s in 0..2 {
            if !playing[s] {
                self.scenes[s].out_l = [0.0; BLOCK];
                self.scenes[s].out_r = [0.0; BLOCK];
                continue;
            }
            let cx = SceneCtx { prog, fs: self.fs, mod_ctx, character, macros: &self.macros, polylimit, audio_in_os: Some(ain), tuning: active_tuning(&self.tuning, prog, self.patch_tuning), retune_all: prog.nonparam.tuning_mode == 0 };
            self.scenes[s].render(&cx, &mut self.formulas, np.hardclip_scene[s]);
        }
        let bypass = self.prog.values[gl::FX_BYPASS].i();
        let mut al = self.scenes[0].out_l;
        let mut ar = self.scenes[0].out_r;
        let mut bl = self.scenes[1].out_l;
        let mut br = self.scenes[1].out_r;
        let present = [self.scenes[0].active_voices > 0, self.scenes[1].active_voices > 0];
        if bypass != 3 {
            self.run_chain(&CHAIN_A, present[0], &mut al, &mut ar);
            self.run_chain(&CHAIN_B, present[1], &mut bl, &mut br);
        }
        let clip = |x: &mut [f32; BLOCK], mode: i32| match mode {
            1 => x.iter_mut().for_each(|s| *s = s.clamp(-8.0, 8.0)),
            2 => x.iter_mut().for_each(|s| *s = s.clamp(-1.0, 1.0)),
            _ => {}
        };
        clip(&mut al, np.hardclip_scene[0]);
        clip(&mut ar, np.hardclip_scene[0]);
        clip(&mut bl, np.hardclip_scene[1]);
        clip(&mut br, np.hardclip_scene[1]);
        let mut ol = [0.0f32; BLOCK];
        let mut or = [0.0f32; BLOCK];
        for i in 0..BLOCK {
            ol[i] = al[i] + bl[i];
            or[i] = ar[i] + br[i];
        }
        let mut sends_out = false;
        // Sends.
        for (k, &slot) in CHAIN_SEND.iter().enumerate() {
            let ga = amp_to_linear(self.scenes[0].eff()[sc::SEND + k]);
            let gb = amp_to_linear(self.scenes[1].eff()[sc::SEND + k]);
            self.send_lines[0][k].set(ga, BLOCK);
            self.send_lines[1][k].set(gb, BLOCK);
            let ret = amp_to_linear(self.global_eff[gl::RETURN + k]);
            self.return_lines[k].set(ret, BLOCK);
            if bypass != 0 || self.fx[slot].ty() == 0 || self.prog.values[gl::FX_DISABLE].i() & (1 << slot) != 0 {
                for _ in 0..BLOCK {
                    self.send_lines[0][k].next();
                    self.send_lines[1][k].next();
                    self.return_lines[k].next();
                }
                continue;
            }
            let mut sl = [0.0f32; BLOCK];
            let mut sr = [0.0f32; BLOCK];
            for i in 0..BLOCK {
                let (a, b) = (self.send_lines[0][k].next(), self.send_lines[1][k].next());
                sl[i] = a * al[i] + b * bl[i];
                sr[i] = a * ar[i] + b * br[i];
            }
            let one = [slot];
            let ran = self.run_chain_one(&one, present[0] || present[1], &mut sl, &mut sr);
            sends_out |= ran;
            for i in 0..BLOCK {
                let g = self.return_lines[k].next();
                ol[i] += g * sl[i];
                or[i] += g * sr[i];
            }
        }
        if bypass == 0 || bypass == 1 {
            self.run_chain(&CHAIN_GLOBAL, present[0] || present[1] || sends_out, &mut ol, &mut or);
        }
        let vol = db_to_linear(self.global_eff[gl::VOLUME]);
        self.volume_line.set(vol, BLOCK);
        let f2 = self.fade * self.fade;
        for i in 0..BLOCK {
            let g = self.volume_line.next() * f2;
            ol[i] *= g;
            or[i] *= g;
        }
        clip(&mut ol, np.hardclip_global);
        clip(&mut or, np.hardclip_global);
        self.out_l = ol;
        self.out_r = or;
        self.song_beats += self.bpm / 60.0 * BLOCK as f64 / self.fs as f64;
    }

    fn run_chain_one(&mut self, chain: &[usize; 1], input_present: bool, l: &mut [f32; BLOCK], r: &mut [f32; BLOCK]) -> bool {
        let slot = chain[0];
        let disable = self.prog.values[gl::FX_DISABLE].i();
        if self.fx[slot].ty() == 0 || disable & (1 << slot) != 0 {
            return false;
        }
        let tsr = (self.bpm / 120.0) as f32;
        let p = self.fx_params(slot);
        if input_present {
            self.ringout[slot] = 0;
        } else {
            self.ringout[slot] += 1;
        }
        let d = ringout_blocks(self.fx[slot].ty(), &p);
        let ain = (&self.audio_in[0], &self.audio_in[1]);
        let ctx = FxCtx { sr: self.fs, tsr, audio_in: Some(ain), user_data: self.prog.fx_user[slot].as_ref(), scene_in: None };
        if d < 0 || self.ringout[slot] < d || self.ringout[slot] == 0 {
            self.fx[slot].process(&p, &ctx, l, r);
            true
        } else {
            self.fx[slot].process_only_control(&p, &ctx);
            false
        }
    }
}

fn active_arc<'a>(user: &'a Option<Arc<Tuning>>, prog: &'a Program, patch: bool) -> Option<&'a Arc<Tuning>> {
    match (&prog.tuning, patch) {
        (Some(t), true) => Some(t),
        _ => user.as_ref(),
    }
}

/// The tuning the scenes play: the patch's own when allowed, else the user's.
fn active_tuning<'a>(user: &'a Option<Arc<Tuning>>, prog: &'a Program, patch: bool) -> Option<&'a Tuning> {
    active_arc(user, prog, patch).map(|t| &**t)
}
