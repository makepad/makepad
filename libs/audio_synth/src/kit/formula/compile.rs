//! The formula compiler: the syntax tree lowered straight into AIR (the
//! compute core's audio IR) through its front-end builder, no text in
//! between.
//!
//! Programs (one formula text compiles once; every instance shares them):
//! - `load`: the top-level chunk (once per formula: it builds the helper
//!   prototypes, defines the functions, sets top-level locals);
//! - `init`: one instance's start (its pinned tables, then `init(state)`);
//! - `process`: one block of one instance;
//! - `gc_clear`, `gc_mark` (per live instance), `gc_finish`: collect the
//!   tables nothing reaches (the runner calls them when the heap fills).
//!
//! Memory: AIR's regions. **Ctx** is the constant string pool (read only).
//! **Shared** is the formula's: error word, random generator, the table
//! heap, the sandbox globals (process's and init's are separate, as in the
//! reference) and top-level locals; every instance sees the same.
//! **State** is one instance's: its `state` table as a static record (a
//! slot per field name) and the entry's result.
//!
//! Value model: a tag word and an f64 payload (number, boolean 0/1, table
//! id, string slice of the pool). Constant tags fold away in the builder,
//! so typed code carries no checks. Locals assigned once are SSA values,
//! the others AIR variables. Every function is inlined at its call (no
//! recursion; function values are known at compile time). Errors the
//! language raises at run time set the formula's error word (first error
//! wins, with its line); the runner then outputs 0 for every instance.

use super::ast::*;
use makepad_script_compute::ir::{Bin, Cmp, Op, Program, Region, Stmt as AStmt, Ty, Un, Val, Var};
use makepad_script_compute::lower::{AirBuilder, MathFn};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

// -- value tags -------------------------------------------------------------

pub const T_NIL: i32 = 0;
pub const T_BOOL: i32 = 1;
pub const T_NUM: i32 = 2;
pub const T_STR: i32 = 3;
pub const T_TABLE: i32 = 4;
pub const T_FUNC: i32 = 5;

// -- shared (per formula) layout, in 32-bit words --------------------------

/// Error code (0: none) and its line.
pub const G_ERR: u32 = 0;
pub const G_ERR_LINE: u32 = 1;
/// The random generator (two words).
pub const G_RNG: u32 = 2;
/// Live heap objects.
pub const G_LIVE: u32 = 4;
/// Where the next allocation starts looking.
pub const G_HINT: u32 = 5;
pub const G_HEADER: u32 = 8;

/// Array part: keys 1..=ARR.
pub const ARR: u32 = 32;
/// Hash part entries (a power of two).
pub const HASH: u32 = 16;
/// Object header: alive, mark, prototype (id + 1, 0: none), unused.
pub const OBJ_ALIVE: u32 = 0;
pub const OBJ_MARK: u32 = 1;
pub const OBJ_PROTO: u32 = 2;
pub const OBJ_ARR: u32 = 4;
pub const OBJ_HASH: u32 = OBJ_ARR + 3 * ARR;
pub const OBJ_WORDS: u32 = OBJ_HASH + 6 * HASH;
/// Tables alive at once across all instances of one formula.
pub const N_OBJ: u32 = 512;
pub const HEAP_BASE: u32 = G_HEADER;
pub const HEAP_WORDS: u32 = N_OBJ * OBJ_WORDS;
pub const SLOTS_BASE: u32 = HEAP_BASE + HEAP_WORDS;

/// The helper prototypes the load program makes (always alive).
pub const PROTO_SLEW: u32 = 0;
pub const PROTO_CLOCK: u32 = 1;
pub const PROTO_AHD: u32 = 2;
pub const PINNED: u32 = 3;

// -- instance layout ----------------------------------------------------------

/// The entry's first result (a slot: tag, low word, high word).
pub const S_RET: u32 = 0;
/// The instance's `macros` table (refilled by the host every block).
pub const S_MACROS: u32 = 3;
pub const S_HEADER: u32 = 4;

/// The table id of `state` (never a heap object).
pub const STATE_ID: f64 = 65536.0;

/// A string payload: `offset * 65536 + len`, plus [`STR_SLICE`] for a
/// slice made at run time (not a pooled constant).
pub const STR_SLICE: f64 = 1099511627776.0; // 2^40

const FOR_CAP: u32 = 1 << 16;
const WHILE_CAP: u32 = 1 << 16;
const STR_MAX: u32 = 1 << 16;

/// Run-time error codes (the formula's error word).
pub mod err {
    pub const ARITH: i32 = 1;
    pub const COMPARE: i32 = 2;
    pub const INDEX: i32 = 3;
    pub const TABLE_FULL: i32 = 4;
    pub const HEAP_FULL: i32 = 5;
    pub const BAD_KEY: i32 = 6;
    pub const LOOP_LIMIT: i32 = 7;
    pub const RAISED: i32 = 8;
    pub const STRING: i32 = 9;
    pub const STATE_DYNAMIC: i32 = 10;
    pub const INIT_RETURN: i32 = 11;
    pub const METHOD: i32 = 12;
    pub const LENGTH: i32 = 13;
    pub const RANDOM: i32 = 14;
    pub const SLICE_KEY: i32 = 15;
    pub const RETURN_TABLE: i32 = 16;

    pub fn message(code: i32) -> &'static str {
        match code {
            ARITH => "arithmetic on a value that is not a number",
            COMPARE => "comparison of values that cannot be ordered",
            INDEX => "indexing a value that is not a table",
            TABLE_FULL => "a table has more non-array keys than the compiler's tables hold (16)",
            HEAP_FULL => "too many tables alive at once",
            BAD_KEY => "table key is nil or NaN",
            LOOP_LIMIT => "a loop ran past its iteration limit (65536)",
            RAISED => "error() was called",
            STRING => "bad argument to a string function (or an unsupported pattern)",
            STATE_DYNAMIC => "the state table was used through a value the compiler could not follow",
            INIT_RETURN => "init() must return its state table",
            METHOD => "method called on an object of another kind",
            LENGTH => "length of a value that is not a table or string",
            RANDOM => "math.random: interval is empty",
            SLICE_KEY => "a string made at run time used as a table key",
            RETURN_TABLE => "process() returned a table other than its state (not supported)",
            _ => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CErr {
    pub line: u32,
    pub msg: String,
}

pub(super) type CR<T> = Result<T, CErr>;

pub(super) fn cerr<T>(line: u32, msg: impl Into<String>) -> CR<T> {
    Err(CErr { line, msg: msg.into() })
}

// -- compile-time values -----------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub(super) struct V {
    pub tag: Val,
    pub num: Val,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Lib {
    Math,
    Surge,
    SurgeMod,
    String,
    /// A builtin function by its name (`sin`, `random`, `str.sub`, ...).
    Fn(&'static str),
}

#[derive(Clone, Debug)]
pub(super) enum CV {
    Rt(V),
    Func(usize),
    Lib(Lib),
}

#[derive(Clone, Debug)]
enum Bind {
    Static(CV),
    Var(Var, Var),
    /// A slot in the shared region.
    Slot(u32),
}

struct Scope {
    binds: RefCell<Vec<(Rc<str>, Bind)>>,
    parent: Option<Rc<Scope>>,
}

type Env = Option<Rc<Scope>>;

fn lookup(env: &Env, name: &str) -> Option<Bind> {
    let mut s = env.clone();
    while let Some(sc) = s {
        if let Some((_, b)) = sc.binds.borrow().iter().rev().find(|(n, _)| &**n == name) {
            return Some(b.clone());
        }
        s = sc.parent.clone();
    }
    None
}

/// Which global table a function's free names resolve in: the top-level
/// chunk's, init's sandbox or process's sandbox.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sandbox {
    Top,
    Init,
    Process,
}

struct FuncVal {
    body: Rc<FuncBody>,
    env: Env,
    sandbox: Sandbox,
}

/// `state` fields the host writes (numbers and booleans) and the ones it
/// reads back; all get record slots.
pub const NUM_FIELDS: &[&str] = &[
    "samplerate", "block_size", "delay", "attack", "hold", "decay", "sustain", "release", "rate", "startphase", "deform", "amplitude", "tempo", "songpos", "pb_range_up", "pb_range_dn",
    "mpe_bendrange", "voice_count", "poly_limit", "scene_mode", "play_mode", "split_point", "lfo_id", "channel", "key", "velocity", "voice_id", "tuned_key", "intphase", "cycle", "phase", "pb",
    "chan_at", "cc_mw", "cc_breath", "cc_expr", "cc_sus", "lowest_key", "highest_key", "latest_key", "rel_velocity", "poly_at", "mpe_bend", "mpe_timbre", "mpe_pressure",
];
pub const BOOL_FIELDS: &[&str] = &[
    "is_playing", "mpe_enabled", "is_voice", "is_rendering_to_ui", "released", "clamp_output", "use_amplitude", "use_envelope", "use_rate", "retrigger_AEG", "retrigger_FEG",
];
pub const OTHER_FIELDS: &[&str] = &["output", "macros", "subscriptions", "labels"];

/// Indices into [`NUM_FIELDS`] and [`BOOL_FIELDS`].
pub mod f {
    pub const SAMPLERATE: usize = 0;
    pub const BLOCK_SIZE: usize = 1;
    pub const EG: usize = 2; // delay..release: 2..=7
    pub const RATE: usize = 8;
    pub const STARTPHASE: usize = 9;
    pub const DEFORM: usize = 10;
    pub const AMPLITUDE: usize = 11;
    pub const TEMPO: usize = 12;
    pub const SONGPOS: usize = 13;
    pub const PB_RANGE_UP: usize = 14;
    pub const PB_RANGE_DN: usize = 15;
    pub const MPE_BENDRANGE: usize = 16;
    pub const VOICE_COUNT: usize = 17;
    pub const POLY_LIMIT: usize = 18;
    pub const SCENE_MODE: usize = 19;
    pub const PLAY_MODE: usize = 20;
    pub const SPLIT_POINT: usize = 21;
    pub const LFO_ID: usize = 22;
    pub const CHANNEL: usize = 23;
    pub const KEY: usize = 24;
    pub const VELOCITY: usize = 25;
    pub const VOICE_ID: usize = 26;
    pub const TUNED_KEY: usize = 27;
    pub const INTPHASE: usize = 28;
    pub const CYCLE: usize = 29;
    pub const PHASE: usize = 30;
    pub const PB: usize = 31;
    pub const CHAN_AT: usize = 32;
    pub const CC_MW: usize = 33; // cc_mw cc_breath cc_expr cc_sus: 33..=36
    pub const LOWEST_KEY: usize = 37;
    pub const HIGHEST_KEY: usize = 38;
    pub const LATEST_KEY: usize = 39;
    pub const REL_VELOCITY: usize = 40;
    pub const POLY_AT: usize = 41;
    pub const MPE_BEND: usize = 42;
    pub const MPE_TIMBRE: usize = 43;
    pub const MPE_PRESSURE: usize = 44;

    pub const IS_PLAYING: usize = 0;
    pub const MPE_ENABLED: usize = 1;
    pub const IS_VOICE: usize = 2;
    pub const IS_RENDERING_TO_UI: usize = 3;
    pub const RELEASED: usize = 4;
    pub const CLAMP_OUTPUT: usize = 5;
    pub const USE_AMPLITUDE: usize = 6;
    pub const USE_ENVELOPE: usize = 7;
    pub const USE_RATE: usize = 8;
    pub const RETRIGGER_AEG: usize = 9;
    pub const RETRIGGER_FEG: usize = 10;
}

pub(super) const MATH_FNS: &[&str] = &[
    "abs", "ceil", "floor", "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "exp", "log", "log10", "pow", "fmod", "modf", "min", "max", "sinh", "cosh", "tanh", "deg", "rad",
    "random", "randomseed", "clamp", "limit_range", "parity", "sgn", "sign", "rescale", "norm", "range", "note_to_freq", "freq_to_note", "lin_to_dB", "dB_to_lin", "gcd", "lcm",
    "random_normal", "random_poisson", "random_exponential", "random_bernoulli",
];
const MATH_CONSTS: &[(&str, f64)] = &[("pi", std::f64::consts::PI), ("huge", f64::INFINITY)];
const GLOBAL_FNS: &[&str] = &["type", "tonumber", "print", "error", "assert"];
const UNSUPPORTED_GLOBALS: &[&str] = &[
    "pairs", "ipairs", "next", "unpack", "select", "setmetatable", "getmetatable", "rawget", "rawset", "rawequal", "pcall", "xpcall", "tostring", "table", "bit", "shared", "real_fft",
    "complex_fft", "real_ifft", "complex_ifft", "loadstring", "require", "setfenv", "getfenv",
];

/// Strings the compiler itself uses (pooled up front with the script's).
pub(super) const INTERNAL_STRINGS: &[&str] = &[
    "nil", "boolean", "number", "string", "table", "function", "macros", "prior", "block_factor", "block_size", "samplerate", "calculate", "is_first", "numerator", "denominator", "prioribeat",
    "newbeat", "intphase", "ibeat", "phase", "a", "h", "d",
];

fn math_fn(name: &str) -> Option<&'static str> {
    MATH_FNS.iter().find(|n| **n == name).copied()
}

// -- the pre-pass: globals, constant keys, strings ---------------------------

#[derive(Default)]
struct GlobalInfo {
    /// Globals assigned a function at the top level, and globals assigned
    /// anywhere else.
    top_funcs: HashSet<Rc<str>>,
    dynamic: HashSet<Rc<str>>,
    keys: Vec<Rc<str>>,
    strings: Vec<Rc<str>>,
}

struct Pre<'a> {
    info: &'a mut GlobalInfo,
    scopes: Vec<HashSet<Rc<str>>>,
}

