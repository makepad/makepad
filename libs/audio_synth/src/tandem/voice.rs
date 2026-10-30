//! One Tandem voice: three oscillators with FM, the six-channel mixer with
//! ring combinators and noise, the filter block in one of eight
//! configurations (two filters, waveshaper, feedback), the VCA and pan,
//! two envelopes and six voice LFOs; per-block modulation into the voice's
//! own copy of the scene parameters. Runs 2x oversampled, 64 samples a
//! block.

use super::catalog::{sc, N_SCENE};
use super::program::{Program, N_SOURCES};
use crate::dsp::Rng;
use crate::kit::core::{amp_to_linear, db_to_linear, envelope_rate_linear, glide_exp, glide_log, megapan, softclip, BlockLine, BLOCK_OS};
use crate::kit::flt::{FilterUnit, TiltNoise};
use crate::kit::formula::VoiceInputs;
use crate::kit::mods::{Adsr, Lfo, ModCtx};
use super::formula::FormulaBank;
use crate::kit::tuning::Tuning;
use crate::kit::oscs::{Osc, OscBlock};
use crate::kit::shaper::Shaper;

/// Mirrors the reference: the waveshaper in Serial/Dual/Ring sees a 0.5-pole average of its input.
pub const QUIRK_WS_PREFILTER: bool = true;
/// Mirrors the reference: legacy noise is computed at the host rate and held for two oversampled samples.
pub const QUIRK_LEGACY_NOISE_HOST_RATE: bool = true;

/// Scene-level source values a voice reads (3 outputs each).
pub type Sources = [[f32; 3]; N_SOURCES];

/// What the scene hands each voice per block.
pub struct VoiceCtx<'a> {
    pub prog: &'a Program,
    pub scene: usize,
    pub scene_eff: &'a [f32; N_SCENE],
    pub scene_src: &'a Sources,
    /// Scene LFOs' 16 outputs (formula sources past index 2).
    pub scene_lfo16: &'a [[f32; 16]; 6],
    pub fs_os: f32,
    pub mod_ctx: ModCtx,
    /// Pitch bend in semitones (scene), already ranged.
    pub bend_semis: f32,
    pub character: u8,
    pub mpe: bool,
    pub audio_in: Option<(&'a [f32; 64], &'a [f32; 64])>,
    /// A tuning that retunes every pitch (oscillator notes, cutoffs whose
    /// extend flag says "follow the tuning").
    pub retune_all: Option<&'a Tuning>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VState {
    #[default]
    Free,
    Playing,
}

pub struct Voice {
    pub state: VState,
    pub gate: bool,
    pub key: i32,
    pub channel: u8,
    pub velocity: f32,
    pub rel_velocity: f32,
    /// Allocation order (older = smaller).
    pub age: u64,
    /// Order of release (for stealing), 0 while gated.
    pub release_age: u64,
    pub fast_release: bool,
    pub sustained: bool,
    pub sostenuto: bool,
    // Pitch.
    pub porta_phase: f32,
    pub k_src: f32,
    pub pkey: f32,
    /// The (tuned) note of `key`: what the glide heads for.
    pub note: f32,
    pub last_gliss: i32,
    pub mpe_bend: f32,
    pub mpe_pressure: f32,
    pub mpe_timbre: f32,
    pub poly_at: f32,
    pub keytrack_out: f32,
    rand: [f32; 4],
    alt: [f32; 2],
    eff: Box<[f32; N_SCENE]>,
    oscs: [Osc; 3],
    osc_buf: [[[f32; BLOCK_OS]; 2]; 3],
    pub aeg: Adsr,
    pub feg: Adsr,
    lfos: Vec<Lfo>,
    f1: [FilterUnit; 2],
    f2: [FilterUnit; 2],
    ws: [Shaper; 2],
    ws_avg: f32,
    fb: [f32; 2],
    lines: Lines,
    noise_y: [[f32; 2]; 2],
    noise_hold: [f32; 2],
    tilt: TiltNoise,
    rng: Rng,
    started: bool,
    /// Formula instances of the voice LFOs (keys into the engine's bank).
    fk: [Option<usize>; 6],
}

#[derive(Default)]
struct Lines {
    level: [BlockLine; 6],
    pfg: BlockLine,
    gain: BlockLine,
    fb: BlockLine,
    mix1: BlockLine,
    mix2: BlockLine,
    drive: BlockLine,
    out: [BlockLine; 4],
}

/// What a note start brings.
pub struct NoteStart {
    pub key: i32,
    /// The note the key plays (the key in standard tuning; its tuned
    /// note number when a tuning retunes the keys).
    pub note: f32,
    pub channel: u8,
    pub velocity: f32,
    pub age: u64,
    /// Glide from this key (None: no glide).
    pub glide_from: Option<f32>,
    pub rand: [f32; 4],
    pub alt: [f32; 2],
    /// Envelopes continue from their level (reclaimed voice).
    pub from_current: bool,
}

