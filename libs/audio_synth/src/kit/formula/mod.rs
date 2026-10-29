//! Formula modulators: an LFO shape whose waveform is a small script stored
//! in the patch (MODULATION.md section 8, FORMULA-ADDENDUM.md). Scripts are
//! compiled, never interpreted: [`parse`] builds the syntax tree,
//! [`compile`] lowers it straight into AIR (the compute core's audio IR,
//! the same IR and native back end as Splash audio shaders), and the
//! [`Runner`] runs the compiled programs once per block.
//!
//! - [`Formula::compile`]: the script text to programs; an untranslatable
//!   script is a [`FormulaError`] with a line (show it on the modulator).
//! - [`Runner`]: one per formula text per engine (the reference shares a
//!   text's globals, tables and error state between every instance that
//!   runs it). It owns a fixed pool of instance states: voice LFOs and
//!   scene LFOs that use the text each hold one ([`Runner::acquire`]).
//! - [`Runner::start`] is the modulator's attack (`init(state)`),
//!   [`Runner::process`] one block (`process(state)`), giving
//!   [`FormulaIo`]: up to 16 outputs and the flags the LFO applies.
//!
//! Nothing in `start`/`process`/`release` allocates, locks or does I/O.
//! [`GUIDE`] describes the language for the Synth AI.

pub mod ast;
pub mod compile;
mod helpers;
pub mod parse;
#[cfg(test)]
mod reference;
#[cfg(test)]
mod tests;

use compile::*;
pub use crate::kit::mods::formula::{FormulaIo, FORMULA_OUTPUTS};
use makepad_script_compute::ir::{self, Program};
use std::sync::Arc;

/// The script a Formula LFO has when the patch stores none.
pub const DEFAULT_SCRIPT: &str = "function init(state)\n    return state\nend\n\nfunction process(state)\n    state.output = state.phase * 2 - 1\n    return state\nend\n";

/// What the Synth AI reads about writing formulas (also the schema's
/// description of the Formula shape).
pub const GUIDE: &str = "Formula LFO: a script with `function process(state) ... return state end` (once per 32-sample block) and an optional `function init(state) ... return state end` (once at each start). \
Set `state.output` to a number or to a table of up to 16 numbers {o1, o2, ...} (each output is a separate mod source), or return a number. \
Inputs on state: phase (0..1 in the cycle), cycle/intphase (cycle count), rate, deform, amplitude, startphase, delay attack hold decay sustain release (0..1, usable as free inputs with use_envelope = false), \
tempo, songpos, is_playing, macros[1..8], pb, cc_mw cc_breath cc_expr cc_sus chan_at, lowest_key highest_key latest_key, samplerate, block_size; voice LFOs also key velocity rel_velocity channel released poly_at mpe_bend mpe_timbre mpe_pressure. \
Flags on state: use_envelope, use_amplitude (apply the LFO EG / amplitude), clamp_output (clamp outputs 1..8 to -1..1, default true), retrigger_AEG/retrigger_FEG (restart the voice EGs this block). \
Language: numbers, booleans, nil, strings (constants; s:sub s:find s:len s:byte), tables {1, 2, x = 3} with t[k] and #t; local, if/elseif/else, while, repeat, numeric for, local functions (no recursion), `a and b or c`. \
Fields you add to state persist per instance; plain globals in process are shared by every voice running the same script. \
math.* (also without the prefix): sin cos tan asin acos atan atan2 exp log log10 pow sqrt abs floor ceil fmod modf min max sinh cosh tanh deg rad pi huge random randomseed clamp limit_range sgn sign parity rescale norm range note_to_freq freq_to_note lin_to_dB dB_to_lin gcd lcm random_normal random_poisson random_exponential random_bernoulli. \
Helpers: surge.mod.Slew:new{samplerate = state.samplerate, block_size = state.block_size} then s:run(x, up_rate, down_rate) (rates 0..1, 0.2 default); \
surge.mod.ClockDivider:new{numerator = n, denominator = d} then c:tick(state.intphase, state.phase) setting c.phase c.ibeat c.intphase c.newbeat; surge.mod.AHDEnvelope:new{a = , h = , d = } then e:at(phase). \
Not supported: '..' concatenation, pairs/ipairs, '...', tostring, setmetatable, table/bit/string.format, shared, FFT helpers, recursion, storing functions in tables.";