impl Pre<'_> {
    fn is_local(&self, n: &str) -> bool {
        self.scopes.iter().any(|s| s.contains(n))
    }
    fn declare(&mut self, n: &Rc<str>) {
        self.scopes.last_mut().unwrap().insert(n.clone());
    }
    fn key(&mut self, k: &Rc<str>) {
        if !self.info.keys.contains(k) {
            self.info.keys.push(k.clone());
        }
    }
    fn block(&mut self, b: &Block, top: bool) {
        self.scopes.push(HashSet::new());
        for s in b {
            self.stat(s, top);
        }
        self.scopes.pop();
    }
    fn func(&mut self, f: &FuncBody) {
        self.scopes.push(f.params.iter().cloned().collect());
        self.block(&f.body, false);
        self.scopes.pop();
    }
    fn stat(&mut self, s: &Stat, top: bool) {
        match &s.kind {
            StatKind::Local(names, vals) => {
                for v in vals {
                    self.expr(v);
                }
                for n in names {
                    self.declare(n);
                }
            }
            StatKind::LocalFunc(n, f) => {
                self.declare(n);
                self.func(f);
            }
            StatKind::Assign(ts, vals) => {
                for v in vals {
                    self.expr(v);
                }
                for t in ts {
                    match &t.kind {
                        ExprKind::Name(n) if !self.is_local(n) => {
                            let is_fn = ts.len() == 1 && vals.len() == 1 && matches!(vals[0].kind, ExprKind::Func(_));
                            if top && is_fn {
                                self.info.top_funcs.insert(n.clone());
                            } else {
                                self.info.dynamic.insert(n.clone());
                            }
                        }
                        _ => self.expr(t),
                    }
                }
            }
            StatKind::Call(e) => self.expr(e),
            StatKind::Do(b) => self.block(b, false),
            StatKind::While(c, b) => {
                self.expr(c);
                self.block(b, false);
            }
            StatKind::Repeat(b, c) => {
                self.scopes.push(HashSet::new());
                for s in b {
                    self.stat(s, false);
                }
                self.expr(c);
                self.scopes.pop();
            }
            StatKind::If(arms, els) => {
                for (c, b) in arms {
                    self.expr(c);
                    self.block(b, false);
                }
                if let Some(b) = els {
                    self.block(b, false);
                }
            }
            StatKind::NumFor(n, a, b, c, body) => {
                self.expr(a);
                self.expr(b);
                if let Some(c) = c {
                    self.expr(c);
                }
                self.scopes.push([n.clone()].into_iter().collect());
                self.block(body, false);
                self.scopes.pop();
            }
            StatKind::GenFor(names, es, body) => {
                for e in es {
                    self.expr(e);
                }
                self.scopes.push(names.iter().cloned().collect());
                self.block(body, false);
                self.scopes.pop();
            }
            StatKind::Return(es) => {
                for e in es {
                    self.expr(e);
                }
            }
            StatKind::Break => {}
        }
    }
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Str(s) => {
                if !self.info.strings.contains(s) {
                    self.info.strings.push(s.clone());
                }
            }
            ExprKind::Func(f) => self.func(f),
            ExprKind::Table(fields) => {
                for f in fields {
                    match f {
                        Field::Keyed(k, v) => {
                            if let ExprKind::Str(s) = &k.kind {
                                self.key(s);
                            }
                            self.expr(k);
                            self.expr(v);
                        }
                        Field::Pos(v) => self.expr(v),
                    }
                }
            }
            ExprKind::Bin(_, a, b) => {
                self.expr(a);
                self.expr(b);
            }
            ExprKind::Un(_, a) | ExprKind::Paren(a) => self.expr(a),
            ExprKind::Index(o, k) => {
                if let ExprKind::Str(s) = &k.kind {
                    self.key(s);
                }
                self.expr(o);
                self.expr(k);
            }
            ExprKind::Call(f, args) => {
                self.expr(f);
                for a in args {
                    self.expr(a);
                }
            }
            ExprKind::Method(o, _, args) => {
                self.expr(o);
                for a in args {
                    self.expr(a);
                }
            }
            _ => {}
        }
    }
}

/// Names assigned (as plain names) anywhere in a block, nested functions
/// included: a local in this set is a variable, every other local an SSA
/// value.
fn assigned_names(b: &Block, out: &mut HashSet<Rc<str>>) {
    fn expr(e: &Expr, out: &mut HashSet<Rc<str>>) {
        match &e.kind {
            ExprKind::Func(f) => assigned_names(&f.body, out),
            ExprKind::Table(fs) => {
                for f in fs {
                    match f {
                        Field::Keyed(k, v) => {
                            expr(k, out);
                            expr(v, out)
                        }
                        Field::Pos(v) => expr(v, out),
                    }
                }
            }
            ExprKind::Bin(_, a, b) | ExprKind::Index(a, b) => {
                expr(a, out);
                expr(b, out)
            }
            ExprKind::Un(_, a) | ExprKind::Paren(a) => expr(a, out),
            ExprKind::Call(f, args) | ExprKind::Method(f, _, args) => {
                expr(f, out);
                for a in args {
                    expr(a, out)
                }
            }
            _ => {}
        }
    }
    for s in b {
        match &s.kind {
            StatKind::Assign(ts, vs) => {
                for t in ts {
                    if let ExprKind::Name(n) = &t.kind {
                        out.insert(n.clone());
                    } else {
                        expr(t, out);
                    }
                }
                for v in vs {
                    expr(v, out)
                }
            }
            StatKind::Local(_, vs) | StatKind::Return(vs) => {
                for v in vs {
                    expr(v, out)
                }
            }
            StatKind::LocalFunc(_, f) => assigned_names(&f.body, out),
            StatKind::Call(e) => expr(e, out),
            StatKind::Do(b) => assigned_names(b, out),
            StatKind::While(c, b) | StatKind::Repeat(b, c) => {
                expr(c, out);
                assigned_names(b, out)
            }
            StatKind::If(arms, els) => {
                for (c, b) in arms {
                    expr(c, out);
                    assigned_names(b, out)
                }
                if let Some(b) = els {
                    assigned_names(b, out)
                }
            }
            StatKind::NumFor(_, a, b, c, body) => {
                expr(a, out);
                expr(b, out);
                if let Some(c) = c {
                    expr(c, out)
                }
                assigned_names(body, out)
            }
            StatKind::GenFor(_, es, body) => {
                for e in es {
                    expr(e, out)
                }
                assigned_names(body, out)
            }
            StatKind::Break => {}
        }
    }
}

fn max_returns(b: &Block) -> usize {
    fn walk(b: &Block, m: &mut usize) {
        for s in b {
            match &s.kind {
                StatKind::Return(es) => {
                    let n = match es.last().map(|e| &e.kind) {
                        Some(ExprKind::Call(..)) | Some(ExprKind::Method(..)) => es.len() + 3,
                        _ => es.len(),
                    };
                    *m = (*m).max(n)
                }
                StatKind::Do(b) | StatKind::While(_, b) | StatKind::Repeat(b, _) | StatKind::NumFor(_, _, _, _, b) | StatKind::GenFor(_, _, b) => walk(b, m),
                StatKind::If(arms, els) => {
                    for (_, b) in arms {
                        walk(b, m)
                    }
                    if let Some(b) = els {
                        walk(b, m)
                    }
                }
                _ => {}
            }
        }
    }
    let mut m = 0;
    walk(b, &mut m);
    m
}

// -- shared compile session --------------------------------------------------

/// The layout every program of one formula shares.
#[derive(Default)]
pub struct Layout {
    /// `state` field -> word offset of its slot (instance region).
    pub record: HashMap<String, u32>,
    pub state_words: u32,
    /// Globals and top-level locals -> slot (shared region).
    persist: HashMap<String, u32>,
    pub shared_words: u32,
    /// The constant string pool, one byte per word (the ctx region).
    pub pool: Vec<u32>,
    strings: HashMap<String, (u32, u32)>,
    frozen: bool,
    late: Option<String>,
}

impl Layout {
    fn shared_slot(&mut self) -> u32 {
        let w = self.shared_words;
        self.shared_words += 3;
        w
    }
    fn add(&mut self, s: &str) {
        if !self.strings.contains_key(s) {
            let off = self.pool.len() as u32;
            self.pool.extend(s.bytes().map(|c| c as u32));
            self.strings.insert(s.to_string(), (off, s.len() as u32));
        }
    }
    /// A pooled string's payload.
    pub fn payload(&mut self, s: &str) -> f64 {
        match self.strings.get(s) {
            Some((off, len)) => *off as f64 * 65536.0 + *len as f64,
            None => {
                if self.frozen {
                    self.late = Some(s.to_string());
                    return 0.0;
                }
                self.add(s);
                self.payload(s)
            }
        }
    }
    /// The text of a pooled string payload.
    pub fn text(&self, p: f64) -> Option<String> {
        if !(p >= 0.0 && p < STR_SLICE) {
            return None;
        }
        let off = (p / 65536.0).floor() as usize;
        let len = (p - off as f64 * 65536.0) as usize;
        self.pool.get(off..off + len).map(|w| w.iter().map(|c| *c as u8 as char).collect())
    }
    fn pool_extent(&self) -> u32 {
        self.pool.len().max(1) as u32
    }
}

pub struct Compiled {
    pub load: Program,
    /// An instance's own tables (before the host writes its inputs).
    pub prologue: Program,
    pub init: Program,
    pub process: Program,
    pub gc_clear: Program,
    pub gc_mark: Program,
    pub gc_finish: Program,
    pub layout: Layout,
}

#[derive(Clone, Copy, PartialEq)]
enum LoopKind {
    Lua,
    Frame,
}

struct Frame {
    rets: Vec<(Var, Var)>,
    loop_index: usize,
}

pub(super) struct Cx<'s> {
    pub b: AirBuilder,
    pub lay: &'s mut Layout,
    funcs: &'s mut Vec<FuncVal>,
    info: &'s GlobalInfo,
    top_static: &'s mut HashMap<Rc<str>, usize>,
    env: Env,
    sandbox: Sandbox,
    loops: Vec<LoopKind>,
    frames: Vec<Frame>,
    inlining: Vec<*const FuncBody>,
    mutable: Vec<HashSet<Rc<str>>>,
    /// Compiling top-level chunk statements (not inside a function).
    top: bool,
    pub line: u32,
}

impl<'s> Cx<'s> {
    fn new(lay: &'s mut Layout, funcs: &'s mut Vec<FuncVal>, info: &'s GlobalInfo, top_static: &'s mut HashMap<Rc<str>, usize>, env: Env, sandbox: Sandbox, top: bool) -> Self {
        Cx { b: AirBuilder::new(), lay, funcs, info, top_static, env, sandbox, loops: Vec::new(), frames: Vec::new(), inlining: Vec::new(), mutable: vec![HashSet::new()], top, line: 0 }
    }