impl Voice {
    pub fn new(seed: u32) -> Voice {
        Voice {
            state: VState::Free,
            gate: false,
            key: 60,
            channel: 0,
            velocity: 0.0,
            rel_velocity: 0.0,
            age: 0,
            release_age: 0,
            fast_release: false,
            sustained: false,
            sostenuto: false,
            porta_phase: 1.0,
            k_src: 60.0,
            pkey: 60.0,
            note: 60.0,
            last_gliss: 60,
            mpe_bend: 0.0,
            mpe_pressure: 0.0,
            mpe_timbre: 0.0,
            poly_at: 0.0,
            keytrack_out: 0.0,
            rand: [0.0; 4],
            alt: [0.0; 2],
            eff: Box::new([0.0; N_SCENE]),
            oscs: std::array::from_fn(|i| Osc::new(0, seed.wrapping_mul(7).wrapping_add(i as u32))),
            osc_buf: [[[0.0; BLOCK_OS]; 2]; 3],
            aeg: Adsr::default(),
            feg: Adsr::default(),
            lfos: (0..6).map(|i| Lfo::new(seed.wrapping_add(101 * i as u32 + 1), false)).collect(),
            f1: Default::default(),
            f2: Default::default(),
            ws: Default::default(),
            ws_avg: 0.0,
            fb: [0.0; 2],
            lines: Lines::default(),
            noise_y: [[0.0; 2]; 2],
            noise_hold: [0.0; 2],
            tilt: TiltNoise::new(seed ^ 0x5eed),
            rng: Rng::new(seed ^ 0x9e37_79b9),
            started: false,
            fk: [None; 6],
        }
    }

    pub fn active(&self) -> bool {
        self.state == VState::Playing
    }

    /// Make the oscillators match the patch's types (allocates on a change:
    /// called when a program is installed, not per note).
    pub fn set_program(&mut self, prog: &Program, s: usize, seed: u32, fs_host: f32) {
        for o in 0..3 {
            let ty = prog.scene_value(s, sc::OSC + sc::OSC_STRIDE * o).i().clamp(0, 11) as u8;
            if self.oscs[o].ty() != ty {
                self.oscs[o] = Osc::new(ty, seed.wrapping_add(o as u32 * 31));
            }
        }
        // Filter state a type needs (the comb's line) is made here, not per block.
        let t1 = prog.scene_value(s, sc::F1_TYPE).i().clamp(0, 35) as u8;
        let t2 = prog.scene_value(s, sc::F2_TYPE).i().clamp(0, 35) as u8;
        for f in &mut self.f1 {
            f.prepare(t1);
        }
        for f in &mut self.f2 {
            f.prepare(t2);
        }
        self.tilt.set_sample_rate(fs_host);
        self.state = VState::Free;
    }

    /// The voice pitch P (ARCHITECTURE 3.3): the glided key plus bend,
    /// scene pitch and octave (and the MPE bend).
    fn voice_pitch(&self, cx: &VoiceCtx, e: &[f32; N_SCENE]) -> f32 {
        let pitch_v = cx.prog.scene_value(cx.scene, sc::PITCH);
        let sp = cx.bend_semis + e[sc::PITCH] * if pitch_v.extend { 12.0 } else { 1.0 } + 12.0 * e[sc::OCTAVE].round();
        let mpe = if cx.mpe { self.mpe_bend } else { 0.0 };
        self.pkey + mpe + sp
    }

    pub fn start(&mut self, n: &NoteStart, cx: &VoiceCtx, bank: &mut FormulaBank) {
        self.state = VState::Playing;
        self.gate = true;
        self.key = n.key;
        self.channel = n.channel;
        self.velocity = n.velocity;
        self.rel_velocity = 0.0;
        self.age = n.age;
        self.release_age = 0;
        self.fast_release = false;
        self.sustained = false;
        self.rand = n.rand;
        self.alt = n.alt;
        match n.glide_from {
            Some(k) if (k - n.note).abs() > 1e-6 => {
                self.k_src = k;
                self.porta_phase = 0.0;
            }
            _ => {
                self.k_src = n.note;
                self.porta_phase = 1.0;
            }
        }
        self.note = n.note;
        self.pkey = if self.porta_phase < 1.0 { self.k_src } else { n.note };
        self.last_gliss = self.pkey.round() as i32;
        let s = cx.scene;
        // The first copy: scene values + voice routes without LFO sources (they have not run yet).
        self.eff.copy_from_slice(cx.scene_eff);
        self.keytrack_out = (self.voice_pitch(cx, cx.scene_eff) - cx.scene_eff[sc::KTRKROOT]) / 12.0;
        let mut eff = *self.eff;
        apply_routes_with(&cx.prog.voice_routes[s], &mut eff, |src, index| self.source(cx, src, index, false));
        *self.eff = eff;
        // EGs and LFOs attack.
        let (ap, fp) = (super::convert::adsr(cx.prog, s, 0, &self.eff[..]), super::convert::adsr(cx.prog, s, 1, &self.eff[..]));
        let from_current = n.from_current;
        self.aeg.attack(&ap, from_current);
        self.feg.attack(&fp, from_current);
        for l in 0..6 {
            let lp = super::convert::lfo(cx.prog, s, l, &self.eff[..]);
            self.lfos[l].attack(&lp, &cx.mod_ctx, &cx.prog.stepseq[s][l], &cx.prog.msegs[s][l]);
            if bank.of_lfo[s][l].is_some() {
                if self.fk[l].is_none() {
                    self.fk[l] = bank.acquire(s, l);
                }
                if let Some(k) = self.fk[l] {
                    let inp = super::formula::inputs(cx.prog, l, &self.eff[..], &self.lfos[l], &cx.mod_ctx, self.macros(cx), Some(self.voice_inputs()));
                    bank.start(s, l, k, &inp);
                }
            }
        }
        if !from_current {
            for f in self.f1.iter_mut().chain(self.f2.iter_mut()) {
                f.reset();
            }
            for w in &mut self.ws {
                w.reset();
            }
            self.ws_avg = 0.0;
            self.fb = [0.0; 2];
            self.lines = Lines::default();
            self.noise_y = [[0.0; 2]; 2];
            self.tilt.start(cx.scene_eff[sc::NOISECOL].clamp(-1.0, 1.0) * 6.0, self.wide(cx));
        }
        self.started = false;
        let pitch_note = self.osc_notes(cx);
        let drift_nz = cx.prog.scene_value(s, sc::DRIFT).extend;
        for o in 0..3 {
            let slots = self.osc_slots(cx, o);
            let blk = self.osc_block(cx, o, pitch_note[o], &slots, &[0.0; BLOCK_OS]);
            self.oscs[o].init(&blk, drift_nz);
        }
    }

