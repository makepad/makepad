//! The app's side of a program: open a session, steer it, take its outputs.
use crate::cpu::{CancelSlot, ElemTy};
use crate::value::Value;
use makepad_script::ScriptInterrupt;
use makepad_script_compute::Backend;
use makepad_script_compute_sched::sched::Executor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// A program's document: its name (the file its errors name) and its Splash source.
#[derive(Clone)]
pub struct Program {
    pub name: String,
    pub source: Arc<str>,
}

impl Program {
    pub fn new(name: &str, source: &str) -> Program {
        Program { name: name.to_string(), source: source.into() }
    }
}

/// A session input: words the app shares by reference (`s.input(name)` binds them read-only,
/// with no copy).
#[derive(Clone)]
pub struct Input {
    pub name: String,
    pub ty: ElemTy,
    pub data: Arc<[u32]>,
}

impl Input {
    pub fn f32(name: &str, values: &[f32]) -> Input {
        Input { name: name.to_string(), ty: ElemTy::F32, data: values.iter().map(|x| x.to_bits()).collect() }
    }

    pub fn words(name: &str, ty: ElemTy, words: Arc<[u32]>) -> Input {
        Input { name: name.to_string(), ty, data: words }
    }
}

/// What one session may spend, all counted (no clock decides anything).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// VM instructions over the whole run (the document's load included).
    pub fuel: usize,
    /// The live VM heap.
    pub heap_bytes: usize,
    pub stack_values: usize,
    pub call_frames: usize,
    /// Kernel work, in the counted ops of `makepad_script_compute::work`.
    pub work: u64,
    /// Session buffers alive at once (inputs are the app's and not counted).
    pub buffer_bytes: u64,
    /// The most words one `s.read` returns.
    pub read_words: usize,
    /// Distinct kernels a program compiles.
    pub kernels: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            fuel: 1 << 36,
            heap_bytes: 64 << 20,
            stack_values: 1 << 16,
            call_frames: 256,
            work: 1 << 46,
            buffer_bytes: 1 << 30,
            read_words: 1 << 22,
            kernels: 256,
        }
    }
}

/// Who waits on a session: the unified workpool's classes (`makepad_cpu_budget`, one OS
/// priority table). The driver thread runs at its class; a node runs the session's kernels on
/// that class's workers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// A person steers it and watches the result (a zoom, a drag).
    Interactive,
    /// On screen, nobody waiting on the next step.
    Visible,
    /// Fan-out, variants, precompute: only idle cores.
    Background,
}

impl Class {
    /// The class as `makepad_cpu_budget` numbers it.
    pub fn budget(self) -> u8 {
        match self {
            Class::Interactive => makepad_cpu_budget::INTERACTIVE,
            Class::Visible => makepad_cpu_budget::VISIBLE,
            Class::Background => makepad_cpu_budget::BACKGROUND,
        }
    }

    pub fn from_budget(class: u8) -> Option<Class> {
        [Class::Interactive, Class::Visible, Class::Background].into_iter().find(|c| c.budget() == class)
    }

    /// Runs the calling thread at this class (the budget's one OS table). False when the OS
    /// refused: raising a class needs a privilege, so the thread keeps the one it has.
    pub fn apply(self) -> bool {
        makepad_cpu_budget::set_thread_class(self.budget())
    }
}

pub struct SessionConfig {
    /// `run`'s second argument.
    pub params: Value,
    pub inputs: Vec<Input>,
    pub limits: Limits,
    /// Where kernels run: the app's workpool. None: the process-wide kernel pool.
    pub executor: Option<Arc<dyn Executor>>,
    /// Workers one kernel call may use (the results never depend on it).
    pub threads: usize,
    /// Native code, or the AIR interpreter (the same bits).
    pub backend: Backend,
    /// The kernels' `seed`.
    pub seed: u32,
    pub class: Class,
}