    // -- builder shorthands ---------------------------------------------------
    pub fn ci(&mut self, x: i32) -> Val {
        self.b.i32c(x)
    }
    pub fn cd(&mut self, x: f64) -> Val {
        self.b.f64c(x)
    }
    pub fn cb(&mut self, x: bool) -> Val {
        self.b.boolc(x)
    }
    pub fn ib(&mut self, op: Bin, a: Val, b: Val) -> Val {
        self.b.def(Ty::I32, Op::Bin(op, a, b))
    }
    pub fn db(&mut self, op: Bin, a: Val, b: Val) -> Val {
        self.b.def(Ty::F64, Op::Bin(op, a, b))
    }
    pub fn dbc(&mut self, op: Bin, a: Val, k: f64) -> Val {
        let k = self.cd(k);
        self.db(op, a, k)
    }
    fn bb(&mut self, op: Bin, a: Val, b: Val) -> Val {
        self.b.def(Ty::Bool, Op::Bin(op, a, b))
    }
    pub fn du(&mut self, op: Un, a: Val) -> Val {
        self.b.def(Ty::F64, Op::Un(op, a))
    }
    pub fn cmpi(&mut self, c: Cmp, a: Val, b: Val) -> Val {
        self.b.def(Ty::Bool, Op::CmpI(c, a, b))
    }
    pub fn cmpic(&mut self, c: Cmp, a: Val, k: i32) -> Val {
        let k = self.ci(k);
        self.cmpi(c, a, k)
    }
    pub fn cmpd(&mut self, c: Cmp, a: Val, b: Val) -> Val {
        self.b.def(Ty::Bool, Op::CmpD(c, a, b))
    }
    pub fn cmpdc(&mut self, c: Cmp, a: Val, k: f64) -> Val {
        let k = self.cd(k);
        self.cmpd(c, a, k)
    }
    pub fn not(&mut self, a: Val) -> Val {
        self.b.def(Ty::Bool, Op::Un(Un::NotB, a))
    }
    pub fn and(&mut self, a: Val, b: Val) -> Val {
        self.bb(Bin::AndB, a, b)
    }
    pub fn or(&mut self, a: Val, b: Val) -> Val {
        self.bb(Bin::OrB, a, b)
    }
    pub fn d2i(&mut self, x: Val) -> Val {
        self.b.def(Ty::I32, Op::Un(Un::D2I, x))
    }
    pub fn i2d(&mut self, x: Val) -> Val {
        self.b.def(Ty::F64, Op::Un(Un::I2D, x))
    }
    pub fn addi(&mut self, a: Val, k: i32) -> Val {
        let k = self.ci(k);
        self.ib(Bin::AddI, a, k)
    }
    fn b2d(&mut self, c: Val) -> Val {
        let one = self.cd(1.0);
        let zero = self.cd(0.0);
        self.b.sel(c, one, zero)
    }
    pub fn is_const_false(&self, c: Val) -> bool {
        self.b.const_bits(c) == Some(0)
    }
    pub fn const_bool(&self, c: Val) -> Option<bool> {
        self.b.const_bits(c).map(|x| x != 0)
    }
    pub fn const_f64(&self, v: Val) -> Option<f64> {
        if self.b.ty(v) == Ty::F64 {
            self.b.const_bits(v).map(f64::from_bits)
        } else {
            None
        }
    }
    pub fn const_i32(&self, v: Val) -> Option<i32> {
        self.b.const_bits(v).map(|x| x as u32 as i32)
    }

    pub fn nil(&mut self) -> V {
        V { tag: self.ci(T_NIL), num: self.cd(0.0) }
    }
    pub fn num(&mut self, x: Val) -> V {
        V { tag: self.ci(T_NUM), num: x }
    }
    pub fn numc(&mut self, x: f64) -> V {
        let n = self.cd(x);
        self.num(n)
    }
    pub fn boolean(&mut self, c: Val) -> V {
        let n = self.b2d(c);
        V { tag: self.ci(T_BOOL), num: n }
    }
    pub fn boolc(&mut self, x: bool) -> V {
        let c = self.cb(x);
        self.boolean(c)
    }
    pub fn strc(&mut self, s: &str) -> V {
        let p = self.lay.payload(s);
        V { tag: self.ci(T_STR), num: self.cd(p) }
    }
    pub fn table_const(&mut self, id: u32) -> V {
        V { tag: self.ci(T_TABLE), num: self.cd(id as f64) }
    }

    // -- memory -------------------------------------------------------------
    pub fn ld(&mut self, r: Region, base: u32, extent: u32, off: Option<Val>) -> Val {
        self.b.load(Ty::I32, r, base, extent, off)
    }
    pub fn st(&mut self, r: Region, base: u32, extent: u32, off: Option<Val>, v: Val) {
        self.b.store(r, base, extent, off, v)
    }
    fn offp(&mut self, off: Option<Val>, k: i32) -> Option<Val> {
        off.map(|o| self.addi(o, k))
    }
    /// A slot (tag, low, high) at `base + off`.
    pub fn ld_slot(&mut self, r: Region, base: u32, extent: u32, off: Option<Val>) -> V {
        let (b1, b2) = if off.is_some() { (base, base) } else { (base + 1, base + 2) };
        let o1 = self.offp(off, 1);
        let o2 = self.offp(off, 2);
        let tag = self.ld(r, base, extent, off);
        let lo = self.ld(r, b1, extent, o1);
        let hi = self.ld(r, b2, extent, o2);
        let num = self.db(Bin::MakeD, hi, lo);
        V { tag, num }
    }
    pub fn st_slot(&mut self, r: Region, base: u32, extent: u32, off: Option<Val>, v: V) {
        let (b1, b2) = if off.is_some() { (base, base) } else { (base + 1, base + 2) };
        let o1 = self.offp(off, 1);
        let o2 = self.offp(off, 2);
        let lo = self.b.def(Ty::I32, Op::Un(Un::LoD, v.num));
        let hi = self.b.def(Ty::I32, Op::Un(Un::HiD, v.num));
        self.st(r, base, extent, off, v.tag);
        self.st(r, b1, extent, o1, lo);
        self.st(r, b2, extent, o2, hi);
    }
    pub fn gld(&mut self, w: u32) -> Val {
        self.ld(Region::Shared, w, 1, None)
    }
    pub fn gst(&mut self, w: u32, v: Val) {
        self.st(Region::Shared, w, 1, None, v)
    }
    fn heap_ld(&mut self, off: Val) -> Val {
        self.ld(Region::Shared, HEAP_BASE, HEAP_WORDS, Some(off))
    }
    fn heap_st(&mut self, off: Val, v: Val) {
        self.st(Region::Shared, HEAP_BASE, HEAP_WORDS, Some(off), v)
    }
    fn heap_ld_slot(&mut self, off: Val) -> V {
        self.ld_slot(Region::Shared, HEAP_BASE, HEAP_WORDS, Some(off))
    }
    fn heap_st_slot(&mut self, off: Val, v: V) {
        self.st_slot(Region::Shared, HEAP_BASE, HEAP_WORDS, Some(off), v)
    }
    /// Word offset (in the heap) of object `id` (an f64 table id).
    fn obj_off(&mut self, id: Val) -> Val {
        let i = self.d2i(id);
        let w = self.ci(OBJ_WORDS as i32);
        self.ib(Bin::MulI, i, w)
    }

    // -- control helpers ----------------------------------------------------
    pub fn var_pair(&mut self) -> (Var, Var) {
        (self.b.var(Ty::I32), self.b.var(Ty::F64))
    }
    pub fn set_pair(&mut self, p: (Var, Var), v: V) {
        self.b.set(p.0, v.tag);
        self.b.set(p.1, v.num);
    }
    pub fn get_pair(&mut self, p: (Var, Var)) -> V {
        V { tag: self.b.get(p.0), num: self.b.get(p.1) }
    }
    /// `if c { then } else { els }` with constant conditions folded.
    pub fn if_else(&mut self, c: Val, then: impl FnOnce(&mut Self) -> CR<()>, els: impl FnOnce(&mut Self) -> CR<()>) -> CR<()> {
        match self.const_bool(c) {
            Some(true) => then(self),
            Some(false) => els(self),
            None => {
                self.b.open();
                let r = then(self);
                let t = self.b.close();
                r?;
                self.b.open();
                let r = els(self);
                let e = self.b.close();
                r?;
                self.b.push(AStmt::If(c, t, e));
                Ok(())
            }
        }
    }
    pub fn when(&mut self, c: Val, then: impl FnOnce(&mut Self)) {
        match self.const_bool(c) {
            Some(false) => {}
            Some(true) => then(self),
            None => {
                self.b.open();
                then(self);
                let t = self.b.close();
                self.b.push(AStmt::If(c, t, Vec::new()));
            }
        }
    }
    pub fn break_if(&mut self, c: Val) {
        if self.is_const_false(c) {
            return;
        }
        self.b.push(AStmt::If(c, vec![AStmt::Break(0)], Vec::new()));
    }
    /// Raises run-time error `code` (the first one wins) when `c` holds.
    pub fn fail_if(&mut self, c: Val, code: i32) {
        let line = self.line as i32;
        self.when(c, |s| {
            let e = s.gld(G_ERR);
            let first = s.cmpic(Cmp::Eq, e, 0);
            s.when(first, |s| {
                let code = s.ci(code);
                s.gst(G_ERR, code);
                let l = s.ci(line);
                s.gst(G_ERR_LINE, l);
            });
        });
    }
    /// An internal loop (Lua code is never compiled inside one).
    pub fn internal_loop(&mut self, cap: u32, body: impl FnOnce(&mut Self)) {
        self.b.open_loop();
        body(self);
        let b = self.b.close();
        self.b.push(AStmt::Loop { cap, body: b });
    }
    /// `for j in 0..n` over internal data.
    pub fn count_loop(&mut self, n: u32, body: impl FnOnce(&mut Self, Val)) {
        let j = self.b.var(Ty::I32);
        let z = self.ci(0);
        self.b.set(j, z);
        self.internal_loop(n + 1, |s| {
            let jv = s.b.get(j);
            let done = s.cmpic(Cmp::Ge, jv, n as i32);
            s.break_if(done);
            body(s, jv);
            let jn = s.addi(jv, 1);
            s.b.set(j, jn);
        });
    }

    pub fn truthy(&mut self, v: V) -> Val {
        let notnil = self.cmpic(Cmp::Ne, v.tag, T_NIL);
        let isbool = self.cmpic(Cmp::Eq, v.tag, T_BOOL);
        let zero = self.cmpdc(Cmp::Eq, v.num, 0.0);
        let f = self.and(isbool, zero);
        let nf = self.not(f);
        self.and(notnil, nf)
    }
    pub fn to_num(&mut self, v: V) -> Val {
        let bad = self.cmpic(Cmp::Ne, v.tag, T_NUM);
        self.fail_if(bad, err::ARITH);
        v.num
    }
    pub fn is_nil(&mut self, v: V) -> Val {
        self.cmpic(Cmp::Eq, v.tag, T_NIL)
    }

