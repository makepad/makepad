//! The Splash side of a score: `mod.score` (the node prototypes and the
//! functions loop code calls), evaluation of a document in its own VM, and
//! reading the result back into plain values that remember where they were
//! written. The VM stays alive with the loaded revision: the composer calls
//! the loops' functions in it, once per cycle.

use crate::theory;
use makepad_script::*;
use std::cell::RefCell;
use std::rc::Rc;

pub const DOC_FILE: &str = "score.splash";
const PRELUDE_FILE: &str = "score_prelude.splash";
/// Instructions one document evaluation may run.
const EVAL_LIMIT: usize = 4_000_000;
/// Instructions one loop cycle may run.
pub const CYCLE_LIMIT: usize = 1_000_000;

/// The node types a score is made of.
pub const NODE_TYPES: [&str; 29] = [
    "Clock", "Synth", "Drums", "Sampler", "Ironfish", "Morph", "Loop", "Stinger", "Section", "Lowpass", "Highpass", "Bandpass", "Filter", "Delay", "Reverb",
    "Drive", "Crush", "Chorus", "Comp", "Duck", "Bus", "Limiter", "Eq", "Width", "Voice", "Shader", "ShaderFx", "Instrument", "Score",
];

const PRELUDE: &str = r#"
/** A library asset by exact revision: a sample for a Sampler or a kit. */
mod.score.asset = fn(id, rev) { {ref: @asset id: id rev: rev} }
"#;

