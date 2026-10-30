//! One Tandem scene: its voice pool and voice manager (poly, mono modes,
//! latch, legato, glide, stealing, sustain/sostenuto, MPE), its scene-level
//! modulation sources and six scene LFOs, the summed 2x voice bus, the
//! scene hard clip, the half-band decimator and the scene low cut.

use super::catalog::{sc, N_SCENE};
use super::program::{Program, N_SOURCES};
use super::formula::FormulaBank;
use crate::kit::tuning::Tuning;
use super::voice::{apply_routes_with, NoteStart, Sources, VState, Voice, VoiceCtx};
use crate::dsp::Rng;
use crate::kit::core::{calc_omega, Biquad, HalfBand, BLOCK, BLOCK_OS};
use crate::kit::mods::{ControllerSmoother, Lfo, ModCtx};

pub const MAX_VOICES: usize = 64;

/// Controller sources a scene smooths (index into `ctl`).
pub const CTL_AT: usize = 0;
pub const CTL_PB: usize = 1;
pub const CTL_MW: usize = 2;
pub const CTL_BREATH: usize = 3;
pub const CTL_EXPR: usize = 4;
pub const CTL_SUS: usize = 5;

#[derive(Clone, Copy, Debug)]
struct Held {
    key: i32,
    channel: u8,
    order: u64,
}

pub struct Scene {
    pub index: usize,
    fs: f32,
    pub voices: Vec<Voice>,
    eff: Box<[f32; N_SCENE]>,
    lfos: Vec<Lfo>,
    pub sources: Sources,
    /// Scene LFOs' 16 outputs (formula sources past index 2).
    pub slfo16: [[f32; 16]; 6],
    /// Formula instances of the scene LFOs (in the engine's bank).
    slfo_k: [Option<usize>; 6],
    pub ctl: [ControllerSmoother; 6],
    held: Vec<Held>,
    pub sustain: bool,
    sostenuto_keys: Vec<i32>,
    order: u64,
    last_key: f32,
    keys_valid: bool,
    rng: Rng,
    alt_state: bool,
    pub bus_l: [f32; BLOCK_OS],
    pub bus_r: [f32; BLOCK_OS],
    pub out_l: [f32; BLOCK],
    pub out_r: [f32; BLOCK],
    hb: HalfBand,
    lowcut: [Biquad; 4],
    pub active_voices: usize,
    pub mpe: bool,
    pub mpe_bend_range: f32,
    /// Per channel MPE state: bend (-1..1), pressure, timbre.
    pub chan_bend: [f32; 16],
    pub chan_pressure: [f32; 16],
    pub chan_timbre: [f32; 16],
    pub poly_at: [[f32; 128]; 16],
}

/// Per-block environment from the engine.
pub struct SceneCtx<'a> {
    pub prog: &'a Program,
    pub fs: f32,
    pub mod_ctx: ModCtx,
    pub character: u8,
    pub macros: &'a [f32; 8],
    pub polylimit: usize,
    pub audio_in_os: Option<(&'a [f32; BLOCK_OS], &'a [f32; BLOCK_OS])>,
    /// The active tuning (None: standard) and whether it retunes every
    /// pitch (else only the keys' notes).
    pub tuning: Option<&'a Tuning>,
    pub retune_all: bool,
}

impl Scene {
    pub fn new(index: usize, fs: f32) -> Scene {
        Scene {
            index,
            fs,
            voices: (0..MAX_VOICES).map(|v| Voice::new(0x1234_5678 ^ ((index as u32) << 16) ^ v as u32)).collect(),
            eff: Box::new([0.0; N_SCENE]),
            lfos: (0..6).map(|l| Lfo::new(0xabc0_0000 ^ ((index as u32) << 8) ^ l as u32, true)).collect(),
            sources: [[0.0; 3]; N_SOURCES],
            slfo16: [[0.0; 16]; 6],
            slfo_k: [None; 6],
            ctl: Default::default(),
            held: Vec::with_capacity(128),
            sustain: false,
            sostenuto_keys: Vec::with_capacity(128),
            order: 1,
            last_key: 60.0,
            keys_valid: false,
            rng: Rng::new(0x51ce_0000 ^ index as u32),
            alt_state: false,
            bus_l: [0.0; BLOCK_OS],
            bus_r: [0.0; BLOCK_OS],
            out_l: [0.0; BLOCK],
            out_r: [0.0; BLOCK],
            hb: HalfBand::new(12, true),
            lowcut: [Biquad::new(); 4],
            active_voices: 0,
            mpe: false,
            mpe_bend_range: 48.0,
            chan_bend: [0.0; 16],
            chan_pressure: [0.0; 16],
            chan_timbre: [0.0; 16],
            poly_at: [[0.0; 128]; 16],
        }
    }