    // -- strings --------------------------------------------------------------
    /// (offset, len) of a string payload (i32).
    pub fn str_parts(&mut self, p: Val) -> (Val, Val) {
        let slice = self.cmpdc(Cmp::Ge, p, STR_SLICE);
        let ps = self.dbc(Bin::SubD, p, STR_SLICE);
        let q = self.b.sel(slice, ps, p);
        let qd = self.dbc(Bin::DivD, q, 65536.0);
        let off = self.du(Un::FloorD, qd);
        let ok = self.dbc(Bin::MulD, off, 65536.0);
        let len = self.db(Bin::SubD, q, ok);
        let off = self.d2i(off);
        let len = self.d2i(len);
        (off, len)
    }
    /// A run-time slice's payload.
    pub fn slice_payload(&mut self, off: Val, len: Val) -> Val {
        let o = self.i2d(off);
        let l = self.i2d(len);
        let m = self.dbc(Bin::MulD, o, 65536.0);
        let p = self.db(Bin::AddD, m, l);
        self.dbc(Bin::AddD, p, STR_SLICE)
    }
    pub fn pool_byte(&mut self, at: Val) -> Val {
        let n = self.lay.pool_extent();
        self.ld(Region::Ctx, 0, n, Some(at))
    }
    /// Does the string at (ao, len) equal the one at (bo, len)?
    pub fn bytes_eq(&mut self, ao: Val, bo: Val, len: Val) -> Val {
        let r = self.b.var(Ty::Bool);
        let t = self.cb(true);
        self.b.set(r, t);
        let j = self.b.var(Ty::I32);
        let z = self.ci(0);
        self.b.set(j, z);
        self.internal_loop(STR_MAX + 1, |s| {
            let jv = s.b.get(j);
            let done = s.cmpi(Cmp::Ge, jv, len);
            s.break_if(done);
            let pa = s.ib(Bin::AddI, ao, jv);
            let pb = s.ib(Bin::AddI, bo, jv);
            let ca = s.pool_byte(pa);
            let cb = s.pool_byte(pb);
            let ne = s.cmpi(Cmp::Ne, ca, cb);
            s.b.open();
            let f = s.cb(false);
            s.b.set(r, f);
            s.b.push(AStmt::Break(0));
            let arm = s.b.close();
            s.b.push(AStmt::If(ne, arm, Vec::new()));
            let jn = s.addi(jv, 1);
            s.b.set(j, jn);
        });
        self.b.get(r)
    }

    // -- equality and ordering ----------------------------------------------
    pub fn eq(&mut self, a: V, b: V) -> Val {
        let same_tag = self.cmpi(Cmp::Eq, a.tag, b.tag);
        let num_eq = self.cmpd(Cmp::Eq, a.num, b.num);
        let plain = self.and(same_tag, num_eq);
        // Strings: pooled constants are unique per content; a slice needs a
        // content compare.
        let a_str = self.cmpic(Cmp::Eq, a.tag, T_STR);
        let b_str = self.cmpic(Cmp::Eq, b.tag, T_STR);
        let both = self.and(a_str, b_str);
        if self.is_const_false(both) {
            return plain;
        }
        let r = self.b.var(Ty::Bool);
        self.b.set(r, plain);
        let sa = self.cmpdc(Cmp::Ge, a.num, STR_SLICE);
        let sb = self.cmpdc(Cmp::Ge, b.num, STR_SLICE);
        let any_slice = self.or(sa, sb);
        let need = self.and(both, any_slice);
        let (an, bn) = (a.num, b.num);
        self.when(need, |s| {
            let (ao, al) = s.str_parts(an);
            let (bo, bl) = s.str_parts(bn);
            let same_len = s.cmpi(Cmp::Eq, al, bl);
            let f = s.cb(false);
            s.b.set(r, f);
            s.when(same_len, |s| {
                let e = s.bytes_eq(ao, bo, al);
                s.b.set(r, e);
            });
        });
        self.b.get(r)
    }
    fn order(&mut self, c: Cmp, a: V, b: V) -> Val {
        let an = self.cmpic(Cmp::Ne, a.tag, T_NUM);
        let bn = self.cmpic(Cmp::Ne, b.tag, T_NUM);
        let bad = self.or(an, bn);
        self.fail_if(bad, err::COMPARE);
        self.cmpd(c, a.num, b.num)
    }

    // -- tables ---------------------------------------------------------------
    /// The hash slot of a key (i32 in 0..HASH). Same function as
    /// [`super::key_hash`].
    fn key_hash(&mut self, k: V) -> Val {
        let x = self.dbc(Bin::AddD, k.num, 0.0);
        let lo = self.b.def(Ty::I32, Op::Un(Un::LoD, x));
        let hi = self.b.def(Ty::I32, Op::Un(Un::HiD, x));
        let m1 = self.ci(0x9E37_79B1u32 as i32);
        let h1 = self.ib(Bin::MulI, hi, m1);
        let m2 = self.ci(0x85EB_CA77u32 as i32);
        let t = self.ib(Bin::MulI, k.tag, m2);
        let z = self.ib(Bin::XorI, lo, h1);
        let z = self.ib(Bin::XorI, z, t);
        let s15 = self.ci(15);
        let sh = self.ib(Bin::ShrUI, z, s15);
        let z = self.ib(Bin::XorI, z, sh);
        let m3 = self.ci(0x2C1B_3C6D);
        let z = self.ib(Bin::MulI, z, m3);
        let s12 = self.ci(12);
        let sh = self.ib(Bin::ShrUI, z, s12);
        let z = self.ib(Bin::XorI, z, sh);
        let mask = self.ci(HASH as i32 - 1);
        self.ib(Bin::AndI, z, mask)
    }
    /// Is `k` an array-part key, and its entry's offset in the object.
    fn arr_key(&mut self, k: V) -> (Val, Val) {
        let isnum = self.cmpic(Cmp::Eq, k.tag, T_NUM);
        let fl = self.du(Un::FloorD, k.num);
        let int = self.cmpd(Cmp::Eq, fl, k.num);
        let ge = self.cmpdc(Cmp::Ge, k.num, 1.0);
        let le = self.cmpdc(Cmp::Le, k.num, ARR as f64);
        let a = self.and(isnum, int);
        let b = self.and(ge, le);
        let is = self.and(a, b);
        let km = self.dbc(Bin::SubD, k.num, 1.0);
        let km = self.dbc(Bin::MaxD, km, 0.0);
        let km = self.dbc(Bin::MinD, km, ARR as f64 - 1.0);
        let i = self.d2i(km);
        let three = self.ci(3);
        let e = self.ib(Bin::MulI, i, three);
        let e = self.addi(e, OBJ_ARR as i32);
        (is, e)
    }
    fn check_table(&mut self, t: V) {
        let not_tab = self.cmpic(Cmp::Ne, t.tag, T_TABLE);
        self.fail_if(not_tab, err::INDEX);
        let is_state = self.cmpdc(Cmp::Eq, t.num, STATE_ID);
        let tab = self.not(not_tab);
        let bad = self.and(tab, is_state);
        self.fail_if(bad, err::STATE_DYNAMIC);
    }
    fn key_checks(&mut self, k: V, for_set: bool) {
        let is_str = self.cmpic(Cmp::Eq, k.tag, T_STR);
        let slice = self.cmpdc(Cmp::Ge, k.num, STR_SLICE);
        let bad = self.and(is_str, slice);
        self.fail_if(bad, err::SLICE_KEY);
        if for_set {
            let nil = self.is_nil(k);
            let isnum = self.cmpic(Cmp::Eq, k.tag, T_NUM);
            let nan = self.cmpd(Cmp::Ne, k.num, k.num);
            let bn = self.and(isnum, nan);
            let bad = self.or(nil, bn);
            self.fail_if(bad, err::BAD_KEY);
        }
    }
    /// The hash probe over object `obj`: `on_found(entry)` for the key's
    /// entry, `on_empty(entry)` for the first free entry when the key is
    /// absent, `on_full` when every entry holds another key.
    fn probe(&mut self, obj: Val, k: V, on_found: &mut dyn FnMut(&mut Self, Val), on_empty: &mut dyn FnMut(&mut Self, Val), on_full: &mut dyn FnMut(&mut Self)) {
        let h = self.key_hash(k);
        let i = self.b.var(Ty::I32);
        let z = self.ci(0);
        self.b.set(i, z);
        let done = self.b.var(Ty::Bool);
        let f = self.cb(false);
        self.b.set(done, f);
        self.internal_loop(HASH, |s| {
            let iv = s.b.get(i);
            let slot = s.ib(Bin::AddI, h, iv);
            let mask = s.ci(HASH as i32 - 1);
            let slot = s.ib(Bin::AndI, slot, mask);
            let six = s.ci(6);
            let e = s.ib(Bin::MulI, slot, six);
            let e = s.addi(e, OBJ_HASH as i32);
            let e = s.ib(Bin::AddI, obj, e);
            let ks = s.heap_ld_slot(e);
            let empty = s.cmpic(Cmp::Eq, ks.tag, T_NIL);
            s.b.open();
            on_empty(s, e);
            let t = s.cb(true);
            s.b.set(done, t);
            s.b.push(AStmt::Break(0));
            let arm = s.b.close();
            s.b.push(AStmt::If(empty, arm, Vec::new()));
            let same_t = s.cmpi(Cmp::Eq, ks.tag, k.tag);
            let same_n = s.cmpd(Cmp::Eq, ks.num, k.num);
            let same = s.and(same_t, same_n);
            s.b.open();
            on_found(s, e);
            let t = s.cb(true);
            s.b.set(done, t);
            s.b.push(AStmt::Break(0));
            let arm = s.b.close();
            s.b.push(AStmt::If(same, arm, Vec::new()));
            let inext = s.addi(iv, 1);
            s.b.set(i, inext);
        });
        let d = self.b.get(done);
        let nd = self.not(d);
        self.when(nd, |s| on_full(s));
    }
    /// Raw `t[k]` (no prototype) into `r`; whether the hash part missed.
    fn raw_get(&mut self, obj: Val, k: V, r: (Var, Var)) -> CR<()> {
        let (is_arr, e) = self.arr_key(k);
        self.if_else(
            is_arr,
            |s| {
                let off = s.ib(Bin::AddI, obj, e);
                let v = s.heap_ld_slot(off);
                s.set_pair(r, v);
                Ok(())
            },
            |s| {
                s.probe(
                    obj,
                    k,
                    &mut |s, e| {
                        let off = s.addi(e, 3);
                        let v = s.heap_ld_slot(off);
                        s.set_pair(r, v);
                    },
                    &mut |_, _| {},
                    &mut |_| {},
                );
                Ok(())
            },
        )
    }
    /// `t[k]` on a heap table (t already checked), reading through to its
    /// prototype when absent.
    fn tget_heap(&mut self, t: V, k: V) -> V {
        self.key_checks(k, false);
        let obj = self.obj_off(t.num);
        let r = self.var_pair();
        let nil = self.nil();
        self.set_pair(r, nil);
        let _ = self.raw_get(obj, k, r);
        let got = self.get_pair(r);
        let miss = self.is_nil(got);
        let po = self.addi(obj, OBJ_PROTO as i32);
        let proto = self.heap_ld(po);
        let has = self.cmpic(Cmp::Ne, proto, 0);
        let look = self.and(miss, has);
        self.when(look, |s| {
            let pid = s.addi(proto, -1);
            let w = s.ci(OBJ_WORDS as i32);
            let pobj = s.ib(Bin::MulI, pid, w);
            let _ = s.raw_get(pobj, k, r);
        });
        self.get_pair(r)
    }
    fn tset_heap(&mut self, t: V, k: V, v: V) {
        self.key_checks(k, true);
        let obj = self.obj_off(t.num);
        let (is_arr, e) = self.arr_key(k);
        let _ = self.if_else(
            is_arr,
            |s| {
                let off = s.ib(Bin::AddI, obj, e);
                s.heap_st_slot(off, v);
                Ok(())
            },
            |s| {
                // Assigning nil to an absent key adds nothing.
                let isnil = s.is_nil(v);
                s.probe(
                    obj,
                    k,
                    &mut |s, e| {
                        let off = s.addi(e, 3);
                        s.heap_st_slot(off, v);
                    },
                    &mut |s, e| {
                        let notnil = s.not(isnil);
                        s.when(notnil, |s| {
                            s.heap_st_slot(e, k);
                            let off = s.addi(e, 3);
                            s.heap_st_slot(off, v);
                        });
                    },
                    &mut |s| {
                        let notnil = s.not(isnil);
                        s.fail_if(notnil, err::TABLE_FULL);
                    },
                );
                Ok(())
            },
        );
    }
    pub fn state_slot(&mut self, name: &str) -> u32 {
        if let Some(w) = self.lay.record.get(name) {
            return *w;
        }
        let w = self.lay.state_words;
        self.lay.state_words += 3;
        self.lay.record.insert(name.to_string(), w);
        w
    }
    pub fn is_state(&self, v: V) -> bool {
        self.const_i32(v.tag) == Some(T_TABLE) && self.const_f64(v.num) == Some(STATE_ID)
    }
    /// `t[k]` for any value t.
    pub fn index(&mut self, t: V, k: V) -> CR<V> {
        if self.is_state(t) {
            let name = self.const_key_name(k)?;
            let w = self.state_slot(&name);
            return Ok(self.ld_slot(Region::State, w, 1, None));
        }
        if self.const_i32(t.tag) == Some(T_STR) {
            return cerr(self.line, "indexing a string (use the method form, e.g. s:sub(1, 2))");
        }
        self.check_table(t);
        Ok(self.tget_heap(t, k))
    }
    pub fn set_index(&mut self, t: V, k: V, v: V) -> CR<()> {
        if self.is_state(t) {
            let name = self.const_key_name(k)?;
            let w = self.state_slot(&name);
            self.st_slot(Region::State, w, 1, None, v);
            return Ok(());
        }
        self.check_table(t);
        self.tset_heap(t, k, v);
        Ok(())
    }
    /// `t.name` / `t.name = v` with a compile-time key.
    pub fn field(&mut self, t: V, name: &str) -> CR<V> {
        if self.is_state(t) {
            let w = self.state_slot(name);
            return Ok(self.ld_slot(Region::State, w, 1, None));
        }
        let k = self.strc(name);
        self.index(t, k)
    }
    pub fn set_field(&mut self, t: V, name: &str, v: V) -> CR<()> {
        if self.is_state(t) {
            let w = self.state_slot(name);
            self.st_slot(Region::State, w, 1, None, v);
            return Ok(());
        }
        let k = self.strc(name);
        self.set_index(t, k, v)
    }
    fn const_key_name(&mut self, k: V) -> CR<String> {
        if self.const_i32(k.tag) == Some(T_STR) {
            if let Some(p) = self.const_f64(k.num) {
                if let Some(s) = self.lay.text(p) {
                    return Ok(s);
                }
            }
        }
        cerr(self.line, "the state table can only be indexed with constant names (state.x or state[\"x\"])")
    }
    /// A new empty table with prototype `proto` (id + 1, 0: none).
    pub fn alloc(&mut self, proto: i32) -> V {
        let id = self.b.var(Ty::I32);
        let m1 = self.ci(-1);
        self.b.set(id, m1);
        let start = self.gld(G_HINT);
        self.count_loop(N_OBJ, |s, j| {
            let at = s.ib(Bin::AddI, start, j);
            let at = s.b.def(Ty::I32, Op::Wrap(at, N_OBJ));
            let w = s.ci(OBJ_WORDS as i32);
            let off = s.ib(Bin::MulI, at, w);
            let alive = s.heap_ld(off);
            let free = s.cmpic(Cmp::Eq, alive, 0);
            s.b.open();
            s.b.set(id, at);
            s.b.push(AStmt::Break(0));
            let arm = s.b.close();
            s.b.push(AStmt::If(free, arm, Vec::new()));
        });
        let idv = self.b.get(id);
        let none = self.cmpic(Cmp::Lt, idv, 0);
        self.fail_if(none, err::HEAP_FULL);
        // On failure the formula is already invalid; the last object takes the writes.
        let last = self.ci(N_OBJ as i32 - 1);
        let idv = self.b.sel(none, last, idv);
        let next = self.addi(idv, 1);
        self.gst(G_HINT, next);
        let w = self.ci(OBJ_WORDS as i32);
        let obj = self.ib(Bin::MulI, idv, w);
        let one = self.ci(1);
        self.heap_st(obj, one);
        let z = self.ci(0);
        let o1 = self.addi(obj, OBJ_MARK as i32);
        self.heap_st(o1, z);
        let c = self.ci(proto);
        let o2 = self.addi(obj, OBJ_PROTO as i32);
        self.heap_st(o2, c);
        self.count_loop(ARR, |s, j| {
            let three = s.ci(3);
            let e = s.ib(Bin::MulI, j, three);
            let e = s.addi(e, OBJ_ARR as i32);
            let e = s.ib(Bin::AddI, obj, e);
            let z = s.ci(T_NIL);
            s.heap_st(e, z);
        });
        self.count_loop(HASH, |s, j| {
            let six = s.ci(6);
            let e = s.ib(Bin::MulI, j, six);
            let e = s.addi(e, OBJ_HASH as i32);
            let e = s.ib(Bin::AddI, obj, e);
            let z = s.ci(T_NIL);
            s.heap_st(e, z);
        });
        let live = self.gld(G_LIVE);
        let l1 = self.addi(live, 1);
        self.gst(G_LIVE, l1);
        let d = self.i2d(idv);
        V { tag: self.ci(T_TABLE), num: d }
    }
    /// Sets a table's prototype (the `new` of the helpers).
    pub fn set_proto(&mut self, t: V, proto: u32) {
        let obj = self.obj_off(t.num);
        let o = self.addi(obj, OBJ_PROTO as i32);
        let p = self.ci(proto as i32 + 1);
        self.heap_st(o, p);
    }
    pub fn proto_of(&mut self, t: V) -> Val {
        let obj = self.obj_off(t.num);
        let o = self.addi(obj, OBJ_PROTO as i32);
        self.heap_ld(o)
    }
    /// The border `#t` of a heap table.
    fn table_len(&mut self, id: Val, r: Var) {
        let obj = self.obj_off(id);
        let n = self.b.var(Ty::I32);
        let z = self.ci(0);
        self.b.set(n, z);
        self.count_loop(ARR, |s, j| {
            let three = s.ci(3);
            let e = s.ib(Bin::MulI, j, three);
            let e = s.addi(e, OBJ_ARR as i32);
            let e = s.ib(Bin::AddI, obj, e);
            let t = s.heap_ld(e);
            let nil = s.cmpic(Cmp::Eq, t, T_NIL);
            s.break_if(nil);
            let j1 = s.addi(j, 1);
            s.b.set(n, j1);
        });
        let nv = self.b.get(n);
        let d = self.i2d(nv);
        self.b.set(r, d);
        // Past the array part: keep counting through the hash part.
        let full = self.cmpic(Cmp::Eq, nv, ARR as i32);
        self.when(full, |s| {
            let c = s.b.var(Ty::F64);
            let a = s.cd(ARR as f64);
            s.b.set(c, a);
            s.internal_loop(HASH + 1, |s| {
                let cv = s.b.get(c);
                let kn = s.dbc(Bin::AddD, cv, 1.0);
                let k = s.num(kn);
                let found = s.b.var(Ty::Bool);
                let f = s.cb(false);
                s.b.set(found, f);
                s.probe(
                    obj,
                    k,
                    &mut |s, e| {
                        let off = s.addi(e, 3);
                        let t = s.heap_ld(off);
                        let nn = s.cmpic(Cmp::Ne, t, T_NIL);
                        s.b.set(found, nn);
                    },
                    &mut |_, _| {},
                    &mut |_| {},
                );
                let fv = s.b.get(found);
                let nf = s.not(fv);
                s.break_if(nf);
                s.b.set(c, kn);
            });
            let cv = s.b.get(c);
            s.b.set(r, cv);
        });
    }
    /// `#v`.
    pub fn length(&mut self, v: V) -> V {
        let r = self.b.var(Ty::F64);
        let z = self.cd(0.0);
        self.b.set(r, z);
        let is_str = self.cmpic(Cmp::Eq, v.tag, T_STR);
        let is_tab = self.cmpic(Cmp::Eq, v.tag, T_TABLE);
        let st = self.cmpdc(Cmp::Eq, v.num, STATE_ID);
        let either = self.or(is_str, is_tab);
        let bad = self.not(either);
        self.fail_if(bad, err::LENGTH);
        let tab_state = self.and(is_tab, st);
        self.fail_if(tab_state, err::STATE_DYNAMIC);
        self.when(is_str, |s| {
            let (_, len) = s.str_parts(v.num);
            let l = s.i2d(len);
            s.b.set(r, l);
        });
        let ns = self.not(st);
        let heap_tab = self.and(is_tab, ns);
        self.when(heap_tab, |s| s.table_len(v.num, r));
        let rv = self.b.get(r);
        self.num(rv)
    }