/// A value read back from the VM.
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    Id(String),
    Arr(Vec<V>),
    /// A function: its index into [`Evaluated::fns`] and where it starts.
    Fn(usize, Option<(u32, u32)>),
    Obj(Obj),
    Other(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Obj {
    /// Where the object was made (1-based line, column in the document).
    pub loc: Option<(u32, u32)>,
    pub proto: Option<String>,
    pub fields: Vec<(String, V)>,
    /// Entries written without keys (`Score{clock kit}`).
    pub items: Vec<V>,
}

impl Obj {
    pub fn get(&self, key: &str) -> Option<&V> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

impl V {
    pub fn num(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            V::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            V::Str(s) | V::Id(s) => Some(s),
            _ => None,
        }
    }
}

/// One played thing a loop's code asked for, relative to its cycle.
#[derive(Clone, Debug, PartialEq)]
pub struct Emit {
    pub beat: f64,
    pub what: EmitKind,
    pub vel: f32,
    pub pan: f32,
    /// Beats held; 0 plays the patch's own gate.
    pub dur: f32,
    pub opts: makepad_score_player::NoteOpts,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EmitKind {
    Note(f32),
    /// A drum: kit slot, General MIDI note, tuning (semitones).
    Hit(u8, u8, f32),
    Sample(f32),
    /// A sung syllable: note, phonemes, choir part.
    Sung(f32, makepad_score_player::sung::PhonemeSeq, u8),
    /// A whole sung line as a neural phrase, by key (its score is in the
    /// phrase book): its audio starts here, its lead before the first note.
    /// Only a loop on a neural voice plays it.
    Phrase(u64),
    /// The sustain pedal to `vel` (0 up .. 1 down).
    Pedal,
    /// A playing note (its handle in `opts.handle`) glides over `dur` beats
    /// to a note (NaN: stays), by semitones (`opts.glide_from`), a level
    /// (`vel`, NaN: stays) and a cutoff (`opts.cutoff`).
    Glide(f32),
}

/// A loop's cues in its last cycle: `wait` expects them again every
/// `len` beats from `start` (the cycle repeating as it last played).
#[derive(Clone, Debug, PartialEq)]
pub struct CuePlan {
    pub name: String,
    pub loop_key: u64,
    pub start: f64,
    pub len: f64,
    pub offsets: Vec<f64>,
}

impl CuePlan {
    /// The first time this cue comes at or after `beat`.
    pub fn next_at(&self, beat: f64) -> Option<f64> {
        let len = self.len.max(1e-6);
        self.offsets.iter().map(|off| {
            let first = self.start + off;
            let k = ((beat - 1e-9 - first) / len).ceil().max(0.0);
            first + k * len
        }).min_by(f64::total_cmp)
    }
}

/// What loop code reads and writes while one cycle runs.
#[derive(Default)]
pub struct Ctx {
    pub active: bool,
    /// Beats from the cycle's start.
    pub cursor: f64,
    pub emits: Vec<Emit>,
    /// The cycle's random stream.
    pub rng: u64,
    /// Named tick counters (`tick()` is the counter `""`).
    pub ticks: Vec<(String, i64)>,
    pub params: Vec<(String, f32)>,
    pub cues: Vec<(String, f64)>,
    pub cycle: u64,
    /// Loop defaults for notes.
    pub amp: f32,
    pub dur: f32,
    pub errors: Vec<String>,
    /// The tempo (sung consonants are timed in seconds).
    pub bpm: f64,
    /// The cycle's start (beats from the piece's start), the bar length,
    /// and the section playing then with the beat it started.
    pub start: f64,
    pub beats_per_bar: f64,
    pub section: Option<(String, f64)>,
    /// Beat-stamped values loops share (`put`/`get`), kept by the composer
    /// across cycles: (key, beat, value).
    pub shared: Vec<(String, f64, Stored)>,
    /// What `log` printed this cycle.
    pub logs: Vec<String>,
    /// The handles `play` gave out this cycle.
    pub handles: u32,
    /// Where each loop cued in its last cycle, for `wait` (see [`CuePlan`]).
    pub cue_plans: Vec<CuePlan>,
    /// The loop's fx() scopes (chains, outermost first), the ones open now,
    /// the emits each scope played (`from..to` of `emits`, scope `k` from
    /// 1; inner scopes are recorded first), and the settings fx() gave
    /// (scope, param index, value, beat from the cycle's start).
    pub scopes: Vec<Vec<makepad_score_player::FxKind>>,
    pub scope_stack: Vec<makepad_score_player::FxKind>,
    pub routed: Vec<(usize, usize, usize)>,
    pub fx_sets: Vec<(usize, u8, f32, f64)>,
    /// MIDI in: the keys held (note, velocity; lowest first) and each
    /// controller's last value (0..1).
    pub held: Vec<(u8, f32)>,
    pub ccs: Vec<f32>,
    /// OSC: what arrived last per address, and what `osc_send` sends this
    /// cycle (beat from the cycle's start, address, arguments, target).
    pub osc_values: Vec<(String, Vec<crate::osc::OscArg>)>,
    pub osc_sends: Vec<(f64, String, Vec<crate::osc::OscArg>, Option<String>)>,
    /// What `sing` sang this cycle, for the lyric display.
    pub lyrics: Vec<makepad_score_player::sung::SyllableSpan>,
}

impl Ctx {
    pub fn rand(&mut self) -> f64 {
        // splitmix64: one draw, one step; the stream is the cycle's.
        self.rng = self.rng.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    fn tick(&mut self, name: &str, step: i64) -> i64 {
        match self.ticks.iter_mut().find(|(n, _)| n == name) {
            Some((_, value)) => {
                *value += step;
                *value
            }
            None => {
                self.ticks.push((name.to_string(), step - 1 + if step > 0 { 0 } else { 1 }));
                self.ticks.last().unwrap().1
            }
        }
    }
}

pub type Shared = Rc<RefCell<Ctx>>;

/// A value `put` shares: a number, a list of numbers, or text.
#[derive(Clone, Debug, PartialEq)]
pub enum Stored {
    Num(f64),
    Nums(Vec<f64>),
    Text(String),
}

/// A document evaluated in its VM, the VM kept for calling its functions.
pub struct Evaluated {
    pub host: Box<ScriptVmHost<(), ()>>,
    pub bx: Option<Box<ScriptVmBase>>,
    pub errors: Vec<String>,
    pub fns: Vec<ScriptValue>,
    /// Keeps every function object alive across collections.
    pub keep: Vec<ScriptObjectRef>,
    pub root: V,
    /// Every node object met, with where it was made.
    pub objects: Vec<Obj>,
    pub ctx: Shared,
}

impl Evaluated {
    pub fn with_vm<R>(&mut self, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
        let bx = self.bx.take().expect("the score's VM is in use");
        let mut vm = ScriptVm {
            host: self.host.host_context(),
            bx,
        };
        let out = f(&mut vm);
        self.bx = Some(vm.bx);
        out
    }

    /// Call function `index` with `args` under the cycle limit; the errors
    /// it raised.
    pub fn call(&mut self, index: usize, args: &[f64]) -> Result<ScriptValue, String> {
        let Some(function) = self.fns.get(index).copied() else { return Err("no such function".into()) };
        self.with_vm(|vm| {
            vm.capture_errors();
            let args: Vec<ScriptValue> = args.iter().map(|a| (*a).into()).collect();
            let value = vm.with_instruction_limit(CYCLE_LIMIT, |vm| vm.call(function, &args));
            let errors = vm.take_errors();
            match errors.first() {
                Some(error) => Err(error.clone()),
                None => Ok(value),
            }
        })
    }

    /// A value returned by a call, as a bool (for `when:`).
    pub fn truthy(value: ScriptValue) -> bool {
        if let Some(b) = value.as_bool() {
            return b;
        }
        if let Some(n) = value.as_number() {
            return n != 0.0;
        }
        !value.is_nil()
    }
}

/// An id whose text reads back.
fn lut_id(name: &str) -> LiveId {
    LiveId::from_str_with_lut(name).unwrap_or_else(|_| LiveId::from_str(name))
}

fn id_string(id: LiveId) -> String {
    id.as_string(|s| s.map(str::to_string)).unwrap_or_else(|| format!("{id}"))
}

/// Evaluate a document in a fresh VM with `mod.score`.
pub fn evaluate(source: &str) -> Evaluated {
    let mut host = Box::new(ScriptVmHost::new((), ()));
    let bx = Box::new(ScriptVmBase::new());
    let ctx: Shared = Rc::new(RefCell::new(Ctx::default()));
    let (errors, fns, keep, root, objects, bx) = {
        let mut vm = ScriptVm {
            host: host.host_context(),
            bx,
        };
        vm.set_script_domain("score");
        let module = vm.new_module(id!(score));
        for name in NODE_TYPES.iter() {
            let proto = vm.bx.heap_mut().new_object();
            let tag = lut_id(name);
            vm.bx.heap_mut().set_value_def(proto, id_lut!(__proto).into(), ScriptValue::from_id(tag));
            vm.bx.heap_mut().set_value_def(module, tag.into(), proto.into());
        }
        install(&mut vm, module, ctx.clone());
        install_toolkit(&mut vm, module, ctx.clone());
        vm.capture_errors();
        vm.eval(ScriptMod::new("", "", &(PRELUDE_FILE), 0, 0, &(PRELUDE), &[]));
        let prelude_errors = vm.take_errors();
        debug_assert!(prelude_errors.is_empty(), "mod.score prelude: {prelude_errors:?}");
        vm.capture_errors();
        let value = vm.with_instruction_limit(EVAL_LIMIT, |vm| {
            // Rows count from 1 like an editor's lines; columns from 0.
            vm.eval(ScriptMod::new("", "", &(DOC_FILE), 1, 0, &(source), &[]))
        });
        let errors = vm.take_errors();
        let mut reader = Reader { vm: &vm, fns: Vec::new(), objects: Vec::new() };
        let root = reader.read(value, 0);
        let (fns, objects) = (reader.fns, reader.objects);
        let keep = fns.iter().filter_map(|f| f.as_object()).map(|o| vm.bx.heap_mut().new_object_ref(o)).collect();
        (errors, fns, keep, root, objects, vm.bx)
    };
    Evaluated { host, bx: Some(bx), errors, fns, keep, root, objects, ctx }
}

/// `file:line:col: message (origin)` as a location in the document and the
/// message alone.
pub fn split_error(error: &str) -> (u32, u32, String) {
    let mut parts = error.splitn(4, ':');
    let (file, line, col, rest) = (parts.next(), parts.next(), parts.next(), parts.next());
    let strip = |message: &str| {
        let message = match message.rfind(" (platform/script/") {
            Some(at) if message.ends_with(')') => &message[..at],
            _ => message,
        };
        // "variable x not found in scope. Did you mean: <everything>": one
        // near miss among the score's words, or none.
        match message.strip_prefix("variable ").and_then(|m| m.split_once(" not found in scope")) {
            Some((name, _)) => {
                let words = NODE_TYPES.iter().copied().chain(crate::assist::FUNCTIONS.iter().map(|(f, _)| *f));
                format!("`{name}` is not defined here{}", crate::assist::hint(name, words))
            }
            None => message.to_string(),
        }
    };
    match (file, line.and_then(|l| l.trim().parse::<u32>().ok()), col.and_then(|c| c.trim().parse::<u32>().ok()), rest) {
        (Some(file), Some(line), Some(col), Some(rest)) if file.ends_with(DOC_FILE) => (line.max(1), col + 1, strip(rest.trim())),
        _ => (1, 1, strip(error)),
    }
}

const MAX_DEPTH: usize = 48;

struct Reader<'a, 'b> {
    vm: &'a ScriptVm<'b>,
    fns: Vec<ScriptValue>,
    objects: Vec<Obj>,
}

impl Reader<'_, '_> {
    fn loc_of(&self, obj: ScriptObject) -> Option<(u32, u32)> {
        // The document's rows count from 1 (see `evaluate`); columns from 0.
        let loc = self.vm.bx.code_ref().ip_to_loc(self.vm.bx.heap_ref().made_at(obj))?;
        (loc.file().ends_with(DOC_FILE)).then_some((loc.line(), loc.col() + 1))
    }

    fn read(&mut self, value: ScriptValue, depth: usize) -> V {
        if value.is_nil() {
            return V::Nil;
        }
        if let Some(b) = value.as_bool() {
            return V::Bool(b);
        }
        if let Some(n) = value.as_number() {
            return V::Num(n);
        }
        if let Some(id) = value.as_id() {
            return V::Id(id_string(id));
        }
        if value.is_string_like() {
            if let Some(s) = value.as_string() {
                return V::Str(self.vm.bx.heap_ref().string(s).to_string());
            }
            if let Some(s) = value.as_inline_string(|s| s.to_string()) {
                return V::Str(s);
            }
        }
        if depth >= MAX_DEPTH {
            return V::Other("nesting too deep".into());
        }
        if let Some(array) = value.as_array() {
            let len = self.vm.bx.heap_ref().array_len(array);
            return V::Arr((0..len).map(|i| {
                let item = self.vm.bx.heap_ref().array_index(array, i, NoTrap);
                self.read(item, depth + 1)
            }).collect());
        }
        if let Some(object) = value.as_object() {
            if self.vm.bx.heap_ref().is_fn(object) {
                let loc = self.loc_of(object);
                self.fns.push(value);
                return V::Fn(self.fns.len() - 1, loc);
            }
            return self.read_object(object, depth);
        }
        V::Other(format!("{:?}", value.value_type()))
    }

    fn read_object(&mut self, object: ScriptObject, depth: usize) -> V {
        let mut chain = Vec::new();
        let mut at = Some(object);
        while let Some(obj) = at {
            if chain.len() > MAX_DEPTH {
                break;
            }
            chain.push(obj);
            at = self.vm.bx.heap_ref().proto(obj).as_object();
        }
        let mut proto = None;
        let mut raw: Vec<(String, ScriptValue)> = Vec::new();
        let mut items = Vec::new();
        for (level, obj) in chain.iter().enumerate() {
            let heap = self.vm.heap();
            let mut fields: Vec<(String, ScriptValue)> = Vec::new();
            heap.map_entries(*obj).iter().for_each(|e| fields.push((key_string(e.key), e.value)));
            for index in 0..heap.vec_len(*obj) {
                let entry = heap.vec_key_value(*obj, index, NoTrap);
                if entry.key.is_nil() {
                    if level == 0 {
                        items.push(entry.value);
                    }
                } else {
                    fields.push((key_string(entry.key), entry.value));
                }
            }
            for (key, value) in fields {
                if key == "__proto" {
                    if proto.is_none() {
                        proto = value.as_id().map(id_string);
                    }
                    continue;
                }
                // Prototype fields (the module's functions) are not the
                // node's own settings.
                if level > 0 && proto.is_some() {
                    continue;
                }
                if !raw.iter().any(|(k, _)| *k == key) {
                    raw.push((key, value));
                }
            }
        }
        if raw.is_empty() && proto.is_none() && !items.is_empty() {
            return V::Arr(items.into_iter().map(|v| self.read(v, depth + 1)).collect());
        }
        let loc = self.loc_of(object);
        let fields: Vec<(String, V)> = raw.into_iter().map(|(key, value)| {
            let value = self.read(value, depth + 1);
            (key, value)
        }).collect();
        let items: Vec<V> = items.into_iter().map(|v| self.read(v, depth + 1)).collect();
        let obj = Obj { loc, proto, fields, items };
        if obj.proto.is_some() {
            self.objects.push(obj.clone());
        }
        V::Obj(obj)
    }
}

fn key_string(key: ScriptValue) -> String {
    if let Some(id) = key.as_id() {
        return id_string(id);
    }
    if let Some(n) = key.as_number() {
        return format!("{n}");
    }
    if let Some(s) = key.as_inline_string(|s| s.to_string()) {
        return s;
    }
    format!("{key:?}")
}

// --- The functions loop code calls -------------------------------------

fn arg(vm: &ScriptVm, args: ScriptObject, key: LiveId) -> ScriptValue {
    vm.bx.heap_ref().value(args, key.into(), NoTrap)
}

fn num_of(vm: &ScriptVm, value: ScriptValue) -> Option<f64> {
    value.as_number().or_else(|| value.as_bool().map(|b| if b { 1.0 } else { 0.0 })).or_else(|| {
        // A note name as a number.
        text_of(vm, value).and_then(|t| theory::note_number(&t).map(|n| n as f64))
    })
}

fn text_of(vm: &ScriptVm, value: ScriptValue) -> Option<String> {
    if let Some(id) = value.as_id() {
        return Some(id_string(id));
    }
    if let Some(s) = value.as_string() {
        return Some(vm.bx.heap_ref().string(s).to_string());
    }
    value.as_inline_string(|s| s.to_string())
}

/// Every element of an array value (a non-array is one element).
fn elements(vm: &ScriptVm, value: ScriptValue) -> Vec<ScriptValue> {
    match value.as_array() {
        Some(array) => (0..vm.bx.heap_ref().array_len(array)).map(|i| vm.bx.heap_ref().array_index(array, i, NoTrap)).collect(),
        None => {
            if value.is_nil() { Vec::new() } else { vec![value] }
        }
    }
}

fn new_array(vm: &mut ScriptVm, values: impl IntoIterator<Item = ScriptValue>) -> ScriptValue {
    let array = vm.bx.heap_mut().new_array();
    for value in values {
        vm.bx.heap_mut().array_push_unchecked(array, value);
    }
    array.into()
}

/// A note argument: a number, a name (`@c4`), or an array (a chord).
fn notes_of(vm: &ScriptVm, value: ScriptValue) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    for element in elements(vm, value) {
        if element.is_nil() {
            continue;
        }
        if let Some(n) = element.as_number() {
            out.push(n as f32);
        } else if let Some(text) = text_of(vm, element) {
            if text == "rest" || text == "r" {
                continue;
            }
            out.push(theory::note_number(&text).ok_or_else(|| format!("`{text}` is not a note (c4, fs2, eb3 or a MIDI number)"))?);
        } else if element.as_array().is_some() {
            out.extend(notes_of(vm, element)?);
        } else {
            return Err("a note is a number, a name like @c4, or an array of them".into());
        }
    }
    Ok(out)
}

struct Opts {
    amp: Option<f32>,
    pan: f32,
    dur: Option<f32>,
    note: makepad_score_player::NoteOpts,
}

fn opts_of(vm: &ScriptVm, value: ScriptValue) -> Opts {
    let mut opts = Opts { amp: None, pan: 0.0, dur: None, note: Default::default() };
    let Some(object) = value.as_object() else { return opts };
    let get = |name: &str| vm.bx.heap_ref().value(object, lut_id(name).into(), NoTrap).as_number().map(|n| n as f32);
    opts.amp = get("amp").or_else(|| get("vel"));
    opts.pan = get("pan").unwrap_or(0.0).clamp(-1.0, 1.0);
    opts.dur = get("dur").or_else(|| get("len"));
    let n = &mut opts.note;
    for (name, slot) in [
        ("cutoff", &mut n.cutoff), ("res", &mut n.res), ("attack", &mut n.attack), ("decay", &mut n.decay),
        ("sustain", &mut n.sustain), ("release", &mut n.release), ("drive", &mut n.drive), ("detune", &mut n.detune),
    ] {
        if let Some(value) = get(name) {
            *slot = value;
        }
    }
    // Sample playback: rate, a region, an onset, fit / stretch to beats, loop.
    for (name, slot) in [
        ("rate", &mut n.rate), ("start", &mut n.start), ("finish", &mut n.finish), ("onset", &mut n.onset),
        ("fit", &mut n.fit), ("stretch", &mut n.stretch),
    ] {
        if let Some(value) = get(name) {
            *slot = value;
        }
    }
    // slice: k of slices: n equal parts (8 when not given).
    if let Some(k) = get("slice") {
        let count = get("slices").unwrap_or(8.0).max(1.0).round();
        let k = k.round().rem_euclid(count);
        n.start = k / count;
        n.finish = (k + 1.0) / count;
    }
    n.looped = vm.bx.heap_ref().value(object, lut_id("looped").into(), NoTrap).as_bool().unwrap_or(false);
    // env: [beats level …] (flat or pairs) on the level, cutoff_env: the
    // same in Hz; curve bends their segments.
    let curve = get("curve").unwrap_or(1.0).clamp(0.1, 10.0);
    let env_of = |name: &str| -> makepad_score_player::Env {
        let value = vm.bx.heap_ref().value(object, lut_id(name).into(), NoTrap);
        let mut flat: Vec<f32> = Vec::new();
        for item in elements(vm, value) {
            match item.as_number() {
                Some(v) => flat.push(v as f32),
                None => flat.extend(elements(vm, item).iter().filter_map(|v| v.as_number()).map(|v| v as f32)),
            }
        }
        let mut env = makepad_score_player::Env { curve, ..Default::default() };
        let mut last = f32::NEG_INFINITY;
        for pair in flat.chunks_exact(2).take(makepad_score_player::ENV_POINTS) {
            let t = pair[0].max(last).max(0.0);
            env.pts[env.n as usize] = [t, pair[1]];
            env.n += 1;
            last = t;
        }
        env
    };
    n.env = env_of("env");
    n.cutoff_env = env_of("cutoff_env");
    // bend: [beats semitones …] the pitch over the note; vib: [rate depth
    // delay]; arp: [semitones …] stepped every arp_step beats.
    n.bend = env_of("bend");
    let list_of = |name: &str| -> Vec<f32> {
        let value = vm.bx.heap_ref().value(object, lut_id(name).into(), NoTrap);
        elements(vm, value).iter().filter_map(|v| v.as_number()).map(|v| v as f32).collect()
    };
    let vib = list_of("vib");
    if let [rate, depth, rest @ ..] = vib.as_slice() {
        n.vib_rate = rate.clamp(0.0, 100.0);
        n.vib_depth = depth.clamp(0.0, 24.0);
        n.vib_delay = rest.first().copied().unwrap_or(0.0).max(0.0);
    }
    let arp = list_of("arp");
    if !arp.is_empty() {
        for (slot, v) in n.arp.iter_mut().zip(&arp) {
            *slot = v.clamp(-48.0, 48.0);
        }
        n.arp_n = arp.len().min(makepad_score_player::ARP_STEPS) as u8;
        n.arp_step = get("arp_step").unwrap_or(0.0625).max(1e-3);
    }
    if n.env.n > 0 {
        // The envelope shapes the note: the patch's own opens at once and
        // holds, and the note lasts to its last point unless told.
        let end = n.env.pts[n.env.n as usize - 1][0];
        for (slot, value) in [(&mut n.attack, 0.002), (&mut n.decay, 0.0), (&mut n.sustain, 1.0)] {
            if slot.is_nan() {
                *slot = value;
            }
        }
        if opts.dur.is_none() && end > 0.0 {
            opts.dur = Some(end);
        }
    }
    if n.cutoff_env.n > 0 && n.cutoff.is_nan() {
        // A cutoff on the note gives an unfiltered patch its filter.
        n.cutoff = n.cutoff_env.pts[0][1];
    }
    opts
}

fn raise(vm: &mut ScriptVm, message: String) -> ScriptValue {
    script_err_invalid_args!(vm.trap(), "{}", message)
}

fn install(vm: &mut ScriptVm, module: ScriptObject, ctx: Shared) {
    // play(note, {amp pan dur cutoff res attack decay sustain release drive detune})
    let c = ctx.clone();
    vm.add_method(module, id_lut!(play), script_args!(note = NIL, opts = NIL), move |vm, args| {
        let notes = match notes_of(vm, arg(vm, args, id!(note))) {
            Ok(notes) => notes,
            Err(message) => return raise(vm, message),
        };
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (beat, amp, dur) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp), opts.dur.unwrap_or(ctx.dur));
        // A handle for glide(): unique in the loop for 4096 cycles.
        ctx.handles += 1;
        let handle = ((ctx.cycle as u32 & 0xfff) << 20) | (ctx.handles & 0xf_ffff);
        let note_opts = makepad_score_player::NoteOpts { handle, ..opts.note };
        for note in notes {
            ctx.emits.push(Emit { beat, what: EmitKind::Note(note), vel: amp, pan: opts.pan, dur, opts: note_opts });
        }
        (handle as f64).into()
    });
    // glide(h, {note by amp cutoff}, beats): a note play() started slides
    // from here, over `beats`, to a note (or by semitones), a level, a
    // cutoff; whatever is not named stays.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(glide), script_args!(handle = 0.0, to = NIL, beats = 1.0), move |vm, args| {
        let handle = arg(vm, args, id!(handle)).as_number().unwrap_or(0.0) as u32;
        let to = arg(vm, args, id!(to));
        let get = |name: &str| to.as_object().map(|o| vm.bx.heap_ref().value(o, lut_id(name).into(), NoTrap)).filter(|v| !v.is_nil());
        let note = get("note").and_then(|v| num_of(vm, v)).map_or(f32::NAN, |n| n as f32);
        let by = get("by").and_then(|v| v.as_number()).map_or(f32::NAN, |n| n as f32);
        let amp = get("amp").and_then(|v| v.as_number()).map_or(f32::NAN, |n| n as f32);
        let cutoff = get("cutoff").and_then(|v| v.as_number()).map_or(f32::NAN, |n| n as f32);
        let beats = arg(vm, args, id!(beats)).as_number().unwrap_or(1.0).max(0.0) as f32;
        let mut ctx = c.borrow_mut();
        if !ctx.active || handle == 0 {
            return NIL;
        }
        let beat = ctx.cursor;
        let opts = makepad_score_player::NoteOpts { cutoff, glide_from: by, handle, ..Default::default() };
        ctx.emits.push(Emit { beat, what: EmitKind::Glide(note), vel: amp, pan: 0.0, dur: beats, opts });
        NIL
    });
    // hit(@kick, {amp pan tune})
    let c = ctx.clone();
    vm.add_method(module, id_lut!(hit), script_args!(drum = NIL, opts = NIL), move |vm, args| {
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let tune = arg(vm, args, id!(opts)).as_object().and_then(|o| vm.bx.heap_ref().value(o, id!(tune).into(), NoTrap).as_number()).unwrap_or(0.0) as f32;
        let mut drums = Vec::new();
        for element in elements(vm, arg(vm, args, id!(drum))) {
            let drum = match text_of(vm, element) {
                Some(name) => match makepad_score_player::presets::drum(&name) {
                    Some((slot, gm)) => (slot as u8, gm),
                    None => return raise(vm, format!("no drum `{name}` (drums: {}, and the GM kit's pedalhat, floortom, tom1-tom3, bell)", makepad_score_player::presets::DRUM_SLOTS.join(", "))),
                },
                None => match element.as_number().and_then(|n| gm_slot(n).map(|slot| (slot, n.round().clamp(0.0, 127.0) as u8))) {
                    Some(drum) => drum,
                    None => return raise(vm, "hit() takes a drum name like @kick".into()),
                },
            };
            drums.push(drum);
        }
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (beat, amp) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp));
        for (slot, gm) in drums {
            ctx.emits.push(Emit { beat, what: EmitKind::Hit(slot, gm, tune), vel: amp, pan: opts.pan, dur: 0.0, opts: opts.note });
        }
        NIL
    });
    // sample(note = root, {amp pan dur})
    let c = ctx.clone();
    vm.add_method(module, id_lut!(sample), script_args!(note = NIL, opts = NIL), move |vm, args| {
        let note = arg(vm, args, id!(note));
        let notes = if note.is_nil() { vec![f32::NAN] } else {
            match notes_of(vm, note) {
                Ok(notes) => notes,
                Err(message) => return raise(vm, message),
            }
        };
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (beat, amp) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp));
        for note in notes {
            ctx.emits.push(Emit { beat, what: EmitKind::Sample(note), vel: amp, pan: opts.pan, dur: opts.dur.unwrap_or(0.0), opts: opts.note });
        }
        NIL
    });
    // sing([@e4 @g4 @a4], "a-ma-zing", {dur durs vel pan}): a sung line
    // from here, one syllable per note, without moving; returns its length
    // in beats (so `sleep(sing(...))` follows it).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(sing), script_args!(notes = NIL, lyrics = NIL, opts = NIL), move |vm, args| {
        use makepad_score_player::sung::{builtin_pronounce, lower_sing, NoteIn, SingOpts};
        let lyrics = text_of(vm, arg(vm, args, id!(lyrics))).unwrap_or_default();
        let opts_value = arg(vm, args, id!(opts));
        let opts = opts_of(vm, opts_value);
        let durs: Vec<f64> = opts_value.as_object().map(|o| vm.bx.heap_ref().value(o, id!(durs).into(), NoTrap)).map_or_else(Vec::new, |v| elements(vm, v).into_iter().filter_map(|e| e.as_number()).collect());
        let mut line = Vec::new();
        let mut rests = Vec::new();
        for element in elements(vm, arg(vm, args, id!(notes))) {
            let rest = element.is_nil() || text_of(vm, element).is_some_and(|t| t == "_" || t == "rest" || t == "r");
            if rest {
                rests.push(line.len() + rests.len());
                line.push(None);
                continue;
            }
            match notes_of(vm, element) {
                Ok(pitches) if !pitches.is_empty() => line.push(Some(pitches)),
                Ok(_) => line.push(None),
                Err(message) => return raise(vm, message),
            }
        }
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let dur = opts.dur.map(|d| d as f64).unwrap_or(if ctx.dur > 0.0 { ctx.dur as f64 } else { 1.0 });
        let (start, vel) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp).clamp(0.0, 1.0));
        let mut notes = Vec::new();
        let mut t = 0.0;
        for (i, pitches) in line.into_iter().enumerate() {
            let len = durs.get(i).copied().unwrap_or(dur).max(1.0 / 64.0);
            if let Some(mut pitches) = pitches {
                // The highest is the melody.
                pitches.sort_by(|a, b| b.total_cmp(a));
                notes.push(NoteIn { beat: start + t, dur: len, pitches, vel });
            }
            t += len;
        }
        let parts = notes.iter().map(|n| n.pitches.len()).max().unwrap_or(1).clamp(1, 8) as u8;
        let sing_opts = SingOpts { bpm: ctx.bpm, lead: true, min_beat: 0.0, chords: Vec::new(), parts };
        let lowered = lower_sing(&notes, &lyrics, &sing_opts, &builtin_pronounce);
        for event in &lowered.events {
            let syl = lowered.table.get(event.syllable as usize).copied().unwrap_or_else(|| makepad_score_player::sung::PhonemeSeq::ipa("ɑ"));
            ctx.emits.push(Emit { beat: event.beat, what: EmitKind::Sung(event.note, syl, event.part), vel: event.vel, pan: opts.pan, dur: event.dur as f32, opts: opts.note });
        }
        // The line as a neural phrase too (a loop on a neural voice plays it;
        // the others drop it): its score goes in the phrase book.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some((key, score, first)) = makepad_score_player::sung::neural::phrase_of(&lowered, ctx.bpm, makepad_score_player::sung::neural::SINGER) {
            makepad_score_player::sung::neural::remember(key, score);
            let lead = makepad_score_player::sung::neural::LEAD as f64 * ctx.bpm / 60.0;
            ctx.emits.push(Emit { beat: first - lead, what: EmitKind::Phrase(key), vel, pan: opts.pan, dur: 0.0, opts: opts.note });
        }
        ctx.lyrics.extend(lowered.syllables);
        ScriptValue::from_f64(t)
    });
    // hits("x..x", @kick, step = 0.25): a pattern from here, without moving.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(hits), script_args!(pattern = NIL, drum = NIL, step = 0.25), move |vm, args| {
        let Some(text) = text_of(vm, arg(vm, args, id!(pattern))) else { return raise(vm, "hits() takes a pattern string like \"x..x\"".into()) };
        let step = arg(vm, args, id!(step)).as_number().unwrap_or(0.25).max(1.0 / 64.0);
        let drum = text_of(vm, arg(vm, args, id!(drum))).unwrap_or_else(|| "kick".into());
        let Some((slot, gm)) = makepad_score_player::presets::drum(&drum) else { return raise(vm, format!("no drum `{drum}`")) };
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (base, amp) = (ctx.cursor, ctx.amp);
        for (i, vel) in theory::pattern(&text).into_iter().enumerate() {
            if let Some(vel) = vel {
                ctx.emits.push(Emit { beat: base + i as f64 * step, what: EmitKind::Hit(slot as u8, gm, 0.0), vel: vel * amp / 0.8, pan: 0.0, dur: 0.0, opts: Default::default() });
            }
        }
        NIL
    });
    // pedal(1) down, pedal(0) up, pedal(0.5) half: a piano's sustain pedal.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(pedal), script_args!(value = 1.0), move |vm, args| {
        let value = arg(vm, args, id!(value)).as_number().unwrap_or(1.0);
        if !value.is_finite() {
            return raise(vm, "pedal() takes 0 (up) to 1 (down)".into());
        }
        let mut ctx = c.borrow_mut();
        if ctx.active {
            let beat = ctx.cursor;
            ctx.emits.push(Emit { beat, what: EmitKind::Pedal, vel: value.clamp(0.0, 1.0) as f32, pan: 0.0, dur: 0.0, opts: Default::default() });
        }
        NIL
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(sleep), script_args!(beats = 1.0), move |vm, args| {
        let beats = arg(vm, args, id!(beats)).as_number().unwrap_or(0.0);
        if !beats.is_finite() || beats < 0.0 {
            return raise(vm, "sleep() takes a length in beats, 0 or more".into());
        }
        c.borrow_mut().cursor += beats;
        NIL
    });
    // tick() the counter; tick(arr) the next element (a ring); tick(arr, @name).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(tick), script_args!(ring = NIL, name = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(name))).unwrap_or_default();
        let ring = arg(vm, args, id!(ring));
        let items = elements(vm, ring);
        let at = c.borrow_mut().tick(&name, 1);
        if ring.is_nil() {
            return (at as f64).into();
        }
        if items.is_empty() { NIL } else { items[at.rem_euclid(items.len() as i64) as usize] }
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(look), script_args!(ring = NIL, name = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(name))).unwrap_or_default();
        let ring = arg(vm, args, id!(ring));
        let items = elements(vm, ring);
        let at = c.borrow_mut().tick(&name, 0).max(0);
        if ring.is_nil() {
            return (at as f64).into();
        }
        if items.is_empty() { NIL } else { items[at.rem_euclid(items.len() as i64) as usize] }
    });
    vm.add_method(module, id_lut!(ring), script_args!(items = NIL), move |vm, args| {
        let items = elements(vm, arg(vm, args, id!(items)));
        new_array(vm, items)
    });
    vm.add_method(module, id_lut!(at), script_args!(ring = NIL, index = 0.0), move |vm, args| {
        let items = elements(vm, arg(vm, args, id!(ring)));
        let index = arg(vm, args, id!(index)).as_number().unwrap_or(0.0).floor() as i64;
        if items.is_empty() { NIL } else { items[index.rem_euclid(items.len() as i64) as usize] }
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(choose), script_args!(items = NIL), move |vm, args| {
        let items = elements(vm, arg(vm, args, id!(items)));
        if items.is_empty() {
            return NIL;
        }
        let r = c.borrow_mut().rand();
        items[((r * items.len() as f64) as usize).min(items.len() - 1)]
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(shuffle), script_args!(items = NIL), move |vm, args| {
        let mut items = elements(vm, arg(vm, args, id!(items)));
        for i in (1..items.len()).rev() {
            let j = ((c.borrow_mut().rand() * (i + 1) as f64) as usize).min(i);
            items.swap(i, j);
        }
        new_array(vm, items)
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(rand), script_args!(lo = 0.0, hi = 1.0), move |vm, args| {
        let lo = arg(vm, args, id!(lo)).as_number().unwrap_or(0.0);
        let hi = arg(vm, args, id!(hi)).as_number().unwrap_or(1.0);
        (lo + (hi - lo) * c.borrow_mut().rand()).into()
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(irand), script_args!(lo = 0.0, hi = 1.0), move |vm, args| {
        let lo = arg(vm, args, id!(lo)).as_number().unwrap_or(0.0).floor();
        let hi = arg(vm, args, id!(hi)).as_number().unwrap_or(1.0).floor();
        (lo + ((hi - lo + 1.0) * c.borrow_mut().rand()).floor().min(hi - lo)).into()
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(one_in), script_args!(n = 2.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(2.0).max(1.0);
        (c.borrow_mut().rand() * n < 1.0).into()
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(chance), script_args!(p = 0.5), move |vm, args| {
        let p = arg(vm, args, id!(p)).as_number().unwrap_or(0.5);
        (c.borrow_mut().rand() < p).into()
    });
    vm.add_method(module, id_lut!(note), script_args!(note = NIL), move |vm, args| {
        match num_of(vm, arg(vm, args, id!(note))) {
            Some(n) => n.into(),
            None => raise(vm, "note() takes a name like @c4 or a MIDI number".into()),
        }
    });
    vm.add_method(module, id_lut!(chord), script_args!(root = NIL, kind = NIL, inversion = 0.0), move |vm, args| {
        let Some(root) = num_of(vm, arg(vm, args, id!(root))) else { return raise(vm, "chord() takes a root like @a2".into()) };
        let kind = text_of(vm, arg(vm, args, id!(kind))).unwrap_or_else(|| "major".into());
        let Some(steps) = theory::chord_kind(&kind) else { return raise(vm, format!("no chord `{kind}` (chords: {})", theory::CHORD_KINDS.join(", "))) };
        let inversion = arg(vm, args, id!(inversion)).as_number().unwrap_or(0.0) as i32;
        let notes = theory::chord(root as f32, steps, inversion);
        new_array(vm, notes.into_iter().map(|n| (n as f64).into()))
    });
    vm.add_method(module, id_lut!(scale), script_args!(root = NIL, kind = NIL, octaves = 1.0), move |vm, args| {
        let Some(root) = num_of(vm, arg(vm, args, id!(root))) else { return raise(vm, "scale() takes a root like @d3".into()) };
        let kind = text_of(vm, arg(vm, args, id!(kind))).unwrap_or_else(|| "major".into());
        let Some(steps) = theory::scale_kind(&kind) else { return raise(vm, format!("no scale `{kind}` (scales: {})", theory::SCALE_KINDS.join(", "))) };
        let octaves = arg(vm, args, id!(octaves)).as_number().unwrap_or(1.0) as i32;
        let notes = theory::scale(root as f32, steps, octaves);
        new_array(vm, notes.into_iter().map(|n| (n as f64).into()))
    });
    vm.add_method(module, id_lut!(euclid), script_args!(hits = 3.0, steps = 8.0, rotate = 0.0), move |vm, args| {
        let hits = arg(vm, args, id!(hits)).as_number().unwrap_or(3.0) as i32;
        let steps = arg(vm, args, id!(steps)).as_number().unwrap_or(8.0) as i32;
        let rotate = arg(vm, args, id!(rotate)).as_number().unwrap_or(0.0) as i32;
        let pattern = theory::euclid(hits, steps, rotate);
        new_array(vm, pattern.into_iter().map(ScriptValue::from))
    });
    // Arithmetic helpers (a note name counts as its number).
    vm.add_method(module, id_lut!(floor), script_args!(x = 0.0), move |vm, args| num_of(vm, arg(vm, args, id!(x))).unwrap_or(f64::NAN).floor().into());
    vm.add_method(module, id_lut!(ceil), script_args!(x = 0.0), move |vm, args| num_of(vm, arg(vm, args, id!(x))).unwrap_or(f64::NAN).ceil().into());
    vm.add_method(module, id_lut!(round), script_args!(x = 0.0), move |vm, args| num_of(vm, arg(vm, args, id!(x))).unwrap_or(f64::NAN).round().into());
    vm.add_method(module, id_lut!(abs), script_args!(x = 0.0), move |vm, args| num_of(vm, arg(vm, args, id!(x))).unwrap_or(f64::NAN).abs().into());
    vm.add_method(module, id_lut!(min), script_args!(a = 0.0, b = 0.0), move |vm, args| {
        let (a, b) = (num_of(vm, arg(vm, args, id!(a))).unwrap_or(f64::NAN), num_of(vm, arg(vm, args, id!(b))).unwrap_or(f64::NAN));
        a.min(b).into()
    });
    vm.add_method(module, id_lut!(max), script_args!(a = 0.0, b = 0.0), move |vm, args| {
        let (a, b) = (num_of(vm, arg(vm, args, id!(a))).unwrap_or(f64::NAN), num_of(vm, arg(vm, args, id!(b))).unwrap_or(f64::NAN));
        a.max(b).into()
    });
    vm.add_method(module, id_lut!(clamp), script_args!(x = 0.0, lo = 0.0, hi = 1.0), move |vm, args| {
        let x = num_of(vm, arg(vm, args, id!(x))).unwrap_or(f64::NAN);
        let (lo, hi) = (arg(vm, args, id!(lo)).as_number().unwrap_or(0.0), arg(vm, args, id!(hi)).as_number().unwrap_or(1.0));
        x.max(lo).min(hi).into()
    });
    vm.add_method(module, id_lut!(steps), script_args!(pattern = NIL), move |vm, args| {
        let text = text_of(vm, arg(vm, args, id!(pattern))).unwrap_or_default();
        let steps = theory::pattern(&text);
        new_array(vm, steps.into_iter().map(|v| v.map_or(NIL, |v| (v as f64).into())))
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(param), script_args!(name = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(name))).unwrap_or_default();
        let ctx = c.borrow();
        match ctx.params.iter().find(|(n, _)| *n == name) {
            Some((_, value)) => (*value as f64).into(),
            None => {
                let known: Vec<_> = ctx.params.iter().map(|(n, _)| n.as_str()).collect();
                let message = format!("no param `{name}` (declare it in Clock{{params: {{…}}}}; params: {})", known.join(", "));
                drop(ctx);
                raise(vm, message)
            }
        }
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(cue), script_args!(name = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(name))).unwrap_or_default();
        let mut ctx = c.borrow_mut();
        let beat = ctx.cursor;
        ctx.cues.push((name, beat));
        NIL
    });
    // fx(@reverb, {room: 0.8 mix: 0.5}, fn() { … }): what the body plays
    // goes through its own reverb (nested fx() chain inner to outer), then
    // where the loop goes. The settings apply from this beat.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(fx), script_args!(kind = NIL, opts = NIL, body = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(kind))).unwrap_or_default();
        let (mut opts, mut body) = (arg(vm, args, id!(opts)), arg(vm, args, id!(body)));
        let is_fn = |vm: &ScriptVm, v: ScriptValue| v.as_object().is_some_and(|o| vm.bx.heap_ref().is_fn(o));
        if body.is_nil() && is_fn(vm, opts) {
            (body, opts) = (opts, NIL);
        }
        let Some(kind) = crate::model::fx_atom(&name) else {
            return raise(vm, format!("fx(@{name}): no such effect; fx() takes {}", crate::model::SCOPE_FX));
        };
        if !is_fn(vm, body) {
            return raise(vm, "fx(@kind, {settings}, fn() { … }) plays the function's notes through the effect".into());
        }
        let (route, from) = {
            let mut ctx = c.borrow_mut();
            ctx.scope_stack.push(kind);
            let Some(k) = ctx.scopes.iter().position(|chain| *chain == ctx.scope_stack) else {
                ctx.scope_stack.pop();
                drop(ctx);
                return raise(vm, format!("fx(@{name}) must be written in the loop's own code (not in a shared function), with its effect as an atom"));
            };
            let beat = ctx.cursor;
            if let Some(object) = opts.as_object() {
                for (i, (param, _, lo, hi)) in kind.params().iter().enumerate() {
                    let value = vm.bx.heap_ref().value(object, lut_id(param).into(), NoTrap).as_number();
                    if let Some(value) = value {
                        ctx.fx_sets.push((k + 1, i as u8, (value as f32).clamp(*lo, *hi), beat));
                    }
                }
            }
            (k + 1, ctx.emits.len())
        };
        let value = vm.call(body, &[]);
        let mut ctx = c.borrow_mut();
        let to = ctx.emits.len();
        ctx.routed.push((from, to, route));
        ctx.scope_stack.pop();
        value
    });
    // held(): the keys held on the MIDI input, lowest first (arp them).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(held), script_args!(), move |vm, _args| {
        let notes: Vec<ScriptValue> = c.borrow().held.iter().map(|(n, _)| (*n as f64).into()).collect();
        new_array(vm, notes)
    });
    // osc_send("/filter", [0.5 1], "127.0.0.1:57120"): an OSC message now
    // (numbers, strings; the target defaults to the Clock's osc_out).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(osc_send), script_args!(addr = NIL, args = NIL, to = NIL), move |vm, args| {
        use crate::osc::OscArg;
        let addr = text_of(vm, arg(vm, args, id!(addr))).unwrap_or_default();
        if !addr.starts_with('/') {
            return raise(vm, format!("osc_send(\"/address\", args): an OSC address starts with / (got `{addr}`)"));
        }
        let values = arg(vm, args, id!(args));
        let list: Vec<OscArg> = if values.is_nil() { Vec::new() } else {
            elements(vm, values).into_iter().filter_map(|v| match v.as_number() {
                Some(n) => Some(OscArg::Float(n as f32)),
                None => text_of(vm, v).map(OscArg::Str),
            }).collect()
        };
        let to = text_of(vm, arg(vm, args, id!(to)));
        let mut ctx = c.borrow_mut();
        if ctx.active {
            let beat = ctx.cursor;
            ctx.osc_sends.push((beat, addr, list, to));
        }
        NIL
    });
    // osc("/fader1"): the first number the address last brought (0 before).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(osc), script_args!(addr = NIL), move |vm, args| {
        let addr = text_of(vm, arg(vm, args, id!(addr))).unwrap_or_default();
        let ctx = c.borrow();
        ctx.osc_values.iter().find(|(a, _)| *a == addr).and_then(|(_, v)| v.iter().find_map(|a| a.num())).unwrap_or(0.0).into()
    });
    // cc(74): a MIDI controller's last value, 0..1 (0 before it moves).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(cc), script_args!(number = 0.0), move |vm, args| {
        let n = arg(vm, args, id!(number)).as_number().unwrap_or(0.0);
        c.borrow().ccs.get(n.clamp(0.0, 127.0) as usize).copied().unwrap_or(0.0).into()
    });
    // wait(@name): on to the next time another loop cues @name (now if it
    // cues right here); returns the beats waited.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(wait), script_args!(name = NIL), move |vm, args| {
        let name = text_of(vm, arg(vm, args, id!(name))).unwrap_or_default();
        let mut ctx = c.borrow_mut();
        let now = ctx.start + ctx.cursor;
        let next = ctx.cue_plans.iter().filter(|p| p.name == name).filter_map(|p| p.next_at(now)).min_by(f64::total_cmp);
        match next {
            Some(at) => {
                let waited = (at - now).max(0.0);
                ctx.cursor += waited;
                waited.into()
            }
            None => {
                let known: Vec<String> = ctx.cue_plans.iter().map(|p| format!("@{}", p.name)).collect();
                drop(ctx);
                let also = if known.is_empty() { String::new() } else { format!(" (cued: {})", known.join(" ")) };
                raise(vm, format!("nothing cues @{name}: put cue(@{name}) in the loop to follow{also}"))
            }
        }
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(beat), script_args!(), move |_vm, _args| c.borrow().cursor.into());
    let c = ctx.clone();
    vm.add_method(module, id_lut!(cycle), script_args!(), move |_vm, _args| (c.borrow().cycle as f64).into());
    // bar(): the bar playing at the loop's beat, from the piece's start (0).
    let c = ctx.clone();
    vm.add_method(module, id_lut!(bar), script_args!(), move |_vm, _args| {
        let ctx = c.borrow();
        ((ctx.start + ctx.cursor) / ctx.beats_per_bar.max(1e-9) + 1e-9).floor().into()
    });
    // section(): the section playing (its name), or nil without sections.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(section), script_args!(), move |vm, _args| {
        let name = c.borrow().section.as_ref().map(|(name, _)| name.clone());
        match name {
            Some(name) => vm.bx.heap_mut().new_string_from_str(&name).into(),
            None => NIL,
        }
    });
    // section_bar(): the bar within the section playing (0 at its start).
    let c = ctx;
    vm.add_method(module, id_lut!(section_bar), script_args!(), move |_vm, _args| {
        let ctx = c.borrow();
        let from = ctx.section.as_ref().map_or(0.0, |(_, at)| *at);
        ((ctx.start + ctx.cursor - from) / ctx.beats_per_bar.max(1e-9) + 1e-9).floor().max(0.0).into()
    });
}