    /// Key-up (or a mono hand-off): release envelopes and LFO envelopes.
    pub fn release(&mut self, rel_vel: f32, order: u64) {
        if !self.gate {
            return;
        }
        self.gate = false;
        self.rel_velocity = rel_vel;
        self.release_age = order;
        self.aeg.release();
        self.feg.release();
        for l in &mut self.lfos {
            l.release();
        }
    }

    /// Steal: the fast ("uber") release.
    pub fn uber_release(&mut self, order: u64) {
        if !self.fast_release {
            self.gate = false;
            if self.release_age == 0 {
                self.release_age = order;
            }
            self.fast_release = true;
            self.aeg.uber_release();
            self.feg.uber_release();
        }
    }

    /// Legato re-pitch (single-trigger modes): glide from where it is.
    pub fn legato(&mut self, key: i32, note: f32, velocity: f32, glide: bool) {
        self.k_src = self.pkey;
        self.key = key;
        self.note = note;
        self.velocity = velocity;
        self.porta_phase = if glide { 0.0 } else { 1.0 };
        if !glide {
            self.pkey = note;
        }
        self.gate = true;
        self.release_age = 0;
    }

    fn macros(&self, cx: &VoiceCtx) -> [f32; 8] {
        std::array::from_fn(|m| cx.scene_src[7 + m][0])
    }

    fn voice_inputs(&self) -> VoiceInputs {
        VoiceInputs {
            channel: self.channel as f64,
            key: self.key as f64,
            velocity: (self.velocity * 127.0) as f64,
            rel_velocity: (self.rel_velocity * 127.0) as f64,
            voice_id: self.age as f64,
            tuned_key: self.pkey as f64,
            released: !self.gate,
            poly_at: self.poly_at,
            mpe_bend: self.mpe_bend,
            mpe_timbre: self.mpe_timbre,
            mpe_pressure: self.mpe_pressure,
        }
    }

    /// Give the voice's formula instances back (its end).
    pub fn release_formulas(&mut self, scene: usize, bank: &mut FormulaBank) {
        for l in 0..6 {
            if let Some(k) = self.fk[l].take() {
                bank.release(scene, l, k);
            }
        }
    }

    /// Forget instances of a bank that was replaced.
    pub fn drop_formulas(&mut self) {
        self.fk = [None; 6];
    }

    pub fn kill(&mut self) {
        self.state = VState::Free;
        self.gate = false;
    }

    fn wide(&self, cx: &VoiceCtx) -> bool {
        cx.scene_eff[sc::FB_CONFIG].round() as i32 == 7
    }

    /// One source output (index 0..15) as the voice sees it.
    fn source(&self, cx: &VoiceCtx, src: u8, index: u8, lfos_ran: bool) -> f32 {
        let i = index as usize;
        let i3 = i.min(2);
        match src {
            1 => self.velocity,
            2 => self.keytrack_out,
            3 => self.poly_at,
            4 if cx.mpe => self.mpe_pressure,
            15 => self.aeg.output(),
            16 => self.feg.output(),
            17..=22 if !lfos_ran => 0.0,
            17..=22 => {
                let l = &self.lfos[src as usize - 17];
                if i < 3 { l.outputs()[i] } else { l.formula_outputs().get(i).copied().unwrap_or(0.0) }
            }
            23..=28 if i >= 3 => cx.scene_lfo16[src as usize - 23].get(i).copied().unwrap_or(0.0),
            29 if cx.mpe => self.mpe_timbre,
            30 => self.rel_velocity,
            31 => if i == 1 { self.rand[1] } else { self.rand[0] },
            32 => if i == 1 { self.rand[3] } else { self.rand[2] },
            33 => self.alt[0],
            34 => self.alt[1],
            _ => cx.scene_src[src as usize][i3],
        }
    }