    // -- random generator (two shared words) ---------------------------------
    fn rotl(&mut self, x: Val, k: i32) -> Val {
        let a = self.ci(k);
        let b = self.ci(32 - k);
        let l = self.ib(Bin::ShlI, x, a);
        let r = self.ib(Bin::ShrUI, x, b);
        self.ib(Bin::OrI, l, r)
    }
    /// The generator step; the same as [`super::Rng::next_u32`].
    fn rng_u32(&mut self) -> Val {
        let s0 = self.gld(G_RNG);
        let s1 = self.gld(G_RNG + 1);
        let m = self.ci(0x9E37_79BBu32 as i32);
        let p = self.ib(Bin::MulI, s0, m);
        let r = self.rotl(p, 5);
        let five = self.ci(5);
        let out = self.ib(Bin::MulI, r, five);
        let s1 = self.ib(Bin::XorI, s1, s0);
        let a = self.rotl(s0, 26);
        let a = self.ib(Bin::XorI, a, s1);
        let nine = self.ci(9);
        let sh = self.ib(Bin::ShlI, s1, nine);
        let n0 = self.ib(Bin::XorI, a, sh);
        let n1 = self.rotl(s1, 13);
        self.gst(G_RNG, n0);
        self.gst(G_RNG + 1, n1);
        out
    }
    /// Uniform in [0, 1) with 53 bits.
    pub fn rng_unit(&mut self) -> Val {
        let a = self.rng_u32();
        let b = self.rng_u32();
        let five = self.ci(5);
        let six = self.ci(6);
        let a = self.ib(Bin::ShrUI, a, five);
        let b = self.ib(Bin::ShrUI, b, six);
        // i32 -> f64 is signed; both are below 2^27, so positive.
        let a = self.i2d(a);
        let b = self.i2d(b);
        let m = self.dbc(Bin::MulD, a, 67108864.0);
        let s = self.db(Bin::AddD, m, b);
        self.dbc(Bin::MulD, s, 1.0 / 9007199254740992.0)
    }
    fn fmix(&mut self, z: Val) -> Val {
        let s16 = self.ci(16);
        let s13 = self.ci(13);
        let a = self.ib(Bin::ShrUI, z, s16);
        let z = self.ib(Bin::XorI, z, a);
        let m = self.ci(0x85EB_CA6Bu32 as i32);
        let z = self.ib(Bin::MulI, z, m);
        let a = self.ib(Bin::ShrUI, z, s13);
        let z = self.ib(Bin::XorI, z, a);
        let m = self.ci(0xC2B2_AE35u32 as i32);
        let z = self.ib(Bin::MulI, z, m);
        let a = self.ib(Bin::ShrUI, z, s16);
        self.ib(Bin::XorI, z, a)
    }
    /// `math.randomseed(x)`; the same as [`super::Rng::seed`].
    pub fn rng_seed(&mut self, x: Val) {
        let x = self.dbc(Bin::AddD, x, 0.0);
        let lo = self.b.def(Ty::I32, Op::Un(Un::LoD, x));
        let hi = self.b.def(Ty::I32, Op::Un(Un::HiD, x));
        let c1 = self.ci(0x6A09_E667);
        let c2 = self.ci(0xBB67_AE85u32 as i32);
        let a = self.ib(Bin::XorI, lo, c1);
        let a = self.fmix(a);
        let b = self.ib(Bin::XorI, hi, c2);
        let b = self.ib(Bin::XorI, b, a);
        let b = self.fmix(b);
        let both = self.ib(Bin::OrI, a, b);
        let zero = self.cmpic(Cmp::Eq, both, 0);
        let one = self.ci(1);
        let a = self.b.sel(zero, one, a);
        self.gst(G_RNG, a);
        self.gst(G_RNG + 1, b);
    }