    /// A new program; `bank` is the engine's new formula bank (the old
    /// one's instances are forgotten).
    pub fn set_program(&mut self, prog: &Program, bank: &mut FormulaBank) {
        let s = self.index;
        self.slfo_k = std::array::from_fn(|l| bank.acquire(s, 6 + l));
        for (i, v) in self.voices.iter_mut().enumerate() {
            v.drop_formulas();
            v.set_program(prog, self.index, 0x77 ^ i as u32, self.fs);
        }
        self.eff.iter_mut().zip(prog.scene_base(self.index)).for_each(|(e, b)| *e = b);
        self.ctl[CTL_MW].snap(prog.modwheel[self.index]);
        self.held.clear();
        self.hb.reset();
        for b in &mut self.lowcut {
            b.suspend();
        }
        self.active_voices = 0;
    }

    pub fn eff(&self) -> &[f32; N_SCENE] {
        &self.eff
    }

    fn polymode(&self) -> i32 {
        self.eff[sc::POLYMODE].round() as i32
    }

    fn bend_semis(&self, prog: &Program) -> f32 {
        let pb = self.sources[5][0];
        let s = self.index;
        let (k, i) = if pb > 0.0 { (sc::PBRANGE_UP, pb) } else { (sc::PBRANGE_DN, pb) };
        let v = prog.scene_value(s, k);
        let range = if v.extend { self.eff[k] / 100.0 } else { self.eff[k] };
        i * range
    }