#[derive(Clone, Debug, PartialEq)]
pub struct FormulaError {
    /// 1-based line (0: the whole script).
    pub line: u32,
    pub message: String,
    /// A run-time error (the formula stopped) rather than a load error.
    pub runtime: bool,
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = if self.runtime { "formula stopped" } else { "formula cannot load" };
        if self.line > 0 {
            write!(f, "{} (line {}): {}", what, self.line, self.message)
        } else {
            write!(f, "{}: {}", what, self.message)
        }
    }
}

/// One compiled program and its native code (when the host has the native
/// back end).
struct Exec {
    prog: Program,
    #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
    native: Option<makepad_script_compute::native::Code>,
    scratch: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    /// The AIR interpreter.
    Interp,
    /// Native code from the shared back end.
    Native,
}

/// The record slot of every field the host writes.
#[derive(Clone, Copy)]
struct Slots {
    num: [u32; NUM_FIELDS.len()],
    boolean: [u32; BOOL_FIELDS.len()],
    output: u32,
    macros: u32,
}

/// A compiled formula text (immutable; share it between engines).
pub struct Formula {
    load: Exec,
    prologue: Exec,
    init: Exec,
    process: Exec,
    gc_clear: Exec,
    gc_mark: Exec,
    gc_finish: Exec,
    pool: Vec<u32>,
    state_words: usize,
    shared_words: usize,
    slots: Slots,
    backend: Backend,
}

impl Formula {
    /// Compiles with the fastest backend available.
    pub fn compile(src: &str) -> Result<Arc<Formula>, FormulaError> {
        Self::compile_with(src, Backend::Native)
    }

    pub fn compile_with(src: &str, backend: Backend) -> Result<Arc<Formula>, FormulaError> {
        let chunk = parse::parse(src).map_err(|e| FormulaError { line: e.line, message: e.msg, runtime: false })?;
        let c = compile::compile(&chunk).map_err(|e| FormulaError { line: e.line, message: e.msg, runtime: false })?;
        let lay = &c.layout;
        let regions = ir::Regions {
            ctx: lay.pool.len().max(1) as u32,
            state: lay.state_words,
            shared: lay.shared_words,
            frame: 0,
            shared_writable: true,
            bufs: Vec::new(),
            io: false,
        };
        let mk = |p: Program| -> Result<Exec, FormulaError> {
            ir::validate(&p, &regions).map_err(|e| FormulaError { line: 0, message: format!("internal compiler error: {}", e), runtime: false })?;
            #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
            let native = if backend == Backend::Native { makepad_script_compute::native::compile(&p) } else { None };
            let scratch = p.scratch_words();
            Ok(Exec {
                prog: p,
                #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
                native,
                scratch,
            })
        };
        let mut slots = Slots { num: [0; NUM_FIELDS.len()], boolean: [0; BOOL_FIELDS.len()], output: 0, macros: 0 };
        for (k, n) in NUM_FIELDS.iter().enumerate() {
            slots.num[k] = lay.record[*n];
        }
        for (k, n) in BOOL_FIELDS.iter().enumerate() {
            slots.boolean[k] = lay.record[*n];
        }
        slots.output = lay.record["output"];
        slots.macros = lay.record["macros"];
        let pool = if lay.pool.is_empty() { vec![0] } else { lay.pool.clone() };
        let (state_words, shared_words) = (lay.state_words as usize, lay.shared_words as usize);
        let f = Formula {
            load: mk(c.load)?,
            prologue: mk(c.prologue)?,
            init: mk(c.init)?,
            process: mk(c.process)?,
            gc_clear: mk(c.gc_clear)?,
            gc_mark: mk(c.gc_mark)?,
            gc_finish: mk(c.gc_finish)?,
            pool,
            state_words,
            shared_words,
            slots,
            backend,
        };
        let native = f.uses_native();
        Ok(Arc::new(Formula { backend: if native { Backend::Native } else { Backend::Interp }, ..f }))
    }