    fn osc_slots(&self, _cx: &VoiceCtx, o: usize) -> [crate::kit::core::SlotValue; 7] {
        let b = sc::OSC + sc::OSC_STRIDE * o + 3;
        let mut slots = [crate::kit::core::SlotValue::default(); 7];
        for k in 0..7 {
            let mut v = *_cx.prog.scene_value(_cx.scene, b + k);
            v.v = self.eff[b + k];
            slots[k] = v;
        }
        slots
    }

    /// Note numbers for each oscillator (ARCHITECTURE 3.3).
    fn osc_notes(&self, cx: &VoiceCtx) -> [f32; 3] {
        let e = &self.eff;
        let p = cx.prog;
        let s = cx.scene;
        let pitch_v = p.scene_value(s, sc::PITCH);
        let sp = cx.bend_semis + e[sc::PITCH] * if pitch_v.extend { 12.0 } else { 1.0 } + 12.0 * e[sc::OCTAVE].round();
        let mpe = if cx.mpe { self.mpe_bend } else { 0.0 };
        let pv = self.pkey + mpe + sp;
        std::array::from_fn(|o| {
            let b = sc::OSC + sc::OSC_STRIDE * o;
            let keytrack = e[b + 10] > 0.5;
            let mut n = if keytrack { pv } else { 60.0 + sp } + 12.0 * e[b + 1].round();
            let opv = p.scene_value(s, b + 2);
            let x = e[b + 2] * if opv.extend { 12.0 } else { 1.0 };
            if opv.absolute {
                let hz = crate::kit::core::midi_hz(n) + 10.0 * x;
                let ratio = (hz / crate::kit::core::midi_hz(n)).max(0.01);
                n += 12.0 * ratio.log2();
            } else {
                n += x;
            }
            match cx.retune_all {
                Some(t) => t.retune(n),
                None => n,
            }
        })
    }