impl Default for SessionConfig {
    fn default() -> SessionConfig {
        SessionConfig {
            params: Value::Nil,
            inputs: Vec::new(),
            limits: Limits::default(),
            executor: None,
            threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
            backend: if cfg!(any(target_arch = "aarch64", target_arch = "x86_64")) { Backend::Native } else { Backend::Interp },
            seed: 0,
            class: Class::Visible,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProgramError {
    /// The document did not load, or its last value has no `run` fn.
    Load(String),
    /// The driver failed: a script error, a bad argument to a session op, a kernel that does not
    /// compile. The message names the document place.
    Program(String),
    /// A limit ran out: "instructions", "heap", "stack", "calls", "work" or "kernels".
    OverBudget(&'static str),
    Cancelled,
    /// A remote session's link to its node broke, or the node sent what does not decode.
    Lost(String),
}

impl std::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProgramError::Load(m) => write!(f, "the program did not load: {m}"),
            ProgramError::Program(m) => write!(f, "{m}"),
            ProgramError::OverBudget(what) => write!(f, "over its {what} budget"),
            ProgramError::Cancelled => write!(f, "cancelled"),
            ProgramError::Lost(why) => write!(f, "the node was lost: {why}"),
        }
    }
}

/// A buffer the program published (`s.publish`) or checkpointed (`s.state`).
#[derive(Clone, Debug)]
pub struct Output {
    pub name: String,
    /// `s.state`: full precision, for resume.
    pub state: bool,
    /// Counts this name's outputs from 1.
    pub version: u64,
    /// The newest message rev the program had taken when it published.
    pub rev: u64,
    pub ty: ElemTy,
    pub data: OutputData,
}

#[derive(Clone, Debug)]
pub enum OutputData {
    /// The buffer's words as they are.
    Words(Arc<[u32]>),
    /// f32 components as f16, rounded to nearest even.
    F16(Arc<[u16]>),
    /// f32 components as levels 0..=255 of `[lo, hi]`.
    U8 { lo: f32, hi: f32, levels: Arc<[u8]> },
}

impl OutputData {
    /// Words as f32 (a `Words` output of f32 components).
    pub fn f32s(&self) -> Option<Vec<f32>> {
        match self {
            OutputData::Words(w) => Some(w.iter().map(|x| f32::from_bits(*x)).collect()),
            _ => None,
        }
    }
}

/// A buffer's precision when it is published (`precision: "f32" | "f16" | "u8"`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Precision {
    Full,
    F16,
    U8 { lo: f32, hi: f32 },
}

/// What a session produced since the last look: the newest output per name, the newest
/// `value` per name, the newest progress, and the end once.
#[derive(Default, Debug)]
pub struct Updates {
    pub outputs: Vec<Output>,
    pub values: Vec<(String, Value)>,
    pub progress: Option<(f64, String)>,
    pub end: Option<Result<Value, ProgramError>>,
}

impl Updates {
    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty() && self.values.is_empty() && self.progress.is_none() && self.end.is_none()
    }

    pub fn output(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name == name)
    }

    pub fn value(&self, name: &str) -> Option<&Value> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// Takes in the newer `u`: the newest output per (name, state), the newest value per name,
    /// its progress and end where it has them.
    pub fn merge(&mut self, u: Updates) {
        for o in u.outputs {
            match self.outputs.iter_mut().find(|m| m.name == o.name && m.state == o.state) {
                Some(slot) => *slot = o,
                None => self.outputs.push(o),
            }
        }
        for (name, value) in u.values {
            match self.values.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = value,
                None => self.values.push((name, value)),
            }
        }
        if u.progress.is_some() {
            self.progress = u.progress;
        }
        if u.end.is_some() {
            self.end = u.end;
        }
    }
}

#[derive(Default)]
pub(crate) struct Inbox {
    pub rev: u64,
    pub msgs: Vec<(String, Value)>,
}

#[derive(Default)]
pub(crate) struct Outbox {
    pub updates: Updates,
    pub ended: Option<Result<Value, ProgramError>>,
    /// [`Session::wake`]: the next `wait` returns, empty or not.
    pub woken: bool,
}

impl Outbox {
    pub(crate) fn output(&mut self, output: Output) {
        let outputs = &mut self.updates.outputs;
        match outputs.iter_mut().find(|o| o.name == output.name && o.state == output.state) {
            Some(slot) => *slot = output,
            None => outputs.push(output),
        }
    }

    pub(crate) fn value(&mut self, name: String, value: Value) {
        let values = &mut self.updates.values;
        match values.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = value,
            None => values.push((name, value)),
        }
    }
}