    /// The backend the programs run on.
    pub fn backend(&self) -> Backend {
        self.backend
    }

    fn uses_native(&self) -> bool {
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        {
            [&self.load, &self.prologue, &self.init, &self.process, &self.gc_clear, &self.gc_mark, &self.gc_finish].iter().all(|e| e.native.is_some())
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        {
            false
        }
    }

    fn scratch_words(&self) -> usize {
        [&self.load, &self.prologue, &self.init, &self.process, &self.gc_clear, &self.gc_mark, &self.gc_finish].iter().map(|e| e.scratch).max().unwrap_or(0)
    }

    /// The AIR process program (tools and tests).
    pub fn process_program(&self) -> &Program {
        &self.process.prog
    }
}

/// The values a formula sees (MODULATION.md 8.2, addendum section 4).
/// Engine values are single precision and arrive as their exact doubles.
#[derive(Clone, Debug)]
pub struct FormulaInputs {
    pub samplerate: f64,
    pub block_size: f64,
    /// delay, attack, hold, decay, sustain, release (0..1 slider positions).
    pub eg: [f32; 6],
    pub rate: f32,
    pub startphase: f32,
    pub deform: f32,
    pub amplitude: f32,
    pub tempo: f32,
    pub songpos: f64,
    pub is_playing: bool,
    pub pb_range_up: f32,
    pub pb_range_dn: f32,
    pub mpe_bendrange: f32,
    pub voice_count: f64,
    pub poly_limit: f64,
    pub scene_mode: f64,
    pub play_mode: f64,
    pub split_point: f64,
    pub lfo_id: f64,
    pub mpe_enabled: bool,
    pub is_rendering_to_ui: bool,
    /// The cycle count and the phase in it.
    pub intphase: f64,
    pub phase: f32,
    pub pb: f32,
    pub chan_at: f32,
    pub cc_mw: f32,
    pub cc_breath: f32,
    pub cc_expr: f32,
    pub cc_sus: f32,
    pub lowest_key: f64,
    pub highest_key: f64,
    pub latest_key: f64,
    pub macros: [f32; 8],
    /// Voice LFOs only.
    pub voice: Option<VoiceInputs>,
}

#[derive(Clone, Debug, Default)]
pub struct VoiceInputs {
    pub channel: f64,
    pub key: f64,
    pub velocity: f64,
    pub rel_velocity: f64,
    pub voice_id: f64,
    pub tuned_key: f64,
    pub released: bool,
    pub poly_at: f32,
    pub mpe_bend: f32,
    pub mpe_timbre: f32,
    pub mpe_pressure: f32,
}

impl Default for FormulaInputs {
    fn default() -> Self {
        FormulaInputs {
            samplerate: 48000.0,
            block_size: 32.0,
            eg: [0.0, 0.0, 0.0, 0.0, 1.0, 1.0],
            rate: 0.0,
            startphase: 0.0,
            deform: 0.0,
            amplitude: 1.0,
            tempo: 120.0,
            songpos: 0.0,
            is_playing: false,
            pb_range_up: 2.0,
            pb_range_dn: 2.0,
            mpe_bendrange: 48.0,
            voice_count: 0.0,
            poly_limit: 16.0,
            scene_mode: 0.0,
            play_mode: 0.0,
            split_point: 60.0,
            lfo_id: 0.0,
            mpe_enabled: false,
            is_rendering_to_ui: false,
            intphase: 0.0,
            phase: 0.0,
            pb: 0.0,
            chan_at: 0.0,
            cc_mw: 0.0,
            cc_breath: 0.0,
            cc_expr: 0.0,
            cc_sus: 0.0,
            lowest_key: 60.0,
            highest_key: 60.0,
            latest_key: 60.0,
            macros: [0.0; 8],
            voice: None,
        }
    }
}

struct Instance {
    state: Vec<u32>,
    active: bool,
    flags: FormulaIo,
}

/// A formula text's running state in one engine: the shared region and a
/// fixed pool of instance states.
pub struct Runner {
    formula: Arc<Formula>,
    shared: Vec<u32>,
    ctx: Vec<u32>,
    scratch: Vec<u32>,
    instances: Vec<Instance>,
    error: Option<(i32, u32)>,
}

/// The generator's state before any `randomseed`.
pub const RNG_START: [u32; 2] = [0x2545_F491, 0x9E37_79B9];

impl Runner {
    /// `capacity` instances (voice LFOs + scene LFOs that may run this text
    /// at once). Runs the top-level chunk. Allocates.
    pub fn new(formula: Arc<Formula>, capacity: usize) -> Runner {
        let mut r = Runner {
            shared: vec![0; formula.shared_words],
            ctx: formula.pool.clone(),
            scratch: vec![0; formula.scratch_words().max(1)],
            instances: (0..capacity).map(|_| Instance { state: vec![0; formula.state_words], active: false, flags: FormulaIo::default() }).collect(),
            formula,
            error: None,
        };
        r.shared[G_RNG as usize] = RNG_START[0];
        r.shared[G_RNG as usize + 1] = RNG_START[1];
        let f = r.formula.clone();
        r.run(&f.load, None);
        r.check_error();
        r
    }