    // -- names ------------------------------------------------------------------
    fn global_slot(&mut self, name: &str) -> u32 {
        let key = format!("{:?}:{}", self.sandbox, name);
        if let Some(w) = self.lay.persist.get(&key) {
            return *w;
        }
        let w = self.lay.shared_slot();
        self.lay.persist.insert(key, w);
        w
    }
    fn builtin_global(&self, name: &str) -> Option<CV> {
        if self.info.dynamic.contains(name) || self.info.top_funcs.contains(name) {
            return None;
        }
        match name {
            "math" => Some(CV::Lib(Lib::Math)),
            "surge" => Some(CV::Lib(Lib::Surge)),
            "string" => Some(CV::Lib(Lib::String)),
            _ => math_fn(name).or_else(|| GLOBAL_FNS.iter().find(|n| **n == name).copied()).map(|f| CV::Lib(Lib::Fn(f))),
        }
    }
    fn name(&mut self, n: &Rc<str>) -> CR<CV> {
        if let Some(b) = lookup(&self.env, n) {
            return Ok(match b {
                Bind::Static(cv) => cv,
                Bind::Var(t, x) => CV::Rt(self.get_pair((t, x))),
                Bind::Slot(w) => CV::Rt(self.ld_slot(Region::Shared, w, 1, None)),
            });
        }
        if let Some(cv) = self.builtin_global(n) {
            return Ok(cv);
        }
        if let Some((_, c)) = MATH_CONSTS.iter().find(|(m, _)| *m == &**n) {
            if !self.info.dynamic.contains(n) {
                return Ok(CV::Rt(self.numc(*c)));
            }
        }
        if UNSUPPORTED_GLOBALS.contains(&&**n) && !self.info.dynamic.contains(n) && !self.info.top_funcs.contains(n) {
            return cerr(self.line, format!("'{}' is not supported by the formula compiler", n));
        }
        if self.info.top_funcs.contains(n) && !self.info.dynamic.contains(n) {
            if self.sandbox != Sandbox::Top {
                return cerr(
                    self.line,
                    format!("global function '{}' is defined at the top level, which init/process cannot see (they run in their own environments); make it 'local function {}'", n, n),
                );
            }
            if let Some(id) = self.top_static.get(n) {
                return Ok(CV::Func(*id));
            }
        }
        let w = self.global_slot(n);
        Ok(CV::Rt(self.ld_slot(Region::Shared, w, 1, None)))
    }
    fn assign_name(&mut self, n: &Rc<str>, cv: CV) -> CR<()> {
        match lookup(&self.env, n) {
            Some(Bind::Var(t, x)) => {
                let v = self.rt(cv, "a function value cannot be stored in a variable that changes")?;
                self.set_pair((t, x), v);
                Ok(())
            }
            Some(Bind::Slot(w)) => {
                let v = self.rt(cv, "a function value cannot be stored in a variable that changes")?;
                self.st_slot(Region::Shared, w, 1, None, v);
                Ok(())
            }
            Some(Bind::Static(_)) => cerr(self.line, format!("internal: '{}' was taken as constant", n)),
            None => {
                if let CV::Func(id) = cv {
                    if self.top && self.frames.is_empty() && self.loops.is_empty() {
                        self.top_static.insert(n.clone(), id);
                        return Ok(());
                    }
                    return cerr(self.line, format!("function '{}' must be defined at the top level of the script", n));
                }
                let v = self.rt(cv, "this value cannot be stored in a global")?;
                let w = self.global_slot(n);
                self.st_slot(Region::Shared, w, 1, None, v);
                Ok(())
            }
        }
    }
    pub fn rt(&mut self, cv: CV, what: &str) -> CR<V> {
        match cv {
            CV::Rt(v) => Ok(v),
            CV::Func(_) => cerr(self.line, format!("{} (functions are only called, not stored)", what)),
            CV::Lib(_) => cerr(self.line, format!("{} (library values are only used directly)", what)),
        }
    }
    fn declare(&mut self, binds: Vec<(Rc<str>, Bind)>) {
        self.env = Some(Rc::new(Scope { binds: RefCell::new(binds), parent: self.env.take() }));
    }
    fn is_mutable(&self, n: &str) -> bool {
        self.mutable.last().map_or(true, |m| m.contains(n))
    }
    fn local_bind(&mut self, n: &Rc<str>, cv: CV) -> CR<Bind> {
        if self.top && self.frames.is_empty() {
            // Top-level locals outlive the chunk: they are upvalues every
            // instance shares.
            return Ok(match cv {
                CV::Rt(v) => {
                    let key = format!("local:{}:{}", n, self.lay.persist.len());
                    let w = self.lay.shared_slot();
                    self.lay.persist.insert(key, w);
                    self.st_slot(Region::Shared, w, 1, None, v);
                    Bind::Slot(w)
                }
                cv => {
                    if self.is_mutable(n) {
                        return cerr(self.line, format!("'{}' holds a function or library and is reassigned", n));
                    }
                    Bind::Static(cv)
                }
            });
        }
        if self.is_mutable(n) {
            let v = self.rt(cv, &format!("'{}' is reassigned, so it cannot hold a function", n))?;
            let p = self.var_pair();
            self.set_pair(p, v);
            Ok(Bind::Var(p.0, p.1))
        } else {
            Ok(Bind::Static(cv))
        }
    }