    fn osc_block<'b>(&self, cx: &'b VoiceCtx, o: usize, pitch: f32, slots: &'b [crate::kit::core::SlotValue; 7], fm: &'b [f32; BLOCK_OS]) -> OscBlock<'b> {
        let e = &self.eff;
        let fm_switch = e[sc::FM_SWITCH].round() as i32;
        let fm_on = match o {
            0 => (1..=3).contains(&fm_switch),
            1 => fm_switch == 2,
            _ => false,
        };
        let b = sc::OSC + sc::OSC_STRIDE * o;
        OscBlock {
            pitch,
            drift: e[sc::DRIFT] * if cx.prog.scene_value(cx.scene, sc::DRIFT).extend { 1.0 } else { 1.0 },
            stereo: self.wide(cx),
            fm: fm_on,
            fmdepth: db_to_linear(e[sc::FM_DEPTH]),
            fm_buf: fm,
            slots,
            retrigger: e[b + 11] > 0.5,
            character: cx.character,
            gate: self.gate,
            table: cx.prog.tables[cx.scene][o].as_ref(),
            extra: &cx.prog.osc_extra[cx.scene][o],
            audio_in: cx.audio_in,
            fs_os: cx.fs_os,
            tsr: cx.mod_ctx.tsr,
        }
    }

    /// One block: modulation, pitch, oscillators, mixer, filter block, out
    /// (added into the scene bus). Returns false when the voice ended.
    pub fn process(&mut self, cx: &VoiceCtx, bank: &mut FormulaBank, bus_l: &mut [f32; BLOCK_OS], bus_r: &mut [f32; BLOCK_OS]) -> bool {
        if self.state != VState::Playing {
            return false;
        }
        let s = cx.scene;
        let prog = cx.prog;
        // 1. LFOs (LFO 1 always, others if read), with last block's parameter copy.
        for l in 0..6 {
            if l == 0 || prog.reads[s][17 + l] {
                let lp = super::convert::lfo(prog, s, l, &self.eff[..]);
                self.lfos[l].process_block(&lp, &cx.mod_ctx, &prog.stepseq[s][l], &prog.msegs[s][l]);
                if let Some(k) = self.fk[l] {
                    let inp = super::formula::inputs(prog, l, &self.eff[..], &self.lfos[l], &cx.mod_ctx, self.macros(cx), Some(self.voice_inputs()));
                    bank.process(s, l, k, &inp, lp.magnitude, lp.unipolar, &mut self.lfos[l]);
                }
            }
        }
        // 2. LFO-requested EG retriggers.
        let (mut ra, mut rf) = (false, false);
        for l in 0..6 {
            let (a, f) = self.lfos[l].retrigger_requests();
            ra |= a;
            rf |= f;
        }
        // An LFO's EG retrigger starts from the current level for a reclaimed voice
        // (poly one-voice-per-key, or mono restart-from-latest), else from zero.
        let mode = self.eff[sc::POLYMODE].round() as i32;
        let np = &prog.nonparam;
        let keep_level = if mode == 0 { np.repeated_key[s] == 1 } else { np.mono_envelope[s] == 1 };
        if ra && self.gate {
            let ap = super::convert::adsr(prog, s, 0, &self.eff[..]);
            self.aeg.retrigger(&ap, keep_level);
        }
        if rf && self.gate {
            let fp = super::convert::adsr(prog, s, 1, &self.eff[..]);
            self.feg.retrigger(&fp, keep_level);
        }
        // 3. EGs.
        let ap = super::convert::adsr(prog, s, 0, &self.eff[..]);
        let fp = super::convert::adsr(prog, s, 1, &self.eff[..]);
        let aeg = self.aeg.process_block(&ap, &cx.mod_ctx);
        let feg = self.feg.process_block(&fp, &cx.mod_ctx);
        // 4. The voice copy.
        self.eff.copy_from_slice(cx.scene_eff);
        let mut eff = *self.eff;
        apply_routes_with(&prog.voice_routes[s], &mut eff, |src, index| self.source(cx, src, index, true));
        *self.eff = eff;
        if cx.mpe {
            // Channel pressure scene-list routes apply per voice under MPE.
            for r in &prog.scene_routes[s] {
                if r.src == 4 {
                    self.eff[r.dst as usize] += r.depth * (self.mpe_pressure - cx.scene_src[4][0]);
                }
            }
        }
        // 5. Portamento and pitch.
        self.portamento(cx);
        let e: [f32; N_SCENE] = *self.eff;
        self.keytrack_out = (self.voice_pitch(cx, &e) - e[sc::KTRKROOT]) / 12.0;
        let notes = self.osc_notes(cx);
        // 6. Oscillators 3, 2, 1 (FM sources first).
        let wide = self.wide(cx);
        let fm_switch = e[sc::FM_SWITCH].round() as i32;
        let solo_any = (0..6).any(|c| e[sc::MIX + 4 * c + 2] > 0.5);
        let audible = |c: usize| if solo_any { e[sc::MIX + 4 * c + 2] > 0.5 } else { e[sc::MIX + 4 * c + 1] < 0.5 };
        let fm_on = fm_switch != 0 && (!solo_any || e[sc::MIX + 2] > 0.5);
        let need = [
            audible(0) || audible(3),
            audible(1) || audible(3) || audible(4) || (fm_on && (1..=3).contains(&fm_switch)),
            audible(2) || audible(4) || (fm_on && (fm_switch == 2 || fm_switch == 3)),
        ];
        let zero = [0.0f32; BLOCK_OS];
        for o in (0..3).rev() {
            if !need[o] {
                self.osc_buf[o] = [[0.0; BLOCK_OS]; 2];
                continue;
            }
            let mut fm = zero;
            if fm_on {
                match (o, fm_switch) {
                    (0, 1) | (0, 2) => fm = self.osc_buf[1][0],
                    (0, 3) => {
                        for i in 0..BLOCK_OS {
                            fm[i] = self.osc_buf[1][0][i] + self.osc_buf[2][0][i];
                        }
                    }
                    (1, 2) => fm = self.osc_buf[2][0],
                    _ => {}
                }
            }
            let slots = self.osc_slots(cx, o);
            let mut blk = self.osc_block(cx, o, notes[o], &slots, &fm);
            blk.fm = blk.fm && fm_on;
            let mut l = [0.0f32; BLOCK_OS];
            let mut r = [0.0f32; BLOCK_OS];
            self.oscs[o].process(&blk, &mut l, &mut r);
            if !wide {
                r = l;
            }
            self.osc_buf[o] = [l, r];
        }
        // 7. Mixer.
        let e: [f32; N_SCENE] = *self.eff;
        let config = e[sc::FB_CONFIG].round().clamp(0.0, 7.0) as i32;
        let serial = config <= 2;
        let mut in_l = [0.0f32; BLOCK_OS];
        let mut in_r = [0.0f32; BLOCK_OS];
        let mut ring = [[[0.0f32; BLOCK_OS]; 2]; 2];
        for (k, (a, b)) in [(0usize, 1usize), (1, 2)].iter().enumerate() {
            let c = 3 + k;
            if !audible(c) {
                continue;
            }
            let mode = prog.scene_value(s, sc::MIX + 4 * c).deform;
            for ch in 0..2 {
                for i in 0..BLOCK_OS {
                    ring[k][ch][i] = combine(mode, self.osc_buf[*a][ch][i], self.osc_buf[*b][ch][i]);
                }
            }
        }
        let mut noise = [[0.0f32; BLOCK_OS]; 2];
        if audible(5) {
            self.noise(cx, &mut noise, wide);
        }
        for c in 0..6 {
            let level = amp_to_linear(e[sc::MIX + 4 * c]);
            self.lines.level[c].set(level, BLOCK_OS);
            if !audible(c) {
                // Keep the line moving so an unmute ramps from its value.
                continue;
            }
            let mut route = e[sc::MIX + 4 * c + 3].round() as i32;
            if serial && route == 1 {
                route = 0;
            }
            let src: &[[f32; BLOCK_OS]; 2] = match c {
                0..=2 => &self.osc_buf[c],
                3 | 4 => &ring[c - 3],
                _ => &noise,
            };
            let mut line = self.lines.level[c];
            for i in 0..BLOCK_OS {
                // Noise levels are cubed inside the tilt path; legacy noise uses the line too.
                let g = line.next();
                if route < 2 {
                    in_l[i] += src[0][i] * g;
                }
                if route > 0 {
                    in_r[i] += src[1][i] * g;
                }
            }
            self.lines.level[c] = line;
        }
        let pfg = db_to_linear(e[sc::LEVEL_PFG]);
        self.lines.pfg.set(pfg, BLOCK_OS);
        for i in 0..BLOCK_OS {
            let g = self.lines.pfg.next();
            in_l[i] *= g;
            in_r[i] *= g;
        }
        // 8. Filter block controls.
        let fenv = feg;
        let kt = self.voice_pitch(cx, &e) - e[sc::KTRKROOT];
        let mut cut1 = e[sc::F1_CUTOFF] + e[sc::F1_KEYTRACK] * kt + e[sc::F1_ENVMOD] * fenv;
        let mut cut2 = e[sc::F2_CUTOFF] + e[sc::F2_KEYTRACK] * kt + e[sc::F2_ENVMOD] * fenv;
        // "Apply tuning" (cutoff extend): the cutoff note (vs A440) through the scale.
        if let Some(t) = cx.retune_all {
            if prog.scene_value(s, sc::F1_CUTOFF).extend {
                cut1 = t.retune(cut1 + 69.0) - 69.0;
            }
            if prog.scene_value(s, sc::F2_CUTOFF).extend {
                cut2 = t.retune(cut2 + 69.0) - 69.0;
            }
        }
        if e[sc::F2_IS_OFFSET] > 0.5 {
            cut2 += cut1;
        }
        let reso1 = e[sc::F1_RESO];
        let reso2 = if e[sc::F2_LINK_RESO] > 0.5 { reso1 } else { e[sc::F2_RESO] };
        let t1v = prog.scene_value(s, sc::F1_TYPE);
        let t2v = prog.scene_value(s, sc::F2_TYPE);
        let ty1 = if t1v.deactivated { 0 } else { e[sc::F1_TYPE].round().clamp(0.0, 35.0) as u8 };
        let ty2 = if t2v.deactivated { 0 } else { e[sc::F2_TYPE].round().clamp(0.0, 35.0) as u8 };
        let st1 = e[sc::F1_SUBTYPE].round().max(0.0) as u8;
        let st2 = e[sc::F2_SUBTYPE].round().max(0.0) as u8;
        let comb = prog.correct_comb;
        for f in &mut self.f1 {
            f.set_block(ty1, st1, cut1, reso1, cx.fs_os, comb);
        }
        for f in &mut self.f2 {
            f.set_block(ty2, st2, cut2, reso2, cx.fs_os, comb);
        }
        let a_on = ty1 != 0;
        let b_on = ty2 != 0;
        let wsv = prog.scene_value(s, sc::WS_TYPE);
        let ws_ty = if wsv.deactivated { 0 } else { e[sc::WS_TYPE].round().clamp(0.0, 42.0) as u8 };
        let ws_on = ws_ty != 0;
        for w in &mut self.ws {
            w.begin_block();
        }
        let vel = self.velocity;
        let gain = db_to_linear(e[sc::VCA_LEVEL] + e[sc::VCA_VELSENSE] * (1.0 - vel)) * aeg;
        let fbv = prog.scene_value(s, sc::FEEDBACK);
        let fb = e[sc::FEEDBACK] * if fbv.extend { 4.0 } else { 1.0 };
        let bal = e[sc::F_BALANCE];
        let (mix1, mix2) = match config {
            3 | 4 | 5 => (0.5 - 0.5 * bal, 0.5 + 0.5 * bal),
            _ => ((1.0 - bal).min(1.0), (1.0 + bal).min(1.0)),
        };
        let wdv = prog.scene_value(s, sc::WS_DRIVE);
        let drive = db_to_linear(e[sc::WS_DRIVE] * if wdv.extend { 2.0 } else { 1.0 });
        let mut amp = 0.5 * amp_to_linear(e[sc::VOLUME]);
        if config == 7 {
            amp *= 2.0 / 3.0;
        } else if config == 5 {
            amp *= 4.0 / 3.0;
        }
        let pan1 = (e[sc::PAN]).clamp(-1.0, 1.0);
        let width = e[sc::WIDTH];
        let stereo_out = config == 5 || config == 7;
        let (p_a, p_b) = if stereo_out { (pan1 - width, pan1 + width) } else { (pan1, pan1) };
        let (al, ar) = megapan(p_a);
        let (bl, br) = megapan(p_b);
        let l = &mut self.lines;
        l.gain.set(gain, BLOCK_OS);
        l.fb.set(fb, BLOCK_OS);
        l.mix1.set(mix1, BLOCK_OS);
        l.mix2.set(mix2, BLOCK_OS);
        l.drive.set(drive, BLOCK_OS);
        l.out[0].set(amp * al, BLOCK_OS);
        l.out[1].set(amp * ar, BLOCK_OS);
        l.out[2].set(amp * bl, BLOCK_OS);
        l.out[3].set(amp * br, BLOCK_OS);
        if !self.started {
            // The first block starts on its values (no ramp from zero).
            let (g, f, m1, m2, d) = (gain, fb, mix1, mix2, drive);
            l.gain.snap(g);
            l.fb.snap(f);
            l.mix1.snap(m1);
            l.mix2.snap(m2);
            l.drive.snap(d);
            l.out[0].snap(amp * al);
            l.out[1].snap(amp * ar);
            l.out[2].snap(amp * bl);
            l.out[3].snap(amp * br);
            self.started = true;
        }
        // 9. Filter block per configuration (FILTERS.md section 4).
        for i in 0..BLOCK_OS {
            let g = self.lines.gain.next();
            let fbk = self.lines.fb.next();
            let m1 = self.lines.mix1.next();
            let m2 = self.lines.mix2.next();
            let d = self.lines.drive.next();
            let ol = self.lines.out[0].next();
            let or = self.lines.out[1].next();
            let o2l = self.lines.out[2].next();
            let o2r = self.lines.out[3].next();
            let (lx, rx) = (in_l[i], in_r[i]);
            let out;
            let mut out2 = (0.0f32, 0.0f32);
            match config {
                0..=2 => {
                    let inp = if config == 0 { lx } else { lx + softclip(fbk * self.fb[0]) };
                    let mut x = if a_on { self.f1[0].tick(inp) } else { inp };
                    if ws_on {
                        x = self.shape(0, ws_ty, x, d, true);
                    }
                    if a_on || ws_on {
                        x = inp * (1.0 - m1) + x * m1;
                    }
                    if config == 2 {
                        x *= g;
                        let o = x;
                        let mut y = x + rx;
                        if b_on {
                            y = self.f2[0].tick(y);
                        }
                        self.fb[0] = y;
                        out = (o * ol, o * or);
                    } else {
                        let mut y = x + rx;
                        if b_on {
                            y = self.f2[0].tick(y);
                        }
                        x = x * (1.0 - m2) + y * m2;
                        let o = x * g;
                        if config == 1 {
                            self.fb[0] = o;
                        }
                        out = (o * ol, o * or);
                    }
                }
                3 | 4 | 6 => {
                    let fbs = softclip(fbk * self.fb[0]);
                    let mut x = lx + fbs;
                    let mut y = rx + fbs;
                    if a_on {
                        x = self.f1[0].tick(x);
                    }
                    if config == 4 && ws_on {
                        x = self.shape(0, ws_ty, x, d, true);
                    }
                    if b_on {
                        y = self.f2[0].tick(y);
                    }
                    x = if config == 6 { ((1.0 - m1) * y + m1 * x) * ((1.0 - m2) * x + m2 * y) } else { x * m1 + y * m2 };
                    if config != 4 && ws_on {
                        x = self.shape(0, ws_ty, x, d, true);
                    }
                    let o = x * g;
                    self.fb[0] = o;
                    out = (o * ol, o * or);
                }
                5 => {
                    let fbs = softclip(fbk * self.fb[0]);
                    let mut x = lx + fbs;
                    let mut y = rx + fbs;
                    if a_on {
                        x = self.f1[0].tick(x);
                    }
                    if b_on {
                        y = self.f2[0].tick(y);
                    }
                    if ws_on {
                        x = self.ws[0].tick(ws_ty, x, d);
                        y = self.ws[1].tick(ws_ty, y, d);
                    }
                    x *= m1 * g;
                    y *= m2 * g;
                    self.fb[0] = x + y;
                    out = (x * ol, x * or);
                    out2 = (y * o2l, y * o2r);
                }
                _ => {
                    let xin = lx + softclip(fbk * self.fb[0]);
                    let yin = rx + softclip(fbk * self.fb[1]);
                    let (mut x, mut y) = (xin, yin);
                    if a_on {
                        x = self.f1[0].tick(x);
                        y = self.f1[1].tick(y);
                    }
                    if ws_on {
                        x = self.ws[0].tick(ws_ty, x, d);
                        y = self.ws[1].tick(ws_ty, y, d);
                    }
                    if a_on || ws_on {
                        x = xin * (1.0 - m1) + x * m1;
                        y = yin * (1.0 - m1) + y * m1;
                    }
                    if b_on {
                        let z = self.f2[0].tick(x);
                        let v = self.f2[1].tick(y);
                        x = x * (1.0 - m2) + z * m2;
                        y = y * (1.0 - m2) + v * m2;
                    }
                    x *= g;
                    y *= g;
                    self.fb = [x, y];
                    out = (x * ol, x * or);
                    out2 = (y * o2l, y * o2r);
                }
            }
            bus_l[i] += out.0 + out2.0;
            bus_r[i] += out.1 + out2.1;
        }
        // 10. The voice ends when its amp EG is idle.
        if self.aeg.idle() {
            self.state = VState::Free;
            return false;
        }
        true
    }

    #[inline]
    fn shape(&mut self, ch: usize, ty: u8, x: f32, d: f32, prefilter: bool) -> f32 {
        if prefilter && QUIRK_WS_PREFILTER {
            self.ws_avg = 0.5 * (self.ws_avg + x);
            self.ws[ch].tick(ty, self.ws_avg, d)
        } else {
            self.ws[ch].tick(ty, x, d)
        }
    }

    fn portamento(&mut self, cx: &VoiceCtx) {
        let s = cx.scene;
        let k = self.note;
        let p = self.eff[sc::PORTAMENTO];
        let opts = cx.prog.porta[s];
        if self.porta_phase < 1.0 && p > -8.0 {
            let mut rate = envelope_rate_linear(p.min(4.0), cx.mod_ctx.block_secs);
            if cx.prog.scene_value(s, sc::PORTAMENTO).temposync {
                rate *= cx.mod_ctx.tsr;
            }
            if opts.const_rate {
                let dist = (k - self.k_src).abs().max(1e-3);
                rate *= 12.0 / dist;
            }
            self.porta_phase += rate;
        } else {
            self.porta_phase = 1.0;
        }
        if self.porta_phase < 1.0 {
            let ph = self.porta_phase;
            let c = match opts.curve {
                -1 => glide_log(ph),
                1 => glide_exp(ph),
                _ => ph,
            };
            let mut pk = (1.0 - c) * self.k_src + c * k;
            if opts.gliss {
                pk = pk.round();
                let r = pk as i32;
                if opts.retrigger && r != self.last_gliss && self.gate {
                    let ap = super::convert::adsr(cx.prog, s, 0, &self.eff[..]);
                    let fp = super::convert::adsr(cx.prog, s, 1, &self.eff[..]);
                    self.aeg.attack(&ap, false);
                    self.feg.attack(&fp, false);
                }
                self.last_gliss = r;
            }
            self.pkey = pk;
        } else {
            self.pkey = k;
        }
    }

    fn noise(&mut self, cx: &VoiceCtx, out: &mut [[f32; BLOCK_OS]; 2], wide: bool) {
        let e = &self.eff;
        let v = cx.prog.scene_value(cx.scene, sc::NOISECOL);
        let stereo = wide && v.deform & 1 == 0;
        let c = e[sc::NOISECOL].clamp(-1.0, 1.0);
        if v.deform & 2 != 0 {
            // Tilt noise: the bank applies the level cube itself; give it unity here and
            // the mixer line applies the cubed level as for other sources.
            let (l, r) = out.split_at_mut(1);
            self.tilt.block(c, 1.0, stereo, &mut l[0], &mut r[0]);
            return;
        }
        let w = 0.8 * c.abs();
        let w2 = 2.0 * w - w * w;
        let sgn = if c < 0.0 { -w2 } else { w2 };
        let m = 1.0 / (1.0 - w2).max(1e-6).sqrt();
        for ch in 0..2 {
            if ch == 1 && !stereo {
                out[1] = out[0];
                break;
            }
            let [mut y1, mut y2] = self.noise_y[ch];
            let mut i = 0;
            while i < BLOCK_OS {
                let r = self.rng.bipolar();
                y2 = r * (1.0 - w2) - sgn * y2;
                y1 = y2 * (1.0 - w2) - sgn * y1;
                let x = y1 * m;
                out[ch][i] = x;
                if QUIRK_LEGACY_NOISE_HOST_RATE && i + 1 < BLOCK_OS {
                    out[ch][i + 1] = x;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            self.noise_y[ch] = [y1, y2];
            self.noise_hold[ch] = y1 * m;
        }
    }
}

/// `eff[dst] += depth * source(src, index)` for every route.
pub fn apply_routes_with(routes: &[super::program::Route], eff: &mut [f32], source: impl Fn(u8, u8) -> f32) {
    for r in routes {
        let v = source(r.src, r.index);
        if let Some(d) = eff.get_mut(r.dst as usize) {
            *d += r.depth * v;
        }
    }
}

/// The ring-mod "combinator" modes (ARCHITECTURE 4.2).
#[inline]
pub fn combine(mode: i32, a: f32, b: f32) -> f32 {
    let mx = a.max(b);
    let c0 = mx.min(-a.min(b));
    match mode {
        1 => c0,
        2 => {
            let v = mx;
            let c = v.min(-a.min(b));
            v.min(-c.min(v))
        }
        3 => {
            let v = mx;
            let c = v.min(-a.min(b));
            a.min(-c.min(v))
        }
        4 => (-c0.min(b)).min(a.max(-b)),
        5 => (-c0.min(b)).min(a.max(-c0)),
        6 => (-c0.min(-b)).min(a.max(b)),
        7 => (-c0.min(b)).min(a.max(c0)),
        8 => {
            let (p, m) = (a + b, a - b);
            p.max(m).min(-p.min(m))
        }
        9 => a - b.max(a.min(0.0)).min(a.max(0.0)),
        10 => {
            let (p, q) = (b + a, b - a);
            b.min(0f32.max(p.min(q)))
        }
        11 => {
            let (p, q) = (b + a, b - a);
            b.max(p).min(0f32.max(p.min(q)))
        }
        12 => {
            let (p, q) = (b + a, b - a);
            (-a).max(b).min(q).max(p.min(-p))
        }
        _ => a * b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combinators() {
        assert_eq!(combine(0, 0.5, -0.5), -0.25);
        // Continuous XOR: min(max(a,b), -min(a,b)).
        assert_eq!(combine(1, 0.3, -0.8), 0.3f32.min(0.8));
        assert_eq!(combine(8, 0.2, 0.1), (0.3f32).max(0.1).min(-(0.3f32).min(0.1)));
    }

    #[test]
    fn lfo_count() {
        assert_eq!(super::super::catalog::N_LFO, 12);
    }
}