    pub fn formula(&self) -> &Arc<Formula> {
        &self.formula
    }

    /// A free instance slot (None: all in use).
    pub fn acquire(&mut self) -> Option<usize> {
        let k = self.instances.iter().position(|i| !i.active)?;
        self.instances[k].active = true;
        Some(k)
    }

    /// The modulator stopped: its tables are collected later.
    pub fn release(&mut self, k: usize) {
        if let Some(i) = self.instances.get_mut(k) {
            i.active = false;
        }
    }

    /// The run-time error that stopped this formula (every instance then
    /// outputs 0 until the text changes).
    pub fn error(&self) -> Option<FormulaError> {
        self.error.map(|(code, line)| FormulaError { line, message: err::message(code).to_string(), runtime: true })
    }

    fn run(&mut self, e: &Exec, inst: Option<usize>) {
        let mut none: [u32; 1] = [0];
        let state: &mut [u32] = match inst {
            Some(k) => &mut self.instances[k].state,
            None => &mut none,
        };
        #[cfg(any(target_arch = "aarch64", target_arch = "x86_64"))]
        if let Some(code) = &e.native {
            let zeros = [0f32; 1];
            let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
            // SAFETY: the regions are the sizes the program was validated
            // against (ctx = pool, state = state_words, shared =
            // shared_words), n = 1.
            unsafe { code.run(&mut self.ctx, state, self.shared.as_mut_ptr(), [&zeros, &zeros], [&mut o0, &mut o1], 1) };
            return;
        }
        let zeros = [0f32; 1];
        let (mut o0, mut o1) = ([0f32; 1], [0f32; 1]);
        let mut mem = ir::Mem { ctx: &mut self.ctx, state, shared: ir::Shared::Write(&mut self.shared), bufs: &[] };
        let mut io = ir::Io { ins: [&zeros, &zeros], outs: [&mut o0, &mut o1] };
        ir::run(&e.prog, &mut self.scratch[..e.scratch.max(1)], &mut mem, &mut io, 1);
    }

    fn check_error(&mut self) {
        if self.error.is_none() && self.shared[G_ERR as usize] != 0 {
            self.error = Some((self.shared[G_ERR as usize] as i32, self.shared[G_ERR_LINE as usize]));
        }
    }

    fn put(state: &mut [u32], w: u32, tag: i32, x: f64) {
        let w = w as usize;
        let b = x.to_bits();
        state[w] = tag as u32;
        state[w + 1] = b as u32;
        state[w + 2] = (b >> 32) as u32;
    }
    fn get(words: &[u32], w: usize) -> (i32, f64) {
        (words[w] as i32, f64::from_bits(words[w + 1] as u64 | ((words[w + 2] as u64) << 32)))
    }