/// What the app and the driver thread share.
#[derive(Default)]
pub(crate) struct Shared {
    pub inbox: Mutex<Inbox>,
    pub inbox_cv: Condvar,
    pub outbox: Mutex<Outbox>,
    pub outbox_cv: Condvar,
    pub cancelled: AtomicBool,
    /// An interrupting message came: device work stops and the segment is dropped until the
    /// program takes its inbox.
    pub interrupted: AtomicBool,
    pub vm_interrupt: Mutex<Option<ScriptInterrupt>>,
    pub running: CancelSlot,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::Relaxed)
    }
}

/// A running program. Dropping it cancels the program and waits for its thread.
pub struct Session {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    /// Starts `program` on its own driver thread.
    pub fn open(program: &Program, config: SessionConfig) -> Session {
        let shared = Arc::new(Shared::default());
        let program = program.clone();
        let thread_shared = shared.clone();
        // The driver's own recursion is bounded by the call-frame limit; the stack covers kernel
        // compilation.
        let thread = std::thread::Builder::new()
            .name("compute-program".into())
            .stack_size(16 << 20)
            .spawn(move || {
                config.class.apply();
                let result = crate::driver::drive(&program, config, thread_shared.clone());
                let mut out = lock(&thread_shared.outbox);
                out.updates.end = Some(result.clone());
                out.ended = Some(result);
                drop(out);
                thread_shared.outbox_cv.notify_all();
            })
            .expect("a compute program's driver thread");
        Session { shared, thread: Some(thread) }
    }

    /// Delivers a message: the program's inbox keeps the newest value per key. Returns its rev.
    pub fn send(&self, key: &str, value: Value) -> u64 {
        self.post(key, value, false)
    }

    /// [`Self::send`], and the device work in flight stops: the program's next `read` returns
    /// nil until it takes its inbox (a regrow, a new shape).
    pub fn interrupt(&self, key: &str, value: Value) -> u64 {
        self.post(key, value, true)
    }

    fn post(&self, key: &str, value: Value, interrupt: bool) -> u64 {
        let rev = {
            let mut inbox = lock(&self.shared.inbox);
            inbox.rev += 1;
            match inbox.msgs.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => inbox.msgs.push((key.to_string(), value)),
            }
            if interrupt {
                self.shared.interrupted.store(true, Ordering::Relaxed);
            }
            inbox.rev
        };
        if interrupt {
            self.shared.running.cancel();
        }
        self.shared.inbox_cv.notify_all();
        rev
    }

    /// Stops the program: its next instruction batch, kernel or wait ends it with
    /// [`ProgramError::Cancelled`].
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
        if let Some(interrupt) = lock(&self.shared.vm_interrupt).as_ref() {
            interrupt.interrupt();
        }
        self.shared.running.cancel();
        // Wakes a `wait_inbox` (it checks the flag under the inbox lock).
        drop(lock(&self.shared.inbox));
        self.shared.inbox_cv.notify_all();
    }

    /// What came since the last look (never blocks).
    pub fn poll(&self) -> Updates {
        std::mem::take(&mut lock(&self.shared.outbox).updates)
    }

    /// [`Self::poll`], waiting until there is something.
    pub fn wait(&self) -> Updates {
        let mut out = lock(&self.shared.outbox);
        while out.updates.is_empty() && out.ended.is_none() && !out.woken {
            out = self.shared.outbox_cv.wait(out).unwrap_or_else(|e| e.into_inner());
        }
        out.woken = false;
        std::mem::take(&mut out.updates)
    }

    /// Makes a `wait` (now or the next) return, with what there is: for a sender that waits on
    /// both the program and its link.
    pub fn wake(&self) {
        lock(&self.shared.outbox).woken = true;
        self.shared.outbox_cv.notify_all();
    }

    /// Waits for the program's end: what `run` returned, or why it stopped.
    pub fn wait_end(&self) -> Result<Value, ProgramError> {
        let mut out = lock(&self.shared.outbox);
        loop {
            if let Some(end) = &out.ended {
                return end.clone();
            }
            out = self.shared.outbox_cv.wait(out).unwrap_or_else(|e| e.into_inner());
        }
    }

    pub fn ended(&self) -> bool {
        lock(&self.shared.outbox).ended.is_some()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.ended() {
            self.cancel();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