fn gm_slot(note: f64) -> Option<u8> {
    Some(match note.round() as i32 {
        35 | 36 => 0,
        38 | 40 => 1,
        39 => 2,
        42 | 44 => 3,
        46 => 4,
        37 => 5,
        41 | 43 => 6,
        45 | 47 => 7,
        48 | 50 => 8,
        49 | 57 => 9,
        51 | 59 => 10,
        _ => return None,
    })
}

/// The list helpers, degrees, phrases, shared state and `log`.
fn install_toolkit(vm: &mut ScriptVm, module: ScriptObject, ctx: Shared) {
    let nums = |vm: &ScriptVm, v: ScriptValue| -> Vec<ScriptValue> { elements(vm, v) };
    // Lists: reversed, rotated, mirrored (there and back), reflected (no
    // repeated ends), each element repeated, the list repeated, the first
    // or last n, all but the first n, sorted, n seeded picks.
    vm.add_method(module, id_lut!(rev), script_args!(items = NIL), move |vm, args| {
        let mut items = nums(vm, arg(vm, args, id!(items)));
        items.reverse();
        new_array(vm, items)
    });
    vm.add_method(module, id_lut!(rot), script_args!(items = NIL, n = 1.0), move |vm, args| {
        let mut items = nums(vm, arg(vm, args, id!(items)));
        if !items.is_empty() {
            let n = (arg(vm, args, id!(n)).as_number().unwrap_or(1.0) as i64).rem_euclid(items.len() as i64) as usize;
            items.rotate_left(n);
        }
        new_array(vm, items)
    });
    vm.add_method(module, id_lut!(mirror), script_args!(items = NIL), move |vm, args| {
        let items = nums(vm, arg(vm, args, id!(items)));
        let back: Vec<ScriptValue> = items.iter().rev().copied().collect();
        new_array(vm, items.into_iter().chain(back))
    });
    vm.add_method(module, id_lut!(reflect), script_args!(items = NIL), move |vm, args| {
        let items = nums(vm, arg(vm, args, id!(items)));
        let back: Vec<ScriptValue> = items.iter().rev().skip(1).take(items.len().saturating_sub(2)).copied().collect();
        new_array(vm, items.into_iter().chain(back))
    });
    vm.add_method(module, id_lut!(stretch), script_args!(items = NIL, n = 2.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(2.0).clamp(1.0, 64.0) as usize;
        let items = nums(vm, arg(vm, args, id!(items)));
        new_array(vm, items.into_iter().flat_map(|v| std::iter::repeat_n(v, n)))
    });
    vm.add_method(module, id_lut!(rep), script_args!(items = NIL, n = 2.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(2.0).clamp(1.0, 256.0) as usize;
        let items = nums(vm, arg(vm, args, id!(items)));
        let len = items.len();
        new_array(vm, items.into_iter().cycle().take(len * n))
    });
    vm.add_method(module, id_lut!(first), script_args!(items = NIL, n = 1.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(1.0).max(0.0) as usize;
        let items = nums(vm, arg(vm, args, id!(items)));
        new_array(vm, items.into_iter().take(n))
    });
    vm.add_method(module, id_lut!(last), script_args!(items = NIL, n = 1.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(1.0).max(0.0) as usize;
        let items = nums(vm, arg(vm, args, id!(items)));
        let skip = items.len().saturating_sub(n);
        new_array(vm, items.into_iter().skip(skip))
    });
    vm.add_method(module, id_lut!(drop), script_args!(items = NIL, n = 1.0), move |vm, args| {
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(1.0).max(0.0) as usize;
        let items = nums(vm, arg(vm, args, id!(items)));
        new_array(vm, items.into_iter().skip(n))
    });
    vm.add_method(module, id_lut!(sort), script_args!(items = NIL), move |vm, args| {
        let mut items: Vec<f64> = nums(vm, arg(vm, args, id!(items))).into_iter().filter_map(|v| num_of(vm, v)).collect();
        items.sort_by(f64::total_cmp);
        new_array(vm, items.into_iter().map(ScriptValue::from))
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(pick), script_args!(items = NIL, n = 1.0), move |vm, args| {
        let items = nums(vm, arg(vm, args, id!(items)));
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(1.0).max(0.0) as usize;
        if items.is_empty() {
            return new_array(vm, std::iter::empty());
        }
        let picks: Vec<ScriptValue> = (0..n).map(|_| items[((c.borrow_mut().rand() * items.len() as f64) as usize).min(items.len() - 1)]).collect();
        new_array(vm, picks)
    });
    // line(from, to, n): n evenly spaced values; bits(n, width): 1/0 of n.
    vm.add_method(module, id_lut!(line), script_args!(from = 0.0, to = 1.0, n = 4.0), move |vm, args| {
        let (from, to) = (num_of(vm, arg(vm, args, id!(from))).unwrap_or(0.0), num_of(vm, arg(vm, args, id!(to))).unwrap_or(1.0));
        let n = arg(vm, args, id!(n)).as_number().unwrap_or(4.0).clamp(1.0, 1024.0) as usize;
        new_array(vm, (0..n).map(|i| (from + (to - from) * i as f64 / n.max(2).saturating_sub(1).max(1) as f64).into()))
    });
    vm.add_method(module, id_lut!(bits), script_args!(value = 0.0, width = 8.0), move |vm, args| {
        let value = arg(vm, args, id!(value)).as_number().unwrap_or(0.0) as u64;
        let width = arg(vm, args, id!(width)).as_number().unwrap_or(8.0).clamp(1.0, 64.0) as u32;
        new_array(vm, (0..width).rev().map(|b| ScriptValue::from((value >> b) & 1 == 1)))
    });
    // deg(degree, root, mode): a scale degree (1-based, any octave);
    // snap(note, root, mode): the nearest note of the scale.
    vm.add_method(module, id_lut!(deg), script_args!(degree = 1.0, root = NIL, mode = NIL), move |vm, args| {
        let degree = arg(vm, args, id!(degree)).as_number().unwrap_or(1.0) as i64;
        let root = num_of(vm, arg(vm, args, id!(root))).unwrap_or(60.0);
        let mode = text_of(vm, arg(vm, args, id!(mode))).unwrap_or_else(|| "major".into());
        let Some(steps) = theory::scale_kind(&mode) else { return raise(vm, format!("no scale `{mode}` (scales: {})", theory::SCALE_KINDS.join(", "))) };
        let len = steps.len() as i64;
        let d = degree - 1;
        let (octave, index) = (d.div_euclid(len), d.rem_euclid(len));
        (root + (octave * 12 + steps[index as usize] as i64) as f64).into()
    });
    vm.add_method(module, id_lut!(snap), script_args!(note = NIL, root = NIL, mode = NIL), move |vm, args| {
        let note = num_of(vm, arg(vm, args, id!(note))).unwrap_or(60.0);
        let root = num_of(vm, arg(vm, args, id!(root))).unwrap_or(60.0);
        let mode = text_of(vm, arg(vm, args, id!(mode))).unwrap_or_else(|| "major".into());
        let Some(steps) = theory::scale_kind(&mode) else { return raise(vm, format!("no scale `{mode}`")) };
        let rel = note - root;
        let octave = (rel / 12.0).floor();
        let within = rel - octave * 12.0;
        let best = steps.iter().map(|s| *s as f64).chain([12.0]).min_by(|a, b| (a - within).abs().total_cmp(&(b - within).abs())).unwrap_or(0.0);
        (root + octave * 12.0 + best).into()
    });
    // phrase(notes, durs, {amp dur...}): a line from here, each note held
    // most of its length; arp(chord, @up, rate, octaves, steps); strum(chord,
    // spread): all without moving time, each returning its length.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(phrase), script_args!(notes = NIL, durs = NIL, opts = NIL), move |vm, args| {
        let notes = nums(vm, arg(vm, args, id!(notes)));
        let durs: Vec<f64> = nums(vm, arg(vm, args, id!(durs))).into_iter().filter_map(|v| v.as_number()).collect();
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let mut line = Vec::new();
        for (i, v) in notes.into_iter().enumerate() {
            let len = durs.get(i % durs.len().max(1)).copied().unwrap_or(1.0).max(1.0 / 64.0);
            let pitches = if v.is_nil() || text_of(vm, v).is_some_and(|t| t == "_" || t == "r" || t == "rest") { Vec::new() } else {
                match notes_of(vm, v) {
                    Ok(p) => p,
                    Err(message) => return raise(vm, message),
                }
            };
            line.push((pitches, len));
        }
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (mut t, amp) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp));
        let start = t;
        for (pitches, len) in line {
            let dur = opts.dur.unwrap_or((len * 0.9) as f32);
            for note in pitches {
                ctx.emits.push(Emit { beat: t, what: EmitKind::Note(note), vel: amp, pan: opts.pan, dur, opts: opts.note });
            }
            t += len;
        }
        (t - start).into()
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(arp), script_args!(chord = NIL, mode = NIL, rate = 0.25, octaves = 1.0, steps = 0.0, opts = NIL), move |vm, args| {
        let notes = match notes_of(vm, arg(vm, args, id!(chord))) {
            Ok(n) => n,
            Err(message) => return raise(vm, message),
        };
        let mode = text_of(vm, arg(vm, args, id!(mode))).unwrap_or_else(|| "up".into());
        let rate = arg(vm, args, id!(rate)).as_number().unwrap_or(0.25).max(1.0 / 64.0);
        let octaves = arg(vm, args, id!(octaves)).as_number().unwrap_or(1.0).clamp(1.0, 4.0) as usize;
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let mut up: Vec<f32> = (0..octaves).flat_map(|o| notes.iter().map(move |n| n + 12.0 * o as f32)).collect();
        up.sort_by(f32::total_cmp);
        let mut order: Vec<f32> = match mode.as_str() {
            "down" => up.iter().rev().copied().collect(),
            "updown" => up.iter().copied().chain(up.iter().rev().skip(1).take(up.len().saturating_sub(2)).copied()).collect(),
            "random" => {
                let mut o = up.clone();
                for i in (1..o.len()).rev() {
                    let j = ((c.borrow_mut().rand() * (i + 1) as f64) as usize).min(i);
                    o.swap(i, j);
                }
                o
            }
            _ => up.clone(),
        };
        if order.is_empty() {
            return 0.0.into();
        }
        let steps = match arg(vm, args, id!(steps)).as_number().unwrap_or(0.0) as usize { 0 => order.len(), n => n };
        order = order.iter().copied().cycle().take(steps).collect();
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (start, amp) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp));
        let dur = opts.dur.unwrap_or((rate * 0.9) as f32);
        for (i, note) in order.iter().enumerate() {
            ctx.emits.push(Emit { beat: start + i as f64 * rate, what: EmitKind::Note(*note), vel: amp, pan: opts.pan, dur, opts: opts.note });
        }
        (order.len() as f64 * rate).into()
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(strum), script_args!(chord = NIL, spread = 0.03, opts = NIL), move |vm, args| {
        let notes = match notes_of(vm, arg(vm, args, id!(chord))) {
            Ok(n) => n,
            Err(message) => return raise(vm, message),
        };
        let spread = arg(vm, args, id!(spread)).as_number().unwrap_or(0.03);
        let opts = opts_of(vm, arg(vm, args, id!(opts)));
        let mut ctx = c.borrow_mut();
        if !ctx.active {
            return NIL;
        }
        let (start, amp, dur) = (ctx.cursor, opts.amp.unwrap_or(ctx.amp), opts.dur.unwrap_or(ctx.dur));
        let n = notes.len();
        for (i, note) in notes.into_iter().enumerate() {
            let at = if spread >= 0.0 { i as f64 * spread } else { (n - 1 - i) as f64 * -spread };
            ctx.emits.push(Emit { beat: start + at, what: EmitKind::Note(note), vel: amp, pan: opts.pan, dur, opts: opts.note });
        }
        (spread.abs() * n.saturating_sub(1) as f64).into()
    });
    // put(@key, value): shared from this beat on; get(@key): the latest
    // value put at or before this beat (nil if none): a chord loop tells
    // the bass and the melody what the chord is.
    let c = ctx.clone();
    vm.add_method(module, id_lut!(put), script_args!(key = NIL, value = NIL), move |vm, args| {
        let key = text_of(vm, arg(vm, args, id!(key))).unwrap_or_default();
        let value = arg(vm, args, id!(value));
        let stored = if let Some(n) = value.as_number() {
            Stored::Num(n)
        } else if value.as_array().is_some() {
            Stored::Nums(elements(vm, value).into_iter().filter_map(|v| num_of(vm, v)).collect())
        } else {
            Stored::Text(text_of(vm, value).unwrap_or_default())
        };
        let mut ctx = c.borrow_mut();
        if ctx.active {
            let beat = ctx.start + ctx.cursor;
            ctx.shared.push((key, beat, stored));
        }
        NIL
    });
    let c = ctx.clone();
    vm.add_method(module, id_lut!(get), script_args!(key = NIL), move |vm, args| {
        let key = text_of(vm, arg(vm, args, id!(key))).unwrap_or_default();
        let found = {
            let ctx = c.borrow();
            let beat = ctx.start + ctx.cursor;
            ctx.shared.iter().filter(|(k, at, _)| *k == key && *at <= beat + 1e-9).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(_, _, v)| v.clone())
        };
        match found {
            Some(Stored::Num(n)) => n.into(),
            Some(Stored::Nums(list)) => new_array(vm, list.into_iter().map(ScriptValue::from)),
            Some(Stored::Text(t)) => vm.bx.heap_mut().new_string_from_str(&t).into(),
            None => NIL,
        }
    });
    // log(x): shown with the loop (score_status, the lens).
    let c = ctx;
    vm.add_method(module, id_lut!(log), script_args!(value = NIL), move |vm, args| {
        let value = arg(vm, args, id!(value));
        let text = if let Some(n) = value.as_number() {
            crate::source::fmt_num(n, 3)
        } else if value.as_array().is_some() {
            let items: Vec<String> = elements(vm, value).into_iter().map(|v| v.as_number().map(|n| crate::source::fmt_num(n, 3)).or_else(|| text_of(vm, v)).unwrap_or_else(|| "?".into())).collect();
            format!("[{}]", items.join(" "))
        } else {
            text_of(vm, value).unwrap_or_else(|| "nil".into())
        };
        let mut ctx = c.borrow_mut();
        if ctx.logs.len() < 64 {
            let beat = ctx.cursor;
            ctx.logs.push(format!("{}: {text}", crate::source::fmt_num(beat, 2)));
        }
        NIL
    });
}