    /// The voices' block context (built from copies, so the voices can be borrowed mutably).
    #[allow(clippy::too_many_arguments)]
    fn voice_ctx<'a>(index: usize, mpe: bool, bend_semis: f32, cx: &'a SceneCtx<'a>, scene_eff: &'a [f32; N_SCENE], src: &'a Sources, lfo16: &'a [[f32; 16]; 6]) -> VoiceCtx<'a> {
        VoiceCtx {
            prog: cx.prog,
            scene: index,
            scene_eff,
            scene_src: src,
            scene_lfo16: lfo16,
            fs_os: cx.fs * 2.0,
            mod_ctx: cx.mod_ctx,
            bend_semis,
            character: cx.character,
            mpe,
            audio_in: cx.audio_in_os,
            retune_all: cx.tuning.filter(|_| cx.retune_all),
        }
    }

    /// The note key `key` plays: itself, its tuned note number when the
    /// tuning retunes the keys, None when the tuning leaves it unmapped.
    fn note_of(cx: &SceneCtx, key: i32) -> Option<f32> {
        match cx.tuning {
            Some(t) if cx.retune_all => t.key_hz(key).map(|_| key as f32),
            Some(t) => t.key_note(key),
            None => Some(key as f32),
        }
    }

    fn gated(&self) -> bool {
        self.voices.iter().any(|v| v.active() && v.gate)
    }

    fn draw_random(&mut self) -> ([f32; 4], [f32; 2]) {
        let u = self.rng.unit();
        // Normal by Box-Muller, sigma 0.33, clipped.
        let (a, b) = (self.rng.unit().max(1e-9), self.rng.unit());
        let n = (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos() * 0.33;
        let rand = [2.0 * u - 1.0, n.clamp(-1.0, 1.0), self.rng.unit(), n.abs().min(1.0)];
        let alt = if self.alt_state { [1.0, 1.0] } else { [-1.0, 0.0] };
        self.alt_state = !self.alt_state;
        (rand, alt)
    }

    /// Steal before allocating (ARCHITECTURE 3.2).
    fn make_room(&mut self, polylimit: usize, bank: &mut FormulaBank) {
        loop {
            let n = self.voices.iter().filter(|v| v.active() && !v.fast_release).count();
            if n + 1 <= polylimit {
                break;
            }
            let pick = self
                .voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.active() && !v.fast_release && !v.gate)
                .min_by_key(|(_, v)| v.release_age)
                .map(|(i, _)| i)
                .or_else(|| self.voices.iter().enumerate().filter(|(_, v)| v.active() && !v.fast_release).min_by_key(|(_, v)| v.age).map(|(i, _)| i));
            match pick {
                Some(i) => {
                    self.order += 1;
                    let o = self.order;
                    self.voices[i].uber_release(o);
                }
                None => break,
            }
        }
        let count = self.voices.iter().filter(|v| v.active()).count();
        if count > polylimit + 3 {
            let mut fast: Vec<(u64, usize)> = self.voices.iter().enumerate().filter(|(_, v)| v.active() && v.fast_release).map(|(i, v)| (v.age, i)).collect();
            fast.sort_unstable();
            for (_, i) in fast.into_iter().take(count - polylimit - 3) {
                self.voices[i].kill();
                self.voices[i].release_formulas(self.index, bank);
            }
        }
    }

    fn free_voice(&self) -> Option<usize> {
        self.voices.iter().position(|v| !v.active())
    }

    fn update_keys(&mut self) {
        let root = self.eff[sc::KTRKROOT];
        if let (Some(lo), Some(hi)) = (self.held.iter().map(|h| h.key).min(), self.held.iter().map(|h| h.key).max()) {
            let latest = self.held.iter().max_by_key(|h| h.order).map(|h| h.key).unwrap_or(lo);
            self.sources[38] = [(lo as f32 - root) / 12.0; 3];
            self.sources[39] = [(hi as f32 - root) / 12.0; 3];
            self.sources[40] = [(latest as f32 - root) / 12.0; 3];
            self.keys_valid = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn note_on(&mut self, cx: &SceneCtx, bank: &mut FormulaBank, channel: u8, key: i32, velocity: f32, priority: i32, mono_env: i32, repeat_key: i32) {
        let Some(note) = Self::note_of(cx, key) else { return };
        self.order += 1;
        let order = self.order;
        self.held.retain(|h| !(h.key == key && h.channel == channel));
        self.held.push(Held { key, channel, order });
        self.update_keys();
        // Scene LFOs attack when no voice is gated (or every note with trigmode deform 1).
        let any_gated = self.gated();
        let scene_eff = *self.eff;
        for l in 0..6 {
            let li = 6 + l;
            let every = cx.prog.scene_value(self.index, sc::LFO + sc::LFO_STRIDE * li + 5).deform == 1;
            if !any_gated || every {
                let lp = super::convert::lfo(cx.prog, self.index, li, &scene_eff);
                self.lfos[l].attack(&lp, &cx.mod_ctx, &cx.prog.stepseq[self.index][li], &cx.prog.msegs[self.index][li]);
                if let Some(k) = self.slfo_k[l] {
                    let macros: [f32; 8] = std::array::from_fn(|m| self.sources[7 + m][0]);
                    let inp = super::formula::inputs(cx.prog, li, &scene_eff, &self.lfos[l], &cx.mod_ctx, macros, None);
                    bank.start(self.index, li, k, &inp);
                }
            }
        }
        let (rand, alt) = self.draw_random();
        let mode = self.polymode();
        let porta_on = self.eff[sc::PORTAMENTO] > -8.0;
        let src = self.sources;
        match mode {
            0 => {
                if repeat_key == 1 {
                    if let Some(i) = self.voices.iter().position(|v| v.active() && v.key == key && v.channel == channel) {
                        let n = NoteStart { key, note, channel, velocity, age: order, glide_from: porta_on.then_some(self.last_key), rand, alt, from_current: true };
                        self.start_voice(cx, bank, i, &n, &scene_eff, &src);
                        self.last_key = note;
                        return;
                    }
                }
                self.make_room(cx.polylimit, bank);
                if let Some(i) = self.free_voice() {
                    let n = NoteStart { key, note, channel, velocity, age: order, glide_from: porta_on.then_some(self.last_key), rand, alt, from_current: false };
                    self.start_voice(cx, bank, i, &n, &scene_eff, &src);
                }
            }
            _ => {
                // Priority: skip if a higher (highest) / lower (lowest) key of the channel is held.
                let others = self.held.iter().filter(|h| h.key != key && (self.mpe || h.channel == channel));
                let skip = match priority {
                    2 => others.clone().any(|h| h.key > key),
                    3 => others.clone().any(|h| h.key < key),
                    _ => false,
                };
                if skip {
                    return;
                }
                let legato_held = self.held.iter().any(|h| h.key != key);
                let single = mode == 2 || mode == 4;
                if single {
                    if let Some(i) = self.voices.iter().position(|v| v.active() && v.gate && !v.fast_release) {
                        let glide = porta_on && (mode == 2 || legato_held);
                        self.voices[i].legato(key, note, velocity, glide);
                        self.voices[i].channel = channel;
                        self.last_key = note;
                        return;
                    }
                }
                let glide_from = if !porta_on || (mode == 3 && !legato_held) || (mode == 4 && !legato_held) { None } else { Some(self.last_key) };
                let reclaim = if mono_env == 1 { self.voices.iter().position(|v| v.active() && !v.fast_release) } else { None };
                for (i, v) in self.voices.iter_mut().enumerate() {
                    if v.active() && Some(i) != reclaim {
                        v.uber_release(order);
                    }
                }
                let idx = reclaim.or_else(|| self.free_voice());
                if let Some(i) = idx {
                    let n = NoteStart { key, note, channel, velocity, age: order, glide_from, rand, alt, from_current: reclaim.is_some() };
                    self.start_voice(cx, bank, i, &n, &scene_eff, &src);
                }
            }
        }
        self.last_key = note;
    }

    #[allow(clippy::too_many_arguments)]
    fn start_voice(&mut self, cx: &SceneCtx, bank: &mut FormulaBank, i: usize, n: &NoteStart, scene_eff: &[f32; N_SCENE], src: &Sources) {
        let lfo16 = self.slfo16;
        let vc = Self::voice_ctx(self.index, self.mpe, self.bend_semis(cx.prog), cx, scene_eff, src, &lfo16);
        let v = &mut self.voices[i];
        v.start(n, &vc, bank);
        if self.mpe {
            v.mpe_bend = self.chan_bend[n.channel as usize & 15] * self.mpe_bend_range;
            v.mpe_pressure = self.chan_pressure[n.channel as usize & 15];
            v.mpe_timbre = self.chan_timbre[n.channel as usize & 15];
        }
    }

    pub fn note_off(&mut self, cx: &SceneCtx, bank: &mut FormulaBank, channel: u8, key: i32, rel_vel: f32, priority: i32) {
        self.held.retain(|h| !(h.key == key && h.channel == channel));
        self.update_keys();
        self.order += 1;
        let order = self.order;
        let mode = self.polymode();
        if mode == 5 {
            return;
        }
        if mode != 0 {
            let others: Vec<Held> = self.held.iter().copied().filter(|h| self.mpe || h.channel == channel).collect();
            let sounding = self.voices.iter().position(|v| v.active() && v.gate && v.key == key);
            if let (Some(vi), false) = (sounding, others.is_empty()) {
                let next = match priority {
                    1 => others.iter().max_by_key(|h| h.order),
                    3 => others.iter().min_by_key(|h| h.key),
                    _ => others.iter().max_by_key(|h| h.key),
                }
                .copied()
                .unwrap();
                let velocity = self.voices[vi].velocity;
                let next_note = Self::note_of(cx, next.key).unwrap_or(next.key as f32);
                if mode == 2 || mode == 4 {
                    let porta_on = self.eff[sc::PORTAMENTO] > -8.0;
                    self.voices[vi].legato(next.key, next_note, velocity, porta_on);
                    self.last_key = next_note;
                } else {
                    // Multi-trigger: a new voice for the held key.
                    let (rand, alt) = self.draw_random();
                    let porta_on = self.eff[sc::PORTAMENTO] > -8.0;
                    let scene_eff = *self.eff;
                    let src = self.sources;
                    for v in self.voices.iter_mut().filter(|v| v.active()) {
                        v.uber_release(order);
                    }
                    if let Some(i) = self.free_voice() {
                        let n = NoteStart { key: next.key, note: next_note, channel: next.channel, velocity, age: order, glide_from: porta_on.then_some(self.last_key), rand, alt, from_current: false };
                        self.start_voice(cx, bank, i, &n, &scene_eff, &src);
                    }
                    self.last_key = next_note;
                }
                return;
            }
        }
        let sostenuto = self.sostenuto_keys.contains(&key);
        for v in self.voices.iter_mut().filter(|v| v.active() && v.gate && v.key == key && v.channel == channel) {
            if self.sustain || sostenuto {
                v.sustained = true;
            } else {
                v.release(rel_vel, order);
            }
        }
    }

    pub fn set_sustain(&mut self, on: bool) {
        self.sustain = on;
        if !on {
            self.order += 1;
            let order = self.order;
            let held: Vec<i32> = self.held.iter().map(|h| h.key).collect();
            let sost = self.sostenuto_keys.clone();
            for v in self.voices.iter_mut().filter(|v| v.active() && v.sustained) {
                if !held.contains(&v.key) && !sost.contains(&v.key) {
                    v.sustained = false;
                    v.release(0.0, order);
                }
            }
        }
    }

    pub fn set_sostenuto(&mut self, on: bool) {
        if on {
            self.sostenuto_keys = self.held.iter().map(|h| h.key).collect();
        } else {
            self.sostenuto_keys.clear();
            if !self.sustain {
                self.set_sustain(false);
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        self.held.clear();
        self.order += 1;
        let o = self.order;
        for v in &mut self.voices {
            v.release(0.0, o);
        }
    }

    pub fn all_sound_off(&mut self, bank: &mut FormulaBank) {
        self.held.clear();
        for v in &mut self.voices {
            v.kill();
            v.release_formulas(self.index, bank);
        }
    }

    /// Scene sources + the scene copy + scene LFOs (step 1 of the block order).
    pub fn begin_block(&mut self, cx: &SceneCtx, bank: &mut FormulaBank) {
        let s = self.index;
        for c in &mut self.ctl {
            c.process(cx.fs);
        }
        self.sources[4] = [self.ctl[CTL_AT].value(); 3];
        self.sources[5] = [self.ctl[CTL_PB].value(); 3];
        self.sources[6] = [self.ctl[CTL_MW].value(); 3];
        self.sources[35] = [self.ctl[CTL_BREATH].value(); 3];
        self.sources[36] = [self.ctl[CTL_EXPR].value(); 3];
        self.sources[37] = [self.ctl[CTL_SUS].value(); 3];
        for m in 0..8 {
            self.sources[7 + m] = [cx.macros[m]; 3];
        }
        // Scene copy with last block's scene LFO outputs.
        for (e, b) in self.eff.iter_mut().zip(cx.prog.scene_base(s)) {
            *e = b;
        }
        let src = self.sources;
        let lfo16 = self.slfo16;
        apply_routes_with(&cx.prog.scene_routes[s], &mut self.eff[..], |sr, i| if (23..=28).contains(&sr) && i >= 3 { lfo16[sr as usize - 23][(i as usize).min(15)] } else { src[sr as usize][(i as usize).min(2)] });
        let eff = *self.eff;
        let macros: [f32; 8] = std::array::from_fn(|m| self.sources[7 + m][0]);
        for l in 0..6 {
            let li = 6 + l;
            let lp = super::convert::lfo(cx.prog, s, li, &eff);
            self.lfos[l].process_block(&lp, &cx.mod_ctx, &cx.prog.stepseq[s][li], &cx.prog.msegs[s][li]);
            if let Some(k) = self.slfo_k[l] {
                let inp = super::formula::inputs(cx.prog, li, &eff, &self.lfos[l], &cx.mod_ctx, macros, None);
                bank.process(s, li, k, &inp, lp.magnitude, lp.unipolar, &mut self.lfos[l]);
            }
            self.sources[23 + l] = self.lfos[l].outputs();
            self.slfo16[l] = *self.lfos[l].formula_outputs();
        }
    }

    /// The voices into the 2x bus, then clip, decimate, low cut, clip.
    pub fn render(&mut self, cx: &SceneCtx, bank: &mut FormulaBank, hardclip: i32) {
        self.bus_l = [0.0; BLOCK_OS];
        self.bus_r = [0.0; BLOCK_OS];
        let scene_eff = *self.eff;
        let src = self.sources;
        let mut bl = [0.0f32; BLOCK_OS];
        let mut br = [0.0f32; BLOCK_OS];
        let mut active = 0;
        let mpe_range = self.mpe_bend_range;
        let lfo16 = self.slfo16;
        let vc = Self::voice_ctx(self.index, self.mpe, self.bend_semis(cx.prog), cx, &scene_eff, &src, &lfo16);
        for v in self.voices.iter_mut() {
            if !v.active() {
                continue;
            }
            if self.mpe {
                let ch = v.channel as usize & 15;
                v.mpe_bend = self.chan_bend[ch] * mpe_range;
                v.mpe_pressure = self.chan_pressure[ch];
                v.mpe_timbre = self.chan_timbre[ch];
            }
            v.poly_at = self.poly_at[v.channel as usize & 15][v.key.clamp(0, 127) as usize];
            if v.process(&vc, bank, &mut bl, &mut br) {
                active += 1;
            } else {
                v.state = VState::Free;
                v.release_formulas(self.index, bank);
            }
        }
        self.active_voices = active;
        self.bus_l = bl;
        self.bus_r = br;
        if self.polymode() == 5 && active == 0 {
            // Latch keeps a voice alive.
            let fake = SceneCtx { prog: cx.prog, fs: cx.fs, mod_ctx: cx.mod_ctx, character: cx.character, macros: cx.macros, polylimit: cx.polylimit, audio_in_os: None, tuning: cx.tuning, retune_all: cx.retune_all };
            self.note_on(&fake, bank, 0, 60, 100.0 / 127.0, 1, 0, 0);
        }
        let clip = |x: &mut [f32], mode: i32| match mode {
            1 => x.iter_mut().for_each(|s| *s = s.clamp(-8.0, 8.0)),
            2 => x.iter_mut().for_each(|s| *s = s.clamp(-1.0, 1.0)),
            _ => {}
        };
        clip(&mut self.bus_l, hardclip);
        clip(&mut self.bus_r, hardclip);
        let (bl, br) = (self.bus_l, self.bus_r);
        self.hb.decimate_into(&bl, &br, &mut self.out_l, &mut self.out_r);
        // Scene volume deactivated mutes the scene.
        if cx.prog.scene_value(self.index, sc::VOLUME).deactivated {
            self.out_l = [0.0; BLOCK];
            self.out_r = [0.0; BLOCK];
        }
        let lc = cx.prog.scene_value(self.index, sc::LOWCUT);
        if !lc.deactivated {
            let stages = (lc.deform.clamp(0, 3) + 1) as usize;
            let w = calc_omega(self.eff[sc::LOWCUT] / 12.0, cx.fs);
            for b in &mut self.lowcut[..stages] {
                b.hp(w, 0.4);
                b.process(&mut self.out_l, &mut self.out_r);
            }
        }
        clip(&mut self.out_l, hardclip);
        clip(&mut self.out_r, hardclip);
    }
}