    // -- expressions --------------------------------------------------------------
    fn expr(&mut self, e: &Expr) -> CR<CV> {
        let mut v = self.expr_multi(e, 1)?;
        Ok(if v.is_empty() { CV::Rt(self.nil()) } else { v.swap_remove(0) })
    }
    fn rexpr(&mut self, e: &Expr) -> CR<V> {
        let cv = self.expr(e)?;
        self.line = e.line;
        self.rt(cv, "a function or library used as a value")
    }
    /// `want`: how many results a call should produce (others give 1).
    fn expr_multi(&mut self, e: &Expr, want: usize) -> CR<Vec<CV>> {
        self.line = e.line;
        Ok(vec![match &e.kind {
            ExprKind::Nil => CV::Rt(self.nil()),
            ExprKind::True => CV::Rt(self.boolc(true)),
            ExprKind::False => CV::Rt(self.boolc(false)),
            ExprKind::Num(x) => CV::Rt(self.numc(*x)),
            ExprKind::Str(s) => CV::Rt(self.strc(s)),
            ExprKind::Vararg => return cerr(e.line, "'...' is not supported"),
            ExprKind::Func(f) => {
                self.funcs.push(FuncVal { body: f.clone(), env: self.env.clone(), sandbox: self.sandbox });
                CV::Func(self.funcs.len() - 1)
            }
            ExprKind::Paren(x) => self.expr(x)?,
            ExprKind::Name(n) => self.name(n)?,
            ExprKind::Table(fields) => CV::Rt(self.table(fields)?),
            ExprKind::Un(op, x) => {
                let v = self.rexpr(x)?;
                self.line = e.line;
                CV::Rt(match op {
                    UnOp::Neg => {
                        let n = self.to_num(v);
                        let r = self.du(Un::NegD, n);
                        self.num(r)
                    }
                    UnOp::Not => {
                        let t = self.truthy(v);
                        let n = self.not(t);
                        self.boolean(n)
                    }
                    UnOp::Len => self.length(v),
                })
            }
            ExprKind::Bin(op, a, b) => CV::Rt(self.binary(*op, a, b, e.line)?),
            ExprKind::Index(o, k) => {
                let ov = self.expr(o)?;
                self.line = e.line;
                match ov {
                    CV::Lib(lib) => {
                        let ExprKind::Str(name) = &k.kind else {
                            return cerr(e.line, "a library indexed with a non-constant key");
                        };
                        self.lib_member(lib, name)?
                    }
                    CV::Rt(t) => {
                        let kv = self.rexpr(k)?;
                        self.line = e.line;
                        CV::Rt(self.index(t, kv)?)
                    }
                    CV::Func(_) => return cerr(e.line, "indexing a function"),
                }
            }
            ExprKind::Call(f, args) => return self.call(f, args, want, e.line),
            ExprKind::Method(o, name, args) => return self.method(o, name, args, want, e.line),
        }])
    }
    fn lib_member(&mut self, lib: Lib, name: &str) -> CR<CV> {
        Ok(match lib {
            Lib::Math => {
                if let Some(f) = math_fn(name) {
                    CV::Lib(Lib::Fn(f))
                } else if let Some((_, c)) = MATH_CONSTS.iter().find(|(n, _)| *n == name) {
                    CV::Rt(self.numc(*c))
                } else {
                    return cerr(self.line, format!("math.{} is not supported", name));
                }
            }
            Lib::Surge => match name {
                "mod" => CV::Lib(Lib::SurgeMod),
                _ => return cerr(self.line, format!("surge.{} is not supported", name)),
            },
            Lib::SurgeMod => match name {
                "Slew" => CV::Rt(self.table_const(PROTO_SLEW)),
                "ClockDivider" => CV::Rt(self.table_const(PROTO_CLOCK)),
                "AHDEnvelope" => CV::Rt(self.table_const(PROTO_AHD)),
                _ => return cerr(self.line, format!("surge.mod.{} is not supported", name)),
            },
            Lib::String => CV::Lib(Lib::Fn(match name {
                "sub" => "str.sub",
                "find" => "str.find",
                "len" => "str.len",
                "byte" => "str.byte",
                _ => return cerr(self.line, format!("string.{} is not supported", name)),
            })),
            Lib::Fn(_) => return cerr(self.line, "indexing a function"),
        })
    }
    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr, line: u32) -> CR<V> {
        if op == BinOp::And || op == BinOp::Or {
            let av = self.rexpr(a)?;
            let t = self.truthy(av);
            let r = self.var_pair();
            self.set_pair(r, av);
            let take_b = if op == BinOp::And { t } else { self.not(t) };
            self.if_else(
                take_b,
                |s| {
                    let bv = s.rexpr(b)?;
                    s.set_pair(r, bv);
                    Ok(())
                },
                |_| Ok(()),
            )?;
            return Ok(self.get_pair(r));
        }
        let av = self.rexpr(a)?;
        let bv = self.rexpr(b)?;
        self.line = line;
        Ok(match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
                let x = self.to_num(av);
                let y = self.to_num(bv);
                let r = self.arith(op, x, y);
                self.num(r)
            }
            BinOp::Concat => return cerr(line, "string concatenation ('..') is not supported"),
            BinOp::Eq | BinOp::Ne => {
                let c = self.eq(av, bv);
                let c = if op == BinOp::Ne { self.not(c) } else { c };
                self.boolean(c)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let cc = match op {
                    BinOp::Lt => Cmp::Lt,
                    BinOp::Le => Cmp::Le,
                    BinOp::Gt => Cmp::Gt,
                    _ => Cmp::Ge,
                };
                let c = self.order(cc, av, bv);
                self.boolean(c)
            }
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }
    pub fn arith(&mut self, op: BinOp, x: Val, y: Val) -> Val {
        match op {
            BinOp::Add => self.db(Bin::AddD, x, y),
            BinOp::Sub => self.db(Bin::SubD, x, y),
            BinOp::Mul => self.db(Bin::MulD, x, y),
            BinOp::Div => self.db(Bin::DivD, x, y),
            BinOp::Mod => self.fmod_floor(x, y),
            BinOp::Pow => self.b.math(MathFn::Pow, &[x, y]),
            _ => unreachable!(),
        }
    }
    /// `a - floor(a / b) * b`.
    pub fn fmod_floor(&mut self, x: Val, y: Val) -> Val {
        let q = self.db(Bin::DivD, x, y);
        let f = self.du(Un::FloorD, q);
        let m = self.db(Bin::MulD, f, y);
        self.db(Bin::SubD, x, m)
    }
    fn table(&mut self, fields: &[Field]) -> CR<V> {
        let t = self.alloc(0);
        let mut pos = 0usize;
        for (k, f) in fields.iter().enumerate() {
            match f {
                Field::Keyed(ke, ve) => {
                    let kv = self.rexpr(ke)?;
                    let vv = self.rexpr(ve)?;
                    self.line = ke.line;
                    self.tset_heap(t, kv, vv);
                }
                Field::Pos(ve) => {
                    let last = k + 1 == fields.len();
                    let vals = if last && matches!(ve.kind, ExprKind::Call(..) | ExprKind::Method(..)) { self.expr_multi(ve, 8)? } else { vec![self.expr(ve)?] };
                    for cv in vals {
                        pos += 1;
                        let v = self.rt(cv, "a table cannot hold a function")?;
                        let kv = self.numc(pos as f64);
                        self.tset_heap(t, kv, v);
                    }
                }
            }
        }
        Ok(t)
    }

    fn args(&mut self, args: &[Expr]) -> CR<Vec<CV>> {
        let mut out = Vec::new();
        for (k, a) in args.iter().enumerate() {
            if k + 1 == args.len() && matches!(a.kind, ExprKind::Call(..) | ExprKind::Method(..)) {
                out.extend(self.expr_multi(a, 8)?);
            } else {
                out.push(self.expr(a)?);
            }
        }
        Ok(out)
    }
    fn rargs(&mut self, args: &[Expr]) -> CR<Vec<V>> {
        let cvs = self.args(args)?;
        let mut out = Vec::new();
        for cv in cvs {
            out.push(self.rt(cv, "a function passed to a builtin")?);
        }
        Ok(out)
    }

    fn call(&mut self, f: &Expr, args: &[Expr], want: usize, line: u32) -> CR<Vec<CV>> {
        let fv = self.expr(f)?;
        self.line = line;
        match fv {
            CV::Func(id) => {
                let a = self.args(args)?;
                self.line = line;
                self.inline(id, a, want, None)
            }
            CV::Lib(Lib::Fn(name)) => {
                let a = self.rargs(args)?;
                self.line = line;
                self.builtin(name, a, want)
            }
            CV::Rt(_) => cerr(line, "calling a value the compiler cannot resolve (call functions by their name)"),
            CV::Lib(_) => cerr(line, "calling a library"),
        }
    }

    /// Inlines function `id` (`sandbox`: the entry's own environment).
    fn inline(&mut self, id: usize, args: Vec<CV>, want: usize, sandbox: Option<Sandbox>) -> CR<Vec<CV>> {
        let body = self.funcs[id].body.clone();
        let env = self.funcs[id].env.clone();
        let fsb = sandbox.unwrap_or(self.funcs[id].sandbox);
        let key = Rc::as_ptr(&body);
        if self.inlining.contains(&key) {
            return cerr(self.line, format!("recursive call of '{}' is not supported", body.name.as_deref().unwrap_or("function")));
        }
        if body.vararg {
            return cerr(body.line, "'...' parameters are not supported");
        }
        self.inlining.push(key);
        let mut muts = HashSet::new();
        assigned_names(&body.body, &mut muts);
        let saved_env = std::mem::replace(&mut self.env, env);
        let saved_top = std::mem::replace(&mut self.top, false);
        let saved_sb = std::mem::replace(&mut self.sandbox, fsb);
        self.mutable.push(muts);
        let r = (|| {
            let mut binds = Vec::new();
            let mut args = args.into_iter();
            for p in &body.params {
                let cv = match args.next() {
                    Some(cv) => cv,
                    None => CV::Rt(self.nil()),
                };
                let b = self.local_bind(p, cv)?;
                binds.push((p.clone(), b));
            }
            self.declare(binds);
            let nret = max_returns(&body.body).max(want.min(8));
            let mut rets = Vec::new();
            for _ in 0..nret {
                let p = self.var_pair();
                let n = self.nil();
                self.set_pair(p, n);
                rets.push(p);
            }
            self.loops.push(LoopKind::Frame);
            self.frames.push(Frame { rets: rets.clone(), loop_index: self.loops.len() - 1 });
            self.b.open_loop();
            let r = self.block(&body.body);
            let lb = self.b.close();
            self.frames.pop();
            self.loops.pop();
            r?;
            self.b.push(AStmt::Loop { cap: 1, body: lb });
            Ok(rets)
        })();
        self.mutable.pop();
        self.env = saved_env;
        self.top = saved_top;
        self.sandbox = saved_sb;
        self.inlining.pop();
        let rets = r?;
        let mut out = Vec::new();
        for p in rets.into_iter().take(want.max(1)) {
            out.push(CV::Rt(self.get_pair(p)));
        }
        Ok(out)
    }

    fn method(&mut self, o: &Expr, name: &str, args: &[Expr], want: usize, line: u32) -> CR<Vec<CV>> {
        let ov = self.expr(o)?;
        self.line = line;
        let v = self.rt(ov, &format!("method ':{}' on a library or function", name))?;
        let mut a = vec![v];
        a.extend(self.rargs(args)?);
        self.line = line;
        match name {
            "new" => Ok(vec![CV::Rt(self.helper_new(a)?)]),
            "sub" => self.builtin("str.sub", a, want),
            "find" => self.builtin("str.find", a, want),
            "len" => self.builtin("str.len", a, want),
            "byte" => self.builtin("str.byte", a, want),
            "run" => Ok(vec![CV::Rt(self.slew_run(a)?)]),
            "tick" => {
                self.clock_tick(a)?;
                Ok(vec![])
            }
            "at" => Ok(vec![CV::Rt(self.ahd_at(a)?)]),
            _ => cerr(line, format!("method ':{}' is not supported (string methods sub/find/len/byte and the helper objects' new/run/tick/at are)", name)),
        }
    }

    // -- statements ---------------------------------------------------------------
    fn block(&mut self, b: &Block) -> CR<()> {
        let saved = self.env.clone();
        let mut r = Ok(());
        for s in b {
            r = self.stat(s);
            if r.is_err() {
                break;
            }
        }
        self.env = saved;
        r
    }
    fn break_depth(&self, target: usize) -> u32 {
        (self.loops.len() - 1 - target) as u32
    }
    fn stat(&mut self, s: &Stat) -> CR<()> {
        self.line = s.line;
        match &s.kind {
            StatKind::Local(names, vals) => {
                let cvs = self.explist(vals, names.len())?;
                let mut binds = Vec::new();
                for (n, cv) in names.iter().zip(cvs) {
                    self.line = s.line;
                    let b = self.local_bind(n, cv)?;
                    binds.push((n.clone(), b));
                }
                self.declare(binds);
            }
            StatKind::LocalFunc(n, f) => {
                if self.is_mutable(n) {
                    return cerr(s.line, format!("local function '{}' is reassigned", n));
                }
                // In scope inside its own body (a recursive call is then
                // reported by the inliner).
                self.funcs.push(FuncVal { body: f.clone(), env: None, sandbox: self.sandbox });
                let id = self.funcs.len() - 1;
                self.declare(vec![(n.clone(), Bind::Static(CV::Func(id)))]);
                self.funcs[id].env = self.env.clone();
            }
            StatKind::Assign(targets, vals) => {
                enum T {
                    Name(Rc<str>),
                    Index(V, V),
                }
                // Targets' tables and keys first, then the values.
                let mut ts = Vec::new();
                for t in targets {
                    match &t.kind {
                        ExprKind::Name(n) => ts.push(T::Name(n.clone())),
                        ExprKind::Index(o, k) => {
                            let ov = self.expr(o)?;
                            let ov = self.rt(ov, "assigning into a library")?;
                            let kv = self.rexpr(k)?;
                            ts.push(T::Index(ov, kv));
                        }
                        _ => return cerr(t.line, "cannot assign to this expression"),
                    }
                }
                let cvs = self.explist(vals, ts.len())?;
                self.line = s.line;
                for (t, cv) in ts.into_iter().zip(cvs) {
                    match t {
                        T::Name(n) => self.assign_name(&n, cv)?,
                        T::Index(o, k) => {
                            let v = self.rt(cv, "a function cannot be stored in a table")?;
                            self.set_index(o, k, v)?;
                        }
                    }
                }
            }
            StatKind::Call(e) => {
                self.expr_multi(e, 0)?;
            }
            StatKind::Do(b) => self.block(b)?,
            StatKind::If(arms, els) => self.if_chain(arms, els.as_ref())?,
            StatKind::While(c, body) => {
                self.lua_loop(WHILE_CAP, |s| {
                    let cv = s.rexpr(c)?;
                    let t = s.truthy(cv);
                    let nt = s.not(t);
                    s.break_if(nt);
                    s.block(body)
                })?;
            }
            StatKind::Repeat(body, c) => {
                self.lua_loop(WHILE_CAP, |s| {
                    let saved = s.env.clone();
                    let r = (|| {
                        for st in body {
                            s.stat(st)?;
                        }
                        s.rexpr(c)
                    })();
                    s.env = saved;
                    let cv = r?;
                    let t = s.truthy(cv);
                    s.break_if(t);
                    Ok(())
                })?;
            }
            StatKind::NumFor(n, a, b, c, body) => self.num_for(n, a, b, c.as_ref(), body)?,
            StatKind::GenFor(..) => return cerr(s.line, "generic 'for ... in' loops (pairs/ipairs) are not supported"),
            StatKind::Return(es) => {
                let Some(fi) = self.frames.len().checked_sub(1) else {
                    return cerr(s.line, "'return' at the top level of the script is not supported");
                };
                let n = self.frames[fi].rets.len();
                let cvs = self.explist(es, n)?;
                for (k, cv) in cvs.into_iter().enumerate() {
                    let v = self.rt(cv, "returning a function or library")?;
                    let p = self.frames[fi].rets[k];
                    self.set_pair(p, v);
                }
                let d = self.break_depth(self.frames[fi].loop_index);
                self.b.push(AStmt::Break(d));
            }
            StatKind::Break => {
                let Some(target) = self.loops.iter().rposition(|k| *k == LoopKind::Lua) else {
                    return cerr(s.line, "'break' outside a loop");
                };
                if self.loops[target + 1..].contains(&LoopKind::Frame) {
                    return cerr(s.line, "'break' outside a loop");
                }
                let d = self.break_depth(target);
                self.b.push(AStmt::Break(d));
            }
        }
        Ok(())
    }
    /// Values for `n` targets: the last expression, a call, fills the rest.
    fn explist(&mut self, es: &[Expr], n: usize) -> CR<Vec<CV>> {
        let mut out = Vec::new();
        for (k, e) in es.iter().enumerate() {
            let last = k + 1 == es.len();
            if last && matches!(e.kind, ExprKind::Call(..) | ExprKind::Method(..)) {
                let want = n.saturating_sub(out.len()).max(1);
                let vs = self.expr_multi(e, want)?;
                out.extend(vs);
            } else {
                out.push(self.expr(e)?);
            }
        }
        out.truncate(n);
        while out.len() < n {
            out.push(CV::Rt(self.nil()));
        }
        Ok(out)
    }
    fn if_chain(&mut self, arms: &[(Expr, Block)], els: Option<&Block>) -> CR<()> {
        let Some(((c, body), rest)) = arms.split_first() else {
            return match els {
                Some(b) => self.block(b),
                None => Ok(()),
            };
        };
        let cv = self.rexpr(c)?;
        let t = self.truthy(cv);
        self.if_else(t, |s| s.block(body), |s| s.if_chain(rest, els))
    }
    fn lua_loop(&mut self, cap: u32, body: impl FnOnce(&mut Self) -> CR<()>) -> CR<()> {
        let count = self.b.var(Ty::I32);
        let z = self.ci(0);
        self.b.set(count, z);
        self.loops.push(LoopKind::Lua);
        self.b.open_loop();
        let cv = self.b.get(count);
        let over = self.cmpic(Cmp::Ge, cv, cap as i32);
        self.fail_if(over, err::LOOP_LIMIT);
        self.break_if(over);
        let c1 = self.addi(cv, 1);
        self.b.set(count, c1);
        let r = body(self);
        let lb = self.b.close();
        self.loops.pop();
        r?;
        self.b.push(AStmt::Loop { cap: cap + 1, body: lb });
        Ok(())
    }
    fn num_for(&mut self, n: &Rc<str>, a: &Expr, b: &Expr, c: Option<&Expr>, body: &Block) -> CR<()> {
        let av = self.rexpr(a)?;
        let bv = self.rexpr(b)?;
        let cv = match c {
            Some(c) => self.rexpr(c)?,
            None => self.numc(1.0),
        };
        let a0 = self.to_num(av);
        let lim = self.to_num(bv);
        let step = self.to_num(cv);
        let exact = match (self.const_f64(a0), self.const_f64(lim), self.const_f64(step)) {
            (Some(x), Some(y), Some(s)) if s != 0.0 && x.is_finite() && y.is_finite() => {
                let n = ((y - x) / s).floor() + 1.0;
                if n <= FOR_CAP as f64 {
                    Some(if n < 0.0 { 0 } else { n as u32 })
                } else {
                    None
                }
            }
            _ => None,
        };
        let i = self.b.var(Ty::F64);
        self.b.set(i, a0);
        let up = self.cmpdc(Cmp::Gt, step, 0.0);
        let mut muts = HashSet::new();
        assigned_names(body, &mut muts);
        let assigned = muts.contains(n);
        let run = |s: &mut Self| -> CR<()> {
            let iv = s.b.get(i);
            let le = s.cmpd(Cmp::Le, iv, lim);
            let ge = s.cmpd(Cmp::Ge, iv, lim);
            let ok = s.b.sel(up, le, ge);
            let stop = s.not(ok);
            s.break_if(stop);
            let nv = s.num(iv);
            let bind = if assigned {
                let p = s.var_pair();
                s.set_pair(p, nv);
                Bind::Var(p.0, p.1)
            } else {
                Bind::Static(CV::Rt(nv))
            };
            let saved = s.env.clone();
            s.declare(vec![(n.clone(), bind)]);
            let r = s.block(body);
            s.env = saved;
            r?;
            let next = s.db(Bin::AddD, iv, step);
            s.b.set(i, next);
            Ok(())
        };
        match exact {
            Some(k) => {
                self.loops.push(LoopKind::Lua);
                self.b.open_loop();
                let r = run(self);
                let lb = self.b.close();
                self.loops.pop();
                r?;
                self.b.push(AStmt::Loop { cap: k + 1, body: lb });
                Ok(())
            }
            None => {
                let zero = self.cmpdc(Cmp::Eq, step, 0.0);
                self.fail_if(zero, err::LOOP_LIMIT);
                self.lua_loop(FOR_CAP, run)
            }
        }
    }

    fn state_value(&mut self) -> V {
        self.table_const(STATE_ID as u32)
    }
}