    /// Writes the inputs: at init the set init sees, else the per-block
    /// set (addendum section 4).
    fn write_inputs(&mut self, k: usize, i: &FormulaInputs, at_init: bool) {
        let s = self.formula.slots;
        let state = &mut self.instances[k].state;
        let mut n = |field: usize, x: f64| Self::put(state, s.num[field], T_NUM, x);
        if at_init {
            n(f::SAMPLERATE, i.samplerate);
            n(f::BLOCK_SIZE, i.block_size);
        }
        for (j, e) in i.eg.iter().enumerate() {
            n(f::EG + j, *e as f64);
        }
        n(f::RATE, i.rate as f64);
        n(f::STARTPHASE, i.startphase as f64);
        n(f::DEFORM, i.deform as f64);
        n(f::AMPLITUDE, i.amplitude as f64);
        n(f::TEMPO, i.tempo as f64);
        n(f::SONGPOS, i.songpos);
        n(f::PB_RANGE_UP, i.pb_range_up as f64);
        n(f::PB_RANGE_DN, i.pb_range_dn as f64);
        n(f::MPE_BENDRANGE, i.mpe_bendrange as f64);
        n(f::VOICE_COUNT, i.voice_count);
        n(f::POLY_LIMIT, i.poly_limit);
        n(f::SCENE_MODE, i.scene_mode);
        n(f::PLAY_MODE, i.play_mode);
        n(f::SPLIT_POINT, i.split_point);
        n(f::LFO_ID, i.lfo_id);
        if let Some(v) = &i.voice {
            n(f::CHANNEL, v.channel);
            n(f::KEY, v.key);
            n(f::VELOCITY, v.velocity);
            n(f::VOICE_ID, v.voice_id);
            n(f::TUNED_KEY, v.tuned_key);
            if !at_init {
                n(f::REL_VELOCITY, v.rel_velocity);
                n(f::POLY_AT, v.poly_at as f64);
                n(f::MPE_BEND, v.mpe_bend as f64);
                n(f::MPE_TIMBRE, v.mpe_timbre as f64);
                n(f::MPE_PRESSURE, v.mpe_pressure as f64);
            }
        }
        if !at_init {
            n(f::INTPHASE, i.intphase);
            n(f::CYCLE, i.intphase);
            n(f::PHASE, i.phase as f64);
            n(f::PB, i.pb as f64);
            n(f::CHAN_AT, i.chan_at as f64);
            n(f::CC_MW, i.cc_mw as f64);
            n(f::CC_MW + 1, i.cc_breath as f64);
            n(f::CC_MW + 2, i.cc_expr as f64);
            n(f::CC_MW + 3, i.cc_sus as f64);
            n(f::LOWEST_KEY, i.lowest_key);
            n(f::HIGHEST_KEY, i.highest_key);
            n(f::LATEST_KEY, i.latest_key);
        }
        let mut b = |field: usize, x: bool| Self::put(state, s.boolean[field], T_BOOL, x as u32 as f64);
        if at_init {
            b(f::CLAMP_OUTPUT, true);
            b(f::USE_AMPLITUDE, true);
            b(f::USE_ENVELOPE, true);
            b(f::USE_RATE, true);
        }
        b(f::IS_PLAYING, i.is_playing);
        b(f::MPE_ENABLED, i.mpe_enabled);
        b(f::IS_RENDERING_TO_UI, i.is_rendering_to_ui);
        b(f::RETRIGGER_AEG, false);
        b(f::RETRIGGER_FEG, false);
        b(f::IS_VOICE, i.voice.is_some());
        if let Some(v) = &i.voice {
            b(f::RELEASED, v.released);
        }
        // macros: the instance's own table (refilled every block).
        let mid = state[S_MACROS as usize] as usize;
        Self::put(state, s.macros, T_TABLE, mid as f64);
        let base = HEAP_BASE as usize + mid * OBJ_WORDS as usize + OBJ_ARR as usize;
        for (j, m) in i.macros.iter().enumerate() {
            let w = base + 3 * j;
            let bits = (*m as f64).to_bits();
            self.shared[w] = T_NUM as u32;
            self.shared[w + 1] = bits as u32;
            self.shared[w + 2] = (bits >> 32) as u32;
        }
    }