// -- the programs -------------------------------------------------------------

/// Compiles a parsed script.
pub fn compile(chunk: &Block) -> Result<Compiled, CErr> {
    let mut info = GlobalInfo::default();
    Pre { info: &mut info, scopes: vec![HashSet::new()] }.block(chunk, true);
    let mut lay = Layout { shared_words: SLOTS_BASE, state_words: S_HEADER, ..Default::default() };
    for s in INTERNAL_STRINGS {
        lay.add(s);
    }
    for s in &info.strings {
        lay.add(s);
    }
    lay.frozen = true;
    // Record slots: the provided fields first, then every constant key.
    for n in NUM_FIELDS.iter().chain(BOOL_FIELDS).chain(OTHER_FIELDS) {
        let w = lay.state_words;
        lay.state_words += 3;
        lay.record.insert(n.to_string(), w);
    }
    for k in &info.keys {
        if !lay.record.contains_key(&**k) {
            let w = lay.state_words;
            lay.state_words += 3;
            lay.record.insert(k.to_string(), w);
        }
    }
    let mut funcs = Vec::new();
    let mut top_static = HashMap::new();

    // load: the helper prototypes, then the chunk.
    let (load, top_env) = {
        let mut cx = Cx::new(&mut lay, &mut funcs, &info, &mut top_static, None, Sandbox::Top, true);
        cx.mutable = vec![{
            let mut m = HashSet::new();
            assigned_names(chunk, &mut m);
            m
        }];
        cx.prototypes()?;
        for s in chunk {
            cx.stat(s)?;
        }
        (cx.b.finish(0), cx.env.clone())
    };
    let Some(process_id) = top_static.get("process").copied() else {
        return cerr(0, "the script defines no global function process(state)");
    };
    let init_id = top_static.get("init").copied();
    let prologue = {
        let mut cx = Cx::new(&mut lay, &mut funcs, &info, &mut top_static, None, Sandbox::Init, false);
        cx.instance_prologue();
        cx.b.finish(0)
    };
    let init = {
        let mut cx = Cx::new(&mut lay, &mut funcs, &info, &mut top_static, top_env.clone(), Sandbox::Init, false);
        if let Some(id) = init_id {
            let st = cx.state_value();
            let r = cx.inline(id, vec![CV::Rt(st)], 1, Some(Sandbox::Init))?;
            let v = match r.into_iter().next() {
                Some(CV::Rt(v)) => v,
                _ => cx.nil(),
            };
            let is_tab = cx.cmpic(Cmp::Eq, v.tag, T_TABLE);
            let is_st = cx.cmpdc(Cmp::Eq, v.num, STATE_ID);
            let ok = cx.and(is_tab, is_st);
            let bad = cx.not(ok);
            cx.fail_if(bad, err::INIT_RETURN);
        }
        cx.b.finish(0)
    };
    let process = {
        let mut cx = Cx::new(&mut lay, &mut funcs, &info, &mut top_static, top_env, Sandbox::Process, false);
        let st = cx.state_value();
        let r = cx.inline(process_id, vec![CV::Rt(st)], 1, Some(Sandbox::Process))?;
        let v = match r.into_iter().next() {
            Some(CV::Rt(v)) => v,
            _ => cx.nil(),
        };
        let is_tab = cx.cmpic(Cmp::Eq, v.tag, T_TABLE);
        let not_st = cx.cmpdc(Cmp::Ne, v.num, STATE_ID);
        let other = cx.and(is_tab, not_st);
        cx.fail_if(other, err::RETURN_TABLE);
        cx.st_slot(Region::State, S_RET, 3, None, v);
        cx.b.finish(0)
    };
    if let Some(s) = lay.late.take() {
        return cerr(0, format!("internal compiler error: string '{}' was not pooled", s));
    }
    let (gc_clear, gc_mark, gc_finish) = gc_programs(&mut lay, &mut funcs, &info, &mut top_static);
    Ok(Compiled { load, prologue, init, process, gc_clear, gc_mark, gc_finish, layout: lay })
}

impl Cx<'_> {
    /// The instance's tables: `macros` (refilled by the host) and
    /// `subscriptions = { macros = {} }` (a legacy table nothing reads).
    fn instance_prologue(&mut self) {
        let m = self.alloc(0);
        let id = self.d2i(m.num);
        self.st(Region::State, S_MACROS, 1, None, id);
        let st = self.state_value();
        let _ = self.set_field(st, "macros", m);
        let subs = self.alloc(0);
        let sm = self.alloc(0);
        self.tset_heap_named(subs, "macros", sm);
        let _ = self.set_field(st, "subscriptions", subs);
    }
    fn tset_heap_named(&mut self, t: V, name: &str, v: V) {
        let k = self.strc(name);
        self.tset_heap(t, k, v);
    }
}

/// Mark from roots: `v` (when a heap table) gets mark 1 unless marked.
fn mark_target(s: &mut Cx, v: V) {
    let is_tab = s.cmpic(Cmp::Eq, v.tag, T_TABLE);
    let st = s.cmpdc(Cmp::Eq, v.num, STATE_ID);
    let ns = s.not(st);
    let c = s.and(is_tab, ns);
    s.when(c, |s| {
        let obj = s.obj_off(v.num);
        let mo = s.addi(obj, OBJ_MARK as i32);
        let m = s.heap_ld(mo);
        let unmarked = s.cmpic(Cmp::Eq, m, 0);
        s.when(unmarked, |s| {
            let one = s.ci(1);
            s.heap_st(mo, one);
        });
    });
}

/// The collector, in three programs: clear marks (pinning the prototypes),
/// mark from one instance's state (run for each live instance), then mark
/// from the shared slots, propagate and sweep.
fn gc_programs(lay: &mut Layout, funcs: &mut Vec<FuncVal>, info: &GlobalInfo, top_static: &mut HashMap<Rc<str>, usize>) -> (Program, Program, Program) {
    let slots_end = lay.shared_words;
    let state_end = lay.state_words;
    let clear = {
        let mut cx = Cx::new(lay, funcs, info, top_static, None, Sandbox::Top, false);
        cx.count_loop(N_OBJ, |s, j| {
            let w = s.ci(OBJ_WORDS as i32);
            let off = s.ib(Bin::MulI, j, w);
            let off = s.addi(off, OBJ_MARK as i32);
            let p = s.cmpic(Cmp::Lt, j, PINNED as i32);
            let one = s.ci(1);
            let zero = s.ci(0);
            let m = s.b.sel(p, one, zero);
            s.heap_st(off, m);
        });
        cx.b.finish(0)
    };
    let mark = {
        let mut cx = Cx::new(lay, funcs, info, top_static, None, Sandbox::Top, false);
        let ret = cx.ld_slot(Region::State, S_RET, 3, None);
        mark_target(&mut cx, ret);
        let mid = cx.ld(Region::State, S_MACROS, 1, None);
        let md = cx.i2d(mid);
        let mv = cx.num(md);
        let mt = V { tag: cx.ci(T_TABLE), num: mv.num };
        mark_target(&mut cx, mt);
        let n = (state_end - S_HEADER) / 3;
        cx.count_loop(n, |s, j| {
            let three = s.ci(3);
            let off = s.ib(Bin::MulI, j, three);
            let v = s.ld_slot(Region::State, S_HEADER, state_end - S_HEADER, Some(off));
            mark_target(s, v);
        });
        cx.b.finish(0)
    };
    let finish = {
        let mut cx = Cx::new(lay, funcs, info, top_static, None, Sandbox::Top, false);
        let n = (slots_end - SLOTS_BASE) / 3;
        if n > 0 {
            cx.count_loop(n, |s, j| {
                let three = s.ci(3);
                let off = s.ib(Bin::MulI, j, three);
                let v = s.ld_slot(Region::Shared, SLOTS_BASE, slots_end - SLOTS_BASE, Some(off));
                mark_target(s, v);
            });
        }
        // Propagate: 1 = reached, 2 = scanned.
        let changed = cx.b.var(Ty::Bool);
        cx.internal_loop(N_OBJ + 2, |s| {
            let f = s.cb(false);
            s.b.set(changed, f);
            s.count_loop(N_OBJ, |s, j| {
                let w = s.ci(OBJ_WORDS as i32);
                let obj = s.ib(Bin::MulI, j, w);
                let mo = s.addi(obj, OBJ_MARK as i32);
                let m = s.heap_ld(mo);
                let alive = s.heap_ld(obj);
                let todo = s.cmpic(Cmp::Eq, m, 1);
                let al = s.cmpic(Cmp::Ne, alive, 0);
                let todo = s.and(todo, al);
                s.when(todo, |s| {
                    let two = s.ci(2);
                    s.heap_st(mo, two);
                    let t = s.cb(true);
                    s.b.set(changed, t);
                    s.count_loop(ARR, |s, k| {
                        let three = s.ci(3);
                        let e = s.ib(Bin::MulI, k, three);
                        let e = s.addi(e, OBJ_ARR as i32);
                        let e = s.ib(Bin::AddI, obj, e);
                        let v = s.heap_ld_slot(e);
                        mark_target(s, v);
                    });
                    s.count_loop(HASH, |s, k| {
                        let six = s.ci(6);
                        let e = s.ib(Bin::MulI, k, six);
                        let e = s.addi(e, OBJ_HASH as i32);
                        let e = s.ib(Bin::AddI, obj, e);
                        let kv = s.heap_ld_slot(e);
                        mark_target(s, kv);
                        let e3 = s.addi(e, 3);
                        let v = s.heap_ld_slot(e3);
                        mark_target(s, v);
                    });
                });
            });
            let c = s.b.get(changed);
            let nc = s.not(c);
            s.break_if(nc);
        });
        // Sweep.
        let live = cx.b.var(Ty::I32);
        let z = cx.ci(0);
        cx.b.set(live, z);
        cx.count_loop(N_OBJ, |s, j| {
            let w = s.ci(OBJ_WORDS as i32);
            let obj = s.ib(Bin::MulI, j, w);
            let alive = s.heap_ld(obj);
            let mo = s.addi(obj, OBJ_MARK as i32);
            let m = s.heap_ld(mo);
            let is_alive = s.cmpic(Cmp::Ne, alive, 0);
            let dead = s.cmpic(Cmp::Eq, m, 0);
            let free = s.and(is_alive, dead);
            s.when(free, |s| {
                let z = s.ci(0);
                s.heap_st(obj, z);
            });
            let keep = s.not(dead);
            let keep = s.and(is_alive, keep);
            s.when(keep, |s| {
                let l = s.b.get(live);
                let l1 = s.addi(l, 1);
                s.b.set(live, l1);
            });
        });
        let l = cx.b.get(live);
        cx.gst(G_LIVE, l);
        cx.b.finish(0)
    };
    (clear, mark, finish)
}