    /// The modulator's attack: a fresh state, its tables, the inputs, then
    /// `init(state)`.
    pub fn start(&mut self, k: usize, inputs: &FormulaInputs) {
        if self.error.is_some() || k >= self.instances.len() {
            return;
        }
        self.instances[k].state.fill(0);
        self.instances[k].flags = FormulaIo::default();
        let f = self.formula.clone();
        self.run(&f.prologue, Some(k));
        self.write_inputs(k, inputs, true);
        self.run(&f.init, Some(k));
        self.check_error();
        self.read_flags(k);
        self.collect_if_full();
    }

    /// One block.
    pub fn process(&mut self, k: usize, inputs: &FormulaInputs) -> FormulaIo {
        if self.error.is_some() || k >= self.instances.len() {
            return FormulaIo::default();
        }
        let f = self.formula.clone();
        self.write_inputs(k, inputs, false);
        self.run(&f.process, Some(k));
        self.check_error();
        if self.error.is_some() {
            return FormulaIo::default();
        }
        let out = self.read_out(k);
        self.collect_if_full();
        out
    }

    fn truth(tag: i32, x: f64) -> Option<bool> {
        match tag {
            T_NIL => None,
            T_BOOL | T_NUM => Some(x != 0.0),
            _ => Some(true),
        }
    }

    fn read_flags(&mut self, k: usize) {
        let s = self.formula.slots;
        let inst = &mut self.instances[k];
        let flag = |p: usize| {
            let (t, x) = Self::get(&inst.state, s.boolean[p] as usize);
            Self::truth(t, x).unwrap_or(false)
        };
        let fl = FormulaIo {
            use_envelope: flag(f::USE_ENVELOPE),
            use_amplitude: flag(f::USE_AMPLITUDE),
            clamp_output: flag(f::CLAMP_OUTPUT),
            retrigger_aeg: flag(f::RETRIGGER_AEG),
            retrigger_feg: flag(f::RETRIGGER_FEG),
            ..inst.flags
        };
        inst.flags = fl;
    }

    fn read_out(&mut self, k: usize) -> FormulaIo {
        let (rt, rx) = Self::get(&self.instances[k].state, S_RET as usize);
        let mut out = [0f64; 16];
        let mut count = 1;
        if rt == T_NUM {
            out[0] = rx;
            self.instances[k].flags.retrigger_aeg = false;
            self.instances[k].flags.retrigger_feg = false;
        } else if rt == T_TABLE && rx == STATE_ID {
            self.read_flags(k);
            let (ot, ox) = Self::get(&self.instances[k].state, self.formula.slots.output as usize);
            if ot == T_NUM {
                out[0] = ox;
            } else if ot == T_TABLE && ox != STATE_ID {
                let base = HEAP_BASE as usize + ox as usize * OBJ_WORDS as usize + OBJ_ARR as usize;
                count = 0;
                for (j, o) in out.iter_mut().enumerate() {
                    let (t, x) = Self::get(&self.shared, base + 3 * j);
                    if t == T_NIL {
                        break;
                    }
                    *o = if t == T_NUM { x } else { 0.0 };
                    count = j + 1;
                }
                count = count.max(1);
            }
        } else {
            // A nil/boolean/string result: the reference marks the formula invalid.
            self.error = Some((err::RETURN_TABLE, 0));
            return FormulaIo::default();
        }
        let flags = self.instances[k].flags;
        let mut r = FormulaIo { count, ..flags };
        for (j, x) in out.iter().enumerate() {
            r.outputs[j] = if j < count { *x as f32 } else { 0.0 };
        }
        r
    }

    /// Live tables across every instance.
    pub fn live_tables(&self) -> u32 {
        self.shared[G_LIVE as usize]
    }

    fn collect_if_full(&mut self) {
        if self.shared[G_LIVE as usize] > N_OBJ * 3 / 4 {
            self.collect();
        }
    }

    /// Frees every table no instance, global or prototype reaches.
    pub fn collect(&mut self) {
        let f = self.formula.clone();
        self.run(&f.gc_clear, None);
        for k in 0..self.instances.len() {
            if self.instances[k].active {
                self.run(&f.gc_mark, Some(k));
            }
        }
        self.run(&f.gc_finish, None);
    }
}
