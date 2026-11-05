//! The driver: the program's isolate (its own VM and heap) on the session thread, and
//! `mod.compute`, the session the program's `run(s, p)` gets as `s`.
use crate::cpu::*;
use crate::gpu::*;
use crate::session::*;
use crate::value::*;
use makepad_script::shader_ir::IrScalar;
use makepad_script::shader_ir_compute::{compile_compute_kernel_result, CompiledKernel, ComputeLayout, ComputeSlotKind, ComputeTarget, KernelCode};
use makepad_script::*;
use makepad_script_compute::boundary::KernelCompilation;
use makepad_script_compute::element_route::{ElementRoute, ELEMENT_ARGS};
use makepad_script_compute::kernel::Access;
use makepad_script_compute::kernel::MathMode;
use makepad_script_compute::lower::kernel::{K_COUNT, K_PARAMS, K_SEED};
use makepad_script_compute::object::{prepare_object, CountPolicy};
use makepad_script_compute_sched::kernel::KernelError;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

/// Recorded ops past which a segment runs without waiting for a boundary.
const SEGMENT_OPS: usize = 4096;
/// The most rounds one `repeat` or `sweep` runs.
const MAX_ROUNDS: u32 = 1 << 20;
/// The most values one `s.write` takes.
const MAX_WRITE: usize = 1 << 20;

struct KernelEntry {
    obj: ScriptObject,
    /// Keeps the object (and so its identity in this table) alive.
    _root: ScriptObjectRef,
    label: String,
    /// Its GPU kernel once installed on the device, and per storage buffer of its layout the
    /// kernel's buffer it binds (None: the args).
    gpu: Option<(usize, Vec<Option<usize>>)>,
}

/// A shader kernel (`mod.shader` compute object, `s.dispatch`): GPU only, compiled once.
struct ShaderEntry {
    obj: ScriptObject,
    _root: ScriptObjectRef,
    label: String,
    gpu: usize,
    layout: ComputeLayout,
}

/// Local arrays an element kernel's GPU form may hold, in words a thread.
const ELEMENT_FRAME_WORDS: u32 = 256;

/// The session's side of its GPU device.
struct GpuSide {
    link: GpuLink,
    session: u64,
    stop: Arc<AtomicU64>,
    /// Ops recorded for the device, not sent yet.
    batch: Vec<GpuOp>,
    kernels: usize,
    /// Bytes its buffers hold, against the device's budget.
    bytes: u64,
    /// It sent the device work.
    used: bool,
}

impl GpuSide {
    /// Sends the batch; with `reply`, waits for the device's answer: the batch's reads.
    fn send(&mut self, reply: bool) -> Result<Vec<Vec<u8>>, GpuError> {
        if !reply {
            let ops = std::mem::take(&mut self.batch);
            self.used |= !ops.is_empty();
            let epoch = self.stop.load(Ordering::Relaxed);
            if ops.is_empty() || self.link.send(GpuMsg::Ops { session: self.session, epoch, ops, reply: None }) {
                return Ok(Vec::new());
            }
            return Err(GpuError::DeviceLost);
        }
        self.send_later()?.recv().unwrap_or(Err(GpuError::DeviceLost))
    }

    /// Sends the batch with a reply, not waiting for it: the device's answer comes on the
    /// receiver.
    fn send_later(&mut self) -> Result<mpsc::Receiver<GpuReads>, GpuError> {
        let ops = std::mem::take(&mut self.batch);
        self.used |= !ops.is_empty();
        let epoch = self.stop.load(Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        if !self.link.send(GpuMsg::Ops { session: self.session, epoch, ops, reply: Some(tx) }) {
            return Err(GpuError::DeviceLost);
        }
        Ok(rx)
    }
}

/// A read the program sent without waiting (`s.fetch`), until `s.take` has its words.
enum Fetch {
    /// On the GPU: the device's answer comes on the receiver.
    Gpu(mpsc::Receiver<GpuReads>, ElemTy),
    /// A CPU buffer's words, read at the fetch.
    Ready(Vec<u32>, ElemTy),
    /// An interrupting message dropped it: it takes as nil.
    Dropped,
}

/// Fetches not taken yet, at most.
const MAX_FETCHES: usize = 64;

impl Drop for GpuSide {
    fn drop(&mut self) {
        self.link.send(GpuMsg::Close { session: self.session });
    }
}

/// A recorded op: the CPU device's, the GPU device's, or a copy between them.
enum Step {
    Cpu(Op),
    Gpu(GpuOp),
    /// A CPU buffer's words into a GPU buffer of the same size.
    Upload { src: usize, dst: usize },
    /// A GPU buffer's words into a CPU buffer of the same size.
    Download { src: usize, dst: usize },
}

#[derive(Default, Clone, Copy)]
struct Stat {
    calls: u64,
    nanos: u64,
    work: u64,
    elements: u64,
}

pub(crate) struct Driver {
    shared: Arc<Shared>,
    device: CpuDevice,
    limits: Limits,
    inputs: Vec<(Input, Option<(usize, u32)>)>,
    pending: Vec<Step>,
    kernels: Vec<KernelEntry>,
    shaders: Vec<ShaderEntry>,
    gpu: Option<GpuSide>,
    seed: u32,
    work: u64,
    buffer_bytes: u64,
    /// The newest message rev the program took.
    rev: u64,
    versions: Vec<(String, bool, u64)>,
    fatal: Option<ProgramError>,
    stats: Vec<(String, Stat)>,
    /// Fetches not taken, by their number, and the next number.
    fetches: HashMap<u64, Fetch>,
    next_fetch: u64,
    /// Where results last between sessions, and the program's content (part of every key).
    keep: Option<(Arc<dyn Keep>, u128)>,
    /// Each input's content hash, once a key named the input.
    input_hashes: Vec<Option<u128>>,
}

// SAFETY: the ID names this type alone; only this crate's natives dereference it.
unsafe impl ScriptHostCapability for Driver {
    const ID: LiveId = LiveId::from_str("makepad.compute.session.v1");
}

enum Fail {
    /// A bad argument: a script error at the call.
    Arg(String),
    /// The session ends.
    Fatal(ProgramError),
}

impl From<String> for Fail {
    fn from(m: String) -> Fail {
        Fail::Arg(m)
    }
}

impl From<&str> for Fail {
    fn from(m: &str) -> Fail {
        Fail::Arg(m.to_string())
    }
}

type Res<T> = Result<T, Fail>;

/// The session's driver: the program's run on this (the session's) thread.
pub(crate) fn drive(program: &Program, config: SessionConfig, shared: Arc<Shared>) -> Result<Value, ProgramError> {
    let limits = config.limits;
    let gpu = match &config.gpu {
        Some(link) => {
            *lock(&shared.gpu) = Some(link.clone());
            let stop = shared.gpu_stop.clone();
            let watched = matches!(config.class, Class::Interactive | Class::Visible);
            let session = link.open(&program.name, stop.clone(), watched).ok_or_else(|| ProgramError::Program("the GPU device has ended".into()))?;
            Some(GpuSide { link: link.clone(), session, stop, batch: Vec::new(), kernels: 0, bytes: 0, used: false })
        }
        None => None,
    };
    let driver = Driver {
        shared,
        device: CpuDevice::new(config.executor.clone(), config.threads, config.backend, config.seed),
        limits,
        inputs: config.inputs.iter().cloned().map(|i| (i, None)).collect(),
        pending: Vec::new(),
        kernels: Vec::new(),
        shaders: Vec::new(),
        gpu,
        seed: config.seed,
        work: 0,
        buffer_bytes: 0,
        rev: 0,
        versions: Vec::new(),
        fatal: None,
        stats: Vec::new(),
        fetches: HashMap::new(),
        next_fetch: 0,
        keep: config.keep.clone().map(|k| (k, bytes_hash(program.source.as_bytes()))),
        input_hashes: vec![None; config.inputs.len()],
    };
    let mut host = ScriptVmHost::new(driver, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let mut result = run_program(&mut vm, program, &config);
    // The device's errors since the program's last read come with an answer: one more, where
    // it sent the device work.
    if let (Ok(_), Some(gpu)) = (&result, state(&mut vm).gpu.as_mut().filter(|g| g.used)) {
        if let Err(e) = gpu.send(true) {
            result = Err(gpu_fatal(e));
        }
    }
    // The kernels' roots point into this VM's heap: they go before it does.
    state(&mut vm).kernels.clear();
    state(&mut vm).shaders.clear();
    drop(vm);
    result
}

fn run_program(vm: &mut ScriptVm, program: &Program, config: &SessionConfig) -> Result<Value, ProgramError> {
    let limits = config.limits;
    vm.capture_errors();
    vm.bx.bail_on_uncaught_error = true;
    vm.bx.allow_debug_output = false;
    vm.set_script_domain("compute");
    vm.heap_mut().set_max_heap_bytes(Some(limits.heap_bytes));
    makepad_script_compute::vm_kernel::script_mod_names(vm, &[]);
    let s = define_compute(vm);
    let interrupt = vm.bx.interrupt_handle();
    {
        let shared = state(vm).shared.clone();
        *lock(&shared.vm_interrupt) = Some(interrupt.clone());
        if shared.cancelled() {
            interrupt.interrupt();
        }
    }
    let mut fuel = limits.fuel;
    let t = Instant::now();
    let doc = limited(vm, &limits, &mut fuel, |vm| vm.eval(ScriptMod::new("", "", &program.name, 0, 0, &program.source, &[])));
    note(state(vm), "(loads the program)".into(), &OpDone { nanos: t.elapsed().as_nanos() as u64, ..OpDone::default() });
    let doc = outcome(vm, doc).map_err(|e| match e {
        ProgramError::Program(m) => ProgramError::Load(m),
        e => e,
    })?;
    let run = doc
        .as_object()
        .map(|o| vm.value(o, id!(run)))
        .and_then(|f| f.as_object())
        .filter(|f| vm.is_fn(*f))
        .ok_or_else(|| ProgramError::Load(format!("{}: the document's last value is {{run: fn(s, p) {{..}}}}", program.name)))?;
    let params = to_script(vm, &config.params);
    let result = limited(vm, &limits, &mut fuel, |vm| vm.call(run.into(), &[s.into(), params]));
    let result = outcome(vm, result)?;
    // The value, read before the end's boundary: nothing in the VM holds it once `run`
    // returned, and the boundary collects garbage.
    let value = from_script(vm, result).map_err(ProgramError::Program)?;
    // The end of `run` is a boundary.
    let d = state(vm);
    match boundary(vm, d) {
        Ok(_) => Ok(value),
        Err(Fail::Fatal(e)) => Err(e),
        Err(Fail::Arg(m)) => Err(ProgramError::Program(m)),
    }
}

/// Runs `f` under the session's stack, call and instruction limits, charging its instructions.
fn limited<R>(vm: &mut ScriptVm, limits: &Limits, fuel: &mut u64, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
    let budget = *fuel;
    let r = vm.with_stack_value_limit(limits.stack_values, |vm| vm.with_call_frame_limit(limits.call_frames, |vm| vm.with_instruction_limit(budget, f)));
    *fuel = fuel.saturating_sub(vm.last_limit_consumed());
    r
}

/// The value an eval or call made, or why the run ended.
fn outcome(vm: &mut ScriptVm, v: ScriptValue) -> Result<ScriptValue, ProgramError> {
    let errors = vm.take_errors();
    vm.capture_errors();
    let d = state(vm);
    if let Some(e) = d.fatal.clone() {
        return Err(e);
    }
    if d.shared.cancelled() {
        return Err(ProgramError::Cancelled);
    }
    if errors.is_empty() && !v.is_err() {
        return Ok(v);
    }
    let text = errors.join("\n");
    for (needle, what) in [
        ("script instruction limit exceeded", "instructions"),
        ("script heap allocation limit exceeded", "heap"),
        ("script operand stack limit exceeded", "stack"),
        ("script call frame limit exceeded", "calls"),
    ] {
        if text.contains(needle) {
            return Err(ProgramError::OverBudget(what));
        }
    }
    Err(ProgramError::Program(if text.is_empty() { "the program failed".into() } else { text }))
}

fn state<'a>(vm: &mut ScriptVm) -> &'a mut Driver {
    // SAFETY: the driver lives in the host the VM borrows for the whole run, and natives run one
    // at a time on this thread; none re-enters the VM while it holds the reference.
    unsafe { &mut *state_ptr(vm) }
}

fn state_ptr(vm: &mut ScriptVm) -> *mut Driver {
    vm.host.get_mut::<Driver>().expect("a compute session's VM") as *mut Driver
}

/// A session op: its result, or the script error a bad argument raises, or the session's end.
fn native(vm: &mut ScriptVm, f: impl FnOnce(&mut ScriptVm, &mut Driver) -> Res<ScriptValue>) -> ScriptValue {
    // SAFETY: see `state`.
    let d = unsafe { &mut *state_ptr(vm) };
    if let Some(e) = &d.fatal {
        let m = e.to_string();
        return script_err_limit!(vm.bx.threads.cur_ref().trap, "{}", m);
    }
    match f(vm, d) {
        Ok(v) => v,
        Err(Fail::Arg(m)) => script_err_invalid_args!(vm.bx.threads.cur_ref().trap, "{}", m),
        Err(Fail::Fatal(e)) => {
            let m = e.to_string();
            d.fatal = Some(e);
            // Uncatchable: a `try` around the op does not keep the program going.
            vm.bx.interrupt_handle().interrupt();
            script_err_limit!(vm.bx.threads.cur_ref().trap, "{}", m)
        }
    }
}

macro_rules! op {
    ($vm:ident, $m:ident, $name:ident, ($($arg:ident),*), $f:expr) => {
        $vm.add_method($m, id_lut!($name), script_args_def!($($arg = NIL),*), move |vm, args| {
            let _args = args;
            $(let $arg = vm.value(_args, id!($arg));)*
            native(vm, |vm, d| ($f)(vm, d, $($arg),*))
        });
    };
}

fn define_compute(vm: &mut ScriptVm) -> ScriptObject {
    let m = vm.new_module(id!(compute));
    op!(vm, m, buffer, (ty, n, opts), op_buffer);
    op!(vm, m, free, (buf), op_free);
    op!(vm, m, input, (name), op_input);
    op!(vm, m, fill, (buf, value), op_fill);
    op!(vm, m, copy, (src, dst), op_copy);
    op!(vm, m, write, (buf, values, at), op_write);
    op!(vm, m, run, (kernel, n, params, bind), op_run);
    op!(vm, m, repeat, (kernel, rounds, n, params, bind), op_repeat);
    op!(vm, m, sweep, (kernel, layers, n, dir, params, bind), op_sweep);
    op!(vm, m, reduce, (kernel, n, params, bind, block, slot), op_reduce);
    op!(vm, m, dispatch, (kernel, groups, params, bind), op_dispatch);
    op!(vm, m, sum, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Sum, block, slot));
    op!(vm, m, min, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Min, block, slot));
    op!(vm, m, max, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Max, block, slot));
    op!(vm, m, dot, (a, b, block, slot), |vm, d, a, b, block, slot| op_fold(vm, d, a, b, Fold::Sum, block, slot));
    op!(vm, m, scan, (src, dst), op_scan);
    op!(vm, m, restrict, (fine, coarse, opts), op_restrict);
    op!(vm, m, prolong, (coarse, fine, opts), op_prolong);
    op!(vm, m, read, (buf, at, n), op_read);
    op!(vm, m, fetch, (buf, at, n), op_fetch);
    op!(vm, m, take, (fetch), op_take);
    op!(vm, m, recall, (name, ty, inputs, with), op_recall);
    op!(vm, m, remember, (name, buf, inputs, with), op_remember);
    op!(vm, m, flush, (), op_flush);
    op!(vm, m, inbox, (defaults), op_inbox);
    op!(vm, m, wait_inbox, (defaults), op_wait_inbox);
    op!(vm, m, publish, (name, buf, opts), op_publish);
    op!(vm, m, state, (name, buf), op_state);
    op!(vm, m, value, (name, value), op_value);
    op!(vm, m, progress, (fraction, text), op_progress);
    op!(vm, m, device, (), op_device);
    op!(vm, m, stats, (), op_stats);
    m
}

// ---------------------------------------------------------------------------------------------
// Arguments

fn string(vm: &ScriptVm, v: ScriptValue) -> Option<String> {
    if let Some(id) = v.as_id() {
        return Some(id.to_string());
    }
    vm.heap().string_with(v, |_, s| s.to_string())
}

fn elem_ty(vm: &ScriptVm, v: ScriptValue) -> Option<ElemTy> {
    if let Some(name) = string(vm, v) {
        return ElemTy::from_name(&name);
    }
    use makepad_script::pod::{ScriptPodTyTag as P, ScriptPodVec as V};
    let ty = vm.heap().pod_type(v)?;
    let ty = vm.heap().pod_type_ref(ty).ty();
    Some(match ty.kind() {
        P::F32 => ElemTy::F32,
        P::I32 => ElemTy::I32,
        P::U32 => ElemTy::U32,
        P::Vec => match ty.as_vec()? {
            V::Vec2f => ElemTy::Vec2,
            V::Vec3f => ElemTy::Vec3,
            V::Vec4f => ElemTy::Vec4,
            _ => return None,
        },
        _ => return None,
    })
}

fn whole(v: ScriptValue, what: &str) -> Res<usize> {
    match v.as_number() {
        Some(x) if x >= 0.0 && x.fract() == 0.0 && x <= i32::MAX as f64 => Ok(x as usize),
        _ => Err(format!("{what}: a whole number below 2^31").into()),
    }
}

fn whole_or(v: ScriptValue, default: usize, what: &str) -> Res<usize> {
    if v.is_nil() {
        Ok(default)
    } else {
        whole(v, what)
    }
}

/// A count: a number, or `[nx, ny, nz]` (their product).
fn count(vm: &ScriptVm, v: ScriptValue, what: &str) -> Res<usize> {
    if let Some(a) = v.as_array() {
        let n = vm.heap().array_len(a);
        if n == 0 || n > 3 {
            return Err(format!("{what}: a count or [nx, ny, nz]").into());
        }
        let mut total = 1usize;
        for i in 0..n {
            total = total.saturating_mul(whole(vm.heap().array_index(a, i, NoTrap), what)?);
        }
        if total > i32::MAX as usize {
            return Err(format!("{what}: {total} elements is 2^31 or more").into());
        }
        return Ok(total);
    }
    whole(v, what)
}

/// An options object's field, nil when it has none.
fn field(vm: &ScriptVm, o: ScriptObject, key: LiveId) -> ScriptValue {
    let v = vm.value(o, key);
    if v.is_err() {
        NIL
    } else {
        v
    }
}

fn dims(vm: &ScriptVm, v: ScriptValue, what: &str) -> Res<[usize; 3]> {
    let a = v.as_array().filter(|a| vm.heap().array_len(*a) == 3).ok_or_else(|| format!("{what}: [nx, ny, nz]"))?;
    let mut out = [0; 3];
    for (k, o) in out.iter_mut().enumerate() {
        *o = whole(vm.heap().array_index(a, k, NoTrap), what)?;
    }
    Ok(out)
}

/// A buffer handle's slot, on either device.
fn any_buf(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let o = v.as_object().ok_or_else(|| format!("{what}: a buffer (from s.buffer or s.input)"))?;
    let slot = vm.value(o, id!(compute_buffer)).as_number();
    let gen = vm.value(o, id!(gen)).as_number();
    match (slot, gen) {
        (Some(slot), Some(gen)) if d.device.get(slot as usize, gen as u32).is_some() => Ok(slot as usize),
        (Some(_), Some(_)) => Err(format!("{what}: the buffer was freed").into()),
        _ => Err(format!("{what}: a buffer (from s.buffer or s.input)").into()),
    }
}

/// A buffer the op writes, on either device: not a session input.
fn any_written(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let b = any_buf(vm, d, v, what)?;
    if d.device.buffer(b).read_only() {
        return Err(format!("{what}: a session input is read-only").into());
    }
    Ok(b)
}

fn on_gpu(d: &Driver, b: usize) -> bool {
    d.device.buffer(b).on_gpu()
}

/// A CPU buffer's slot (the library ops run on the CPU device).
fn buf(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let b = any_buf(vm, d, v, what)?;
    if on_gpu(d, b) {
        return Err(format!("{what}: a CPU buffer (this one is on the GPU: s.copy it to one, or s.dispatch a shader kernel there)").into());
    }
    Ok(b)
}

/// A CPU buffer the op writes: not a session input.
fn written(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let b = buf(vm, d, v, what)?;
    if d.device.buffer(b).read_only() {
        return Err(format!("{what}: a session input is read-only").into());
    }
    Ok(b)
}

/// The session's GPU side, or the error an op on the GPU gets without one.
fn gpu_of<'a>(d: &'a mut Driver, what: &str) -> Res<&'a mut GpuSide> {
    d.gpu.as_mut().ok_or_else(|| format!("{what}: this session has no GPU device (s.device().gpu is nil)").into())
}

/// The session's end for a device error.
fn gpu_fatal(e: GpuError) -> ProgramError {
    match e {
        GpuError::Stopped => ProgramError::Cancelled,
        GpuError::DeviceLost => ProgramError::DeviceLost,
        GpuError::Failed(why) => ProgramError::Program(format!("the GPU device: {why}")),
    }
}

fn f32_buf(vm: &ScriptVm, d: &Driver, b: usize, what: &str) -> Res<()> {
    let _ = vm;
    if d.device.buffer(b).ty.is_f32() {
        Ok(())
    } else {
        Err(format!("{what}: an f32 (or vec2/3/4) buffer").into())
    }
}

fn handle(vm: &mut ScriptVm, slot: usize, gen: u32, ty: ElemTy, len: usize) -> ScriptValue {
    let o = vm.new_object();
    vm.set_value_def(o, id_lut!(compute_buffer), ScriptValue::from_f64(slot as f64));
    vm.set_value_def(o, id_lut!(gen), ScriptValue::from_f64(gen as f64));
    vm.set_value_def(o, id_lut!(len), ScriptValue::from_f64(len as f64));
    let t = vm.new_string(ty.name());
    vm.set_value_def(o, id_lut!(ty), t);
    o.into()
}

fn object(vm: &mut ScriptVm, fields: &[(&str, ScriptValue)]) -> ScriptValue {
    let o = vm.new_object();
    for (k, v) in fields {
        vm.set_value_def(o, key_id(k), *v);
    }
    o.into()
}

fn num(x: f64) -> ScriptValue {
    ScriptValue::from_f64(x)
}

// ---------------------------------------------------------------------------------------------
// Buffers

fn op_buffer(vm: &mut ScriptVm, d: &mut Driver, ty: ScriptValue, n: ScriptValue, opts: ScriptValue) -> Res<ScriptValue> {
    let what = "s.buffer(type, n, {device: \"cpu\" | \"gpu\"})";
    let ty = elem_ty(vm, ty).ok_or("s.buffer(type, n): the type is f32, i32, u32, f64, vec2, vec3 or vec4")?;
    let n = count(vm, n, what)?;
    if n == 0 {
        return Err("s.buffer(type, n): at least one element".into());
    }
    let device = match opts.as_object() {
        Some(o) => string(vm, field(vm, o, id!(device))),
        None if opts.is_nil() => None,
        None => return Err(format!("{what}: opts are an object").into()),
    };
    let gpu = match device.as_deref() {
        None | Some("cpu") => false,
        Some("gpu") => true,
        Some(other) => return Err(format!("{what}: no device `{other}`").into()),
    };
    let words = n.saturating_mul(ty.words());
    let bytes = words as u64 * 4;
    let mut available = d.limits.buffer_bytes.saturating_sub(d.buffer_bytes);
    if gpu {
        let g = gpu_of(d, what)?;
        available = available.min(g.link.info().budget_bytes.saturating_sub(g.bytes));
    }
    if words > i32::MAX as usize || bytes > available {
        // A value the program answers (a coarser level, fewer buffers), not an error.
        return Ok(object(vm, &[("refused", ScriptValue::from_bool(true)), ("wanted", num(bytes as f64)), ("available", num(available as f64))]));
    }
    d.buffer_bytes += bytes;
    if !gpu {
        let (slot, gen) = d.device.alloc(ty, Data::Owned(vec![0; words]));
        return Ok(handle(vm, slot, gen, ty, n));
    }
    let (slot, gen) = d.device.alloc(ty, Data::Gpu(words));
    gpu_of(d, what)?.bytes += bytes;
    record(vm, d, Step::Gpu(GpuOp::Alloc { buf: slot, bytes }))?;
    Ok(handle(vm, slot, gen, ty, n))
}

fn op_free(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue) -> Res<ScriptValue> {
    let b = any_written(vm, d, b, "s.free(buf)")?;
    let bytes = d.device.buffer(b).len_words() as u64 * 4;
    d.buffer_bytes -= bytes;
    let step = match on_gpu(d, b) {
        true => {
            gpu_of(d, "s.free(buf)")?.bytes -= bytes;
            Step::Gpu(GpuOp::Free { buf: b })
        }
        false => Step::Cpu(Op::Free { buf: b }),
    };
    d.device.retire(b);
    record(vm, d, step)?;
    Ok(NIL)
}

fn op_input(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue) -> Res<ScriptValue> {
    let name = string(vm, name).ok_or("s.input(name): the input's name")?;
    let Some(k) = d.inputs.iter().position(|(i, _)| i.name == name) else { return Ok(NIL) };
    let ty = d.inputs[k].0.ty;
    let len = d.inputs[k].0.data.len() / ty.words();
    let (slot, gen) = match d.inputs[k].1 {
        Some(h) => h,
        None => {
            let h = d.device.alloc(ty, Data::Shared(d.inputs[k].0.data.clone()));
            d.inputs[k].1 = Some(h);
            h
        }
    };
    Ok(handle(vm, slot, gen, ty, len))
}

/// A number as one word of a buffer of `ty`.
fn word(ty: ElemTy, x: f64) -> u32 {
    match ty {
        ElemTy::I32 => (x as i32) as u32,
        ElemTy::U32 => x as u32,
        _ => (x as f32).to_bits(),
    }
}

fn op_fill(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, value: ScriptValue) -> Res<ScriptValue> {
    let b = any_written(vm, d, b, "s.fill(buf, value)")?;
    let ty = d.device.buffer(b).ty;
    if ty == ElemTy::F64 {
        return Err("s.fill(buf, value): fill an f64 buffer with a kernel or s.write".into());
    }
    let x = value.as_number().ok_or("s.fill(buf, value): a number")?;
    let step = match on_gpu(d, b) {
        true => Step::Gpu(GpuOp::Fill { buf: b, word: word(ty, x) }),
        false => Step::Cpu(Op::Fill { buf: b, word: word(ty, x) }),
    };
    record(vm, d, step)?;
    Ok(NIL)
}

/// A copy within a device, or across: to the GPU from a CPU buffer as it is when the copy runs,
/// to the CPU once the GPU's work before it ran.
fn op_copy(vm: &mut ScriptVm, d: &mut Driver, src: ScriptValue, dst: ScriptValue) -> Res<ScriptValue> {
    let src = any_buf(vm, d, src, "s.copy(src, dst)")?;
    let dst = any_written(vm, d, dst, "s.copy(src, dst)")?;
    if src == dst {
        return Err("s.copy(src, dst): two buffers".into());
    }
    if d.device.buffer(src).len_words() != d.device.buffer(dst).len_words() {
        return Err("s.copy(src, dst): buffers of the same size".into());
    }
    let step = match (on_gpu(d, src), on_gpu(d, dst)) {
        (false, false) => Step::Cpu(Op::Copy { src, dst }),
        (true, true) => Step::Gpu(GpuOp::Copy { src, dst }),
        (false, true) => Step::Upload { src, dst },
        (true, false) => Step::Download { src, dst },
    };
    record(vm, d, step)?;
    Ok(NIL)
}

fn op_write(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, values: ScriptValue, at: ScriptValue) -> Res<ScriptValue> {
    let b = any_written(vm, d, b, "s.write(buf, values, at)")?;
    let ty = d.device.buffer(b).ty;
    let a = values.as_array().ok_or("s.write(buf, values, at): an array of numbers")?;
    let n = vm.heap().array_len(a);
    if n > MAX_WRITE {
        return Err(format!("s.write: at most {MAX_WRITE} values").into());
    }
    let mut words = Vec::with_capacity(n * if ty == ElemTy::F64 { 2 } else { 1 });
    for i in 0..n {
        let x = vm.heap().array_index(a, i, NoTrap).as_number().ok_or("s.write(buf, values, at): an array of numbers")?;
        if ty == ElemTy::F64 {
            let bits = x.to_bits();
            words.push(bits as u32);
            words.push((bits >> 32) as u32);
        } else {
            words.push(word(ty, x));
        }
    }
    // `at` counts elements; the values are an f64 buffer's numbers, or the other types' words.
    let at = whole_or(at, 0, "s.write(buf, values, at)")? * ty.words();
    if at + words.len() > d.device.buffer(b).len_words() {
        return Err("s.write(buf, values, at): past the buffer's end".into());
    }
    let step = match on_gpu(d, b) {
        true => Step::Gpu(GpuOp::Upload { buf: b, offset: at as u64 * 4, bytes: bytes_of(&words) }),
        false => Step::Cpu(Op::Write { buf: b, at, words }),
    };
    record(vm, d, step)?;
    Ok(NIL)
}

// ---------------------------------------------------------------------------------------------
// Kernels

/// The kernel an object declares, compiled once per object.
fn kernel(vm: &mut ScriptVm, d: &mut Driver, v: ScriptValue) -> Res<usize> {
    let obj = v.as_object().ok_or("a kernel: a mod.kernel object {.. element: fn(i) {..}}")?;
    if let Some(k) = d.kernels.iter().position(|k| k.obj == obj) {
        return Ok(k);
    }
    if d.kernels.len() >= d.limits.kernels {
        return Err(Fail::Fatal(ProgramError::OverBudget("kernels")));
    }
    let label = string(vm, field(vm, obj, id!(label))).unwrap_or_else(|| format!("kernel{}", d.kernels.len()));
    let math = match string(vm, field(vm, obj, id!(math))).as_deref() {
        None | Some("fast") => MathMode::Fast,
        Some("portable") => MathMode::Portable,
        Some("exact32") => MathMode::Exact32,
        Some(_) => return Err("a kernel's math: \"portable\", \"exact32\" or \"fast\"".into()),
    };
    // The whole Kernel object (its tables and records too), read compiler-side.
    let counts = CountPolicy { fields: &[], entry: obj, output: "", binding: "", record_suffix: "" };
    let t = Instant::now();
    let mut prepared = prepare_object(vm, obj, &label, &["label", "math"], &[], &[], counts, math);
    let program = prepared.take_program().ok_or_else(|| compile_errors(&prepared))?;
    let mut compiled = program.compile(d.device.backend(), &[]);
    let compiled = compiled.take_kernel().ok_or_else(|| compile_errors(&compiled))?;
    compiled_in(d, t);
    let root = vm.new_object_ref(obj);
    let k = d.device.add_kernel(compiled);
    debug_assert_eq!(k, d.kernels.len());
    d.kernels.push(KernelEntry { obj, _root: root, label, gpu: None });
    Ok(k)
}

/// The time a kernel's compile since `t` took, in the program's stats.
fn compiled_in(d: &mut Driver, t: Instant) {
    note(d, "(compiles kernels)".into(), &OpDone { nanos: t.elapsed().as_nanos() as u64, elements: 1, ..OpDone::default() });
}

/// A compiled kernel's code, named: kernels with the same code run alike on a device.
fn code_key(code: &KernelCode) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    match code {
        KernelCode::Spirv(words) => words.hash(&mut h),
        KernelCode::Msl(text) | KernelCode::Hlsl(text) => text.hash(&mut h),
    }
    h.finish()
}

/// Sends a compiled kernel to the device, in order with the session's ops: its GPU index.
fn install(vm: &mut ScriptVm, d: &mut Driver, compiled: CompiledKernel, label: &str, what: &str) -> Res<usize> {
    if d.kernels.len() + d.shaders.len() >= d.limits.kernels {
        return Err(Fail::Fatal(ProgramError::OverBudget("kernels")));
    }
    let g = gpu_of(d, what)?;
    let id = g.kernels;
    g.kernels += 1;
    let key = code_key(&compiled.code);
    record(vm, d, Step::Gpu(GpuOp::Kernel { kernel: id, key, compiled, label: label.to_string() }))?;
    Ok(id)
}

/// Element kernel `k`'s GPU form, compiled and sent the first time: its GPU index, and per
/// storage buffer of its layout the kernel's buffer it binds (None: the args).
fn element_on_gpu(vm: &mut ScriptVm, d: &mut Driver, k: usize, what: &str) -> Res<(usize, Vec<Option<usize>>)> {
    if let Some(g) = &d.kernels[k].gpu {
        return Ok(g.clone());
    }
    let kernel = d.device.kernel(k).clone();
    let label = d.kernels[k].label.clone();
    let target = gpu_of(d, what)?.link.info().target;
    let t = Instant::now();
    let compiled = ElementRoute::new(&kernel, ELEMENT_FRAME_WORDS).compile(target).view().to_kernel().map_err(|e| format!("{what}: `{label}` has no GPU form: {e}"))?;
    compiled_in(d, t);
    let args = LiveId::from_str(ELEMENT_ARGS);
    let mut binds = Vec::new();
    for b in &compiled.layout.buffers {
        binds.push(match b.name == args {
            true => None,
            false => Some(kernel.buffer_views().position(|v| LiveId::from_str(v.name) == b.name).ok_or_else(|| format!("{what}: `{label}`'s GPU form binds `{}`, which it does not declare", b.name))?),
        });
    }
    let id = install(vm, d, compiled, &label, what)?;
    d.kernels[k].gpu = Some((id, binds.clone()));
    Ok((id, binds))
}

/// Shader kernel `v` (a `mod.shader` compute object), compiled for the device the first time.
fn shader(vm: &mut ScriptVm, d: &mut Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let obj = v.as_object().ok_or_else(|| format!("{what}: a shader kernel {{workgroup_size: n .. main: fn() {{..}}}}"))?;
    if let Some(k) = d.shaders.iter().position(|k| k.obj == obj) {
        return Ok(k);
    }
    let label = string(vm, field(vm, obj, id!(label))).unwrap_or_else(|| format!("shader{}", d.shaders.len()));
    let target: ComputeTarget = gpu_of(d, what)?.link.info().target;
    // through the crate's op and its view: under `makepad run` the script crate is the loader's
    let t = Instant::now();
    let compiled = compile_compute_kernel_result(vm, obj, target).view().to_kernel().map_err(|e| format!("{what}: `{label}`: {e}"))?;
    compiled_in(d, t);
    if !compiled.layout.storage_textures.is_empty() {
        return Err(format!("{what}: `{label}` binds textures; a program's kernels bind buffers").into());
    }
    let layout = compiled.layout.clone();
    let gpu = install(vm, d, compiled, &label, what)?;
    let root = vm.new_object_ref(obj);
    d.shaders.push(ShaderEntry { obj, _root: root, label, gpu, layout });
    Ok(d.shaders.len() - 1)
}

/// Counts `work` against the session's budget.
fn charge(d: &mut Driver, work: u64) -> Res<()> {
    d.work = d.work.saturating_add(work);
    if d.work > d.limits.work {
        return Err(Fail::Fatal(ProgramError::OverBudget("work")));
    }
    Ok(())
}

/// An element kernel's run on the GPU: its args as the CPU's call sets its context (params
/// clamped as there), then its host buffers' lengths.
fn gpu_run(vm: &mut ScriptVm, d: &mut Driver, run: Run, what: &str) -> Res<()> {
    let (id, binds) = element_on_gpu(vm, d, run.kernel, what)?;
    let kernel = d.device.kernel(run.kernel).clone();
    let mut args = vec![0u32; kernel.ctx_words()];
    kernel.init_context(&mut args);
    for (name, value) in &run.params {
        let k = kernel.param_index(name).expect("make_run checks each param");
        let p = kernel.param_view(k);
        let v = if value.is_finite() { value.clamp(p.min, p.max) } else { p.default };
        args[K_PARAMS as usize + k] = v.to_bits();
    }
    args[K_SEED as usize] = d.seed;
    args[K_COUNT as usize] = run.count as u32;
    for kb in 0..kernel.buffer_count() {
        args.push(run.bind.iter().find(|(b, _)| *b == kb).map_or(0, |(_, slot)| d.device.buffer(*slot).len_words() as u32));
    }
    let buffers = binds.iter().map(|kb| kb.map(|kb| run.bind.iter().find(|(b, _)| *b == kb).expect("make_run binds every buffer").1)).collect();
    charge(d, (run.count as u64).saturating_mul(kernel.cost().max(1)))?;
    record(vm, d, Step::Gpu(GpuOp::Run { kernel: id, count: run.count as u32, args, buffers }))
}

/// Params as a shader kernel's layout takes them: each a number, or as many as its lanes.
fn shader_params(vm: &ScriptVm, layout: &ComputeLayout, params: ScriptValue, what: &str) -> Res<Vec<(LiveId, Vec<u8>)>> {
    let mut out = Vec::new();
    let Some(p) = params.as_object() else {
        return match params.is_nil() {
            true => Ok(out),
            false => Err(format!("{what}: params are an object {{name: number or [numbers]}}").into()),
        };
    };
    for e in vm.heap().map_entries_ordered(p) {
        let Some(name) = e.key.as_id() else { continue };
        let slot = layout.slot(name).ok_or_else(|| format!("{what}: the kernel has no param `{name}`"))?;
        let ComputeSlotKind::Param { scalar, lanes } = slot.kind else {
            return Err(format!("{what}: `{name}` is a buffer: bind it").into());
        };
        let mut values = Vec::new();
        match e.value.as_array() {
            Some(a) => {
                for i in 0..vm.heap().array_len(a) {
                    values.push(vm.heap().array_index(a, i, NoTrap).as_number().ok_or_else(|| format!("{what}: param `{name}` takes numbers"))?);
                }
            }
            None => values.push(e.value.as_number().ok_or_else(|| format!("{what}: param `{name}` takes a number or {lanes}"))?),
        }
        if values.len() != lanes as usize {
            return Err(format!("{what}: param `{name}` takes {lanes} values").into());
        }
        let mut bytes: Vec<u8> = Vec::with_capacity(slot.size as usize);
        for x in values {
            bytes.extend(match scalar {
                IrScalar::F32 => (x as f32).to_le_bytes(),
                IrScalar::U32 => (x as u32).to_le_bytes(),
                IrScalar::I32 => (x as i32).to_le_bytes(),
                IrScalar::Bool => ((x != 0.0) as u32).to_le_bytes(),
                IrScalar::F16 => return Err(format!("{what}: param `{name}` is f16, which a program does not set").into()),
            });
        }
        bytes.resize(slot.size as usize, 0);
        out.push((name, bytes));
    }
    Ok(out)
}

fn op_dispatch(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, groups: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.dispatch(kernel, groups, params, bind)";
    let k = shader(vm, d, k, what)?;
    let mut g = [1u32; 3];
    match groups.as_array() {
        Some(a) if (1..=3).contains(&vm.heap().array_len(a)) => {
            for (i, o) in g.iter_mut().enumerate().take(vm.heap().array_len(a)) {
                *o = whole(vm.heap().array_index(a, i, NoTrap), what)? as u32;
            }
        }
        Some(_) => return Err(format!("{what}: groups are a count or [x, y, z]").into()),
        None => g[0] = whole(groups, what)? as u32,
    }
    if g.iter().any(|n| *n == 0 || *n > 65535) {
        return Err(format!("{what}: 1 to 65535 groups an axis").into());
    }
    let layout = d.shaders[k].layout.clone();
    let params = shader_params(vm, &layout, params, what)?;
    let b = bind.as_object().ok_or_else(|| format!("{what}: bind is an object {{buffer name: buffer}}"))?;
    let mut bound: Vec<Option<usize>> = vec![None; layout.buffers.len()];
    for e in vm.heap().map_entries_ordered(b) {
        let Some(name) = e.key.as_id() else { continue };
        let at = layout.buffers.iter().position(|b| b.name == name).ok_or_else(|| format!("{what}: the kernel has no buffer `{name}`"))?;
        let slot = any_buf(vm, d, e.value, &format!("{what}: `{name}`"))?;
        if !on_gpu(d, slot) {
            return Err(format!("{what}: `{name}` is a CPU buffer: s.copy it to a GPU one").into());
        }
        let bytes = d.device.buffer(slot).len_words() as u64 * 4;
        if bytes % layout.buffers[at].alignment.max(1) as u64 != 0 {
            return Err(format!("{what}: `{name}` holds {bytes} bytes, not a multiple of the {} its elements take", layout.buffers[at].alignment).into());
        }
        bound[at] = Some(slot);
    }
    let mut buffers = Vec::with_capacity(bound.len());
    for (i, slot) in bound.iter().enumerate() {
        let slot = slot.ok_or_else(|| format!("{what}: bind the kernel's buffer `{}`", layout.buffers[i].name))?;
        if layout.buffers[i].writable && bound.iter().enumerate().any(|(j, o)| j != i && *o == Some(slot)) {
            return Err(format!("{what}: a buffer the kernel writes is bound once (`{}`)", layout.buffers[i].name).into());
        }
        buffers.push(slot);
    }
    let threads = g.iter().map(|n| *n as u64).product::<u64>() * layout.workgroup.iter().map(|n| *n as u64).product::<u64>();
    charge(d, threads)?;
    let kernel = d.shaders[k].gpu;
    record(vm, d, Step::Gpu(GpuOp::Dispatch { kernel, groups: g, params, buffers }))?;
    Ok(NIL)
}

fn compile_errors(c: &KernelCompilation) -> String {
    (0..c.error_count()).map(|k| c.error(k).message.to_string()).collect::<Vec<_>>().join("\n")
}

/// A kernel run, checked against the kernel: every param it is given exists, every buffer it
/// declares is bound, written buffers are the session's own and bound once, and an
/// element-local kernel's written buffers hold every element (so how a call splits never
/// changes whether it runs). With whether its buffers are on the GPU (all of them, or none).
fn make_run(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue, what: &str) -> Res<(Run, bool)> {
    let kernel_index = kernel(vm, d, k)?;
    let count = count(vm, n, what)?;
    let kernel = d.device.kernel(kernel_index).clone();
    let mut run_params = Vec::new();
    if let Some(p) = params.as_object() {
        for e in vm.heap().map_entries_ordered(p) {
            let Some(name) = e.key.as_id() else { continue };
            let name = name.to_string();
            if kernel.param_index(&name).is_none() {
                return Err(format!("{what}: the kernel has no param `{name}`").into());
            }
            let x = e.value.as_number().ok_or_else(|| format!("{what}: param `{name}` is a number"))?;
            run_params.push((name, x as f32));
        }
    } else if !params.is_nil() {
        return Err(format!("{what}: params are an object {{name: number}}").into());
    }
    let mut run_bind: Vec<(usize, usize)> = Vec::new();
    let b = bind.as_object().ok_or_else(|| format!("{what}: bind is an object {{buffer name: buffer}}"))?;
    for e in vm.heap().map_entries_ordered(b) {
        let Some(name) = e.key.as_id() else { continue };
        let name = name.to_string();
        let kb = kernel.buffer_index(&name).filter(|k| *k > 0).ok_or_else(|| format!("{what}: the kernel has no buffer `{name}`"))?;
        let slot = any_buf(vm, d, e.value, &format!("{what}: `{name}`"))?;
        let view = kernel.buffer_view(kb);
        let buffer = d.device.buffer(slot);
        let words = buffer.len_words() as u64;
        let need = match view.access {
            Access::Read => 0,
            Access::Write if kernel.parallel_safe() => view.stride as u64 * count as u64,
            Access::Write => 0,
            Access::Emit { width, capacity } => width as u64 * capacity as u64 * count as u64,
            Access::EmitCount => count as u64,
        };
        if view.access != Access::Read && buffer.read_only() {
            return Err(format!("{what}: `{name}` is written by the kernel; a session input is read-only").into());
        }
        if need > words {
            return Err(format!("{what}: `{name}` has {words} words; {count} elements write {need}").into());
        }
        run_bind.push((kb, slot));
    }
    for (k, view) in kernel.buffer_views().enumerate().skip(1) {
        if !run_bind.iter().any(|(kb, _)| *kb == k) {
            return Err(format!("{what}: bind the kernel's buffer `{}`", view.name).into());
        }
    }
    for (i, (kb, slot)) in run_bind.iter().enumerate() {
        let writes = kernel.buffer_view(*kb).access != Access::Read;
        for (kb2, slot2) in &run_bind[i + 1..] {
            if slot == slot2 && (writes || kernel.buffer_view(*kb2).access != Access::Read) {
                return Err(format!("{what}: a buffer the kernel writes is bound once (`{}`, `{}`)", kernel.buffer_view(*kb).name, kernel.buffer_view(*kb2).name).into());
            }
        }
    }
    let gpu = run_bind.iter().filter(|(_, slot)| on_gpu(d, *slot)).count();
    if gpu != 0 && gpu != run_bind.len() {
        return Err(format!("{what}: the kernel's buffers are all on the CPU or all on the GPU").into());
    }
    Ok((Run { kernel: kernel_index, count, params: run_params, bind: run_bind, repeat: None }, gpu != 0))
}

/// [`make_run`] for a run the CPU device makes: its buffers there.
fn cpu_run(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue, what: &str) -> Res<Run> {
    match make_run(vm, d, k, n, params, bind, what)? {
        (run, false) => Ok(run),
        (_, true) => Err(format!("{what}: CPU buffers (on the GPU: s.run or s.dispatch)").into()),
    }
}

fn lanes(d: &Driver, run: &Run) -> usize {
    d.device.kernel(run.kernel).reduce_parts().1
}

fn op_run(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.run(kernel, n, params, bind)";
    let (run, gpu) = make_run(vm, d, k, n, params, bind, what)?;
    if lanes(d, &run) > 0 {
        return Err("s.run: a reduce kernel runs with s.reduce(kernel, n, params, bind, block, slot)".into());
    }
    match gpu {
        true => gpu_run(vm, d, run, what)?,
        false => record(vm, d, Step::Cpu(Op::Run(run)))?,
    }
    Ok(NIL)
}

fn repeated(vm: &mut ScriptVm, d: &mut Driver, mut run: Run, rounds: ScriptValue, param: &'static str, down: bool, what: &str) -> Res<ScriptValue> {
    let rounds = whole(rounds, what)?;
    if rounds as u64 > MAX_ROUNDS as u64 {
        return Err(format!("{what}: at most {MAX_ROUNDS} rounds").into());
    }
    if d.device.kernel(run.kernel).param_index(param).is_none() {
        return Err(format!("{what}: the kernel declares a `{param}` param (each round's index)").into());
    }
    if lanes(d, &run) > 0 {
        return Err(format!("{what}: an element kernel").into());
    }
    run.repeat = Some(Repeat { rounds: rounds as u32, param, down });
    record(vm, d, Step::Cpu(Op::Run(run)))?;
    Ok(NIL)
}

fn op_repeat(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, rounds: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.repeat(kernel, rounds, n, params, bind)";
    let run = cpu_run(vm, d, k, n, params, bind, what)?;
    repeated(vm, d, run, rounds, "round", false, what)
}

fn op_sweep(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, layers: ScriptValue, n: ScriptValue, dir: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.sweep(kernel, layers, n, \"up\" | \"down\", params, bind)";
    let down = match string(vm, dir).as_deref() {
        Some("up") => false,
        Some("down") => true,
        _ => return Err(format!("{what}: the direction is \"up\" or \"down\"").into()),
    };
    let run = cpu_run(vm, d, k, n, params, bind, what)?;
    repeated(vm, d, run, layers, "layer", down, what)
}

/// A block's slot range a result writes: an f32 buffer of the session's own.
fn block_slot(vm: &ScriptVm, d: &Driver, block: ScriptValue, slot: ScriptValue, lanes: usize, what: &str) -> Res<(usize, usize)> {
    let b = written(vm, d, block, what)?;
    f32_buf(vm, d, b, what)?;
    let slot = whole_or(slot, 0, what)?;
    if slot + lanes > d.device.buffer(b).len_words() {
        return Err(format!("{what}: block slots {slot}..{} are past its end", slot + lanes).into());
    }
    Ok((b, slot))
}

fn op_reduce(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue, block: ScriptValue, slot: ScriptValue) -> Res<ScriptValue> {
    let what = "s.reduce(kernel, n, params, bind, block, slot)";
    let run = cpu_run(vm, d, k, n, params, bind, what)?;
    let lanes = lanes(d, &run);
    if lanes == 0 {
        return Err(format!("{what}: a reduce kernel (reduce_sum, reduce_min or reduce_max)").into());
    }
    let (block, slot) = block_slot(vm, d, block, slot, lanes, what)?;
    if run.bind.iter().any(|(kb, b)| *b == block && d.device.kernel(run.kernel).buffer_view(*kb).access != Access::Read) {
        return Err(format!("{what}: the block is not one of the kernel's written buffers").into());
    }
    record(vm, d, Step::Cpu(Op::Reduce { run, block, slot }))?;
    Ok(NIL)
}

fn op_fold(vm: &mut ScriptVm, d: &mut Driver, a: ScriptValue, b: ScriptValue, fold: Fold, block: ScriptValue, slot: ScriptValue) -> Res<ScriptValue> {
    let what = match (fold, b.is_nil()) {
        (_, false) => "s.dot(a, b, block, slot)",
        (Fold::Sum, _) => "s.sum(a, block, slot)",
        (Fold::Min, _) => "s.min(a, block, slot)",
        (Fold::Max, _) => "s.max(a, block, slot)",
    };
    let a = buf(vm, d, a, what)?;
    f32_buf(vm, d, a, what)?;
    let b = if b.is_nil() {
        None
    } else {
        let b = buf(vm, d, b, what)?;
        f32_buf(vm, d, b, what)?;
        if d.device.buffer(a).len_words() != d.device.buffer(b).len_words() {
            return Err(format!("{what}: a and b of the same size").into());
        }
        Some(b)
    };
    let (block, slot) = block_slot(vm, d, block, slot, 1, what)?;
    record(vm, d, Step::Cpu(Op::Fold { a, b, fold, block, slot }))?;
    Ok(NIL)
}

fn op_scan(vm: &mut ScriptVm, d: &mut Driver, src: ScriptValue, dst: ScriptValue) -> Res<ScriptValue> {
    let what = "s.scan(src, dst)";
    let src = buf(vm, d, src, what)?;
    let dst = written(vm, d, dst, what)?;
    let ints = |b: usize| matches!(d.device.buffer(b).ty, ElemTy::I32 | ElemTy::U32);
    if !ints(src) || !ints(dst) {
        return Err(format!("{what}: i32 or u32 buffers").into());
    }
    if src == dst {
        return Err(format!("{what}: two buffers").into());
    }
    let n = d.device.buffer(src).len_words();
    if d.device.buffer(dst).len_words() < n {
        return Err(format!("{what}: dst holds src's {n} elements (n + 1 to take the total)").into());
    }
    record(vm, d, Step::Cpu(Op::Scan { src, dst }))?;
    Ok(NIL)
}

/// `{grid: "vertex" | "cell", dims: [nx, ny, nz] (the fine level's cells), comps, kind}`.
fn grid_opts(vm: &ScriptVm, opts: ScriptValue, what: &str) -> Res<(Grid, Option<String>)> {
    let o = opts.as_object().ok_or_else(|| format!("{what}: opts {{grid: \"vertex\" | \"cell\", dims: [nx, ny, nz], comps, kind}}"))?;
    let vertex = match string(vm, field(vm, o, id!(grid))).as_deref() {
        Some("vertex") => true,
        Some("cell") => false,
        _ => return Err(format!("{what}: grid is \"vertex\" or \"cell\"").into()),
    };
    let cells = dims(vm, field(vm, o, id!(dims)), what)?;
    if cells.iter().any(|c| *c < 2 || c % 2 == 1) {
        return Err(format!("{what}: the fine level's cells are even, at least 2 per axis").into());
    }
    let comps = whole_or(field(vm, o, id!(comps)), 1, what)?;
    if comps == 0 || comps > 16 {
        return Err(format!("{what}: 1 to 16 comps").into());
    }
    Ok((Grid { cells, vertex, comps }, string(vm, field(vm, o, id!(kind)))))
}

fn transfer_bufs(vm: &ScriptVm, d: &Driver, from: ScriptValue, to: ScriptValue, from_words: usize, to_words: usize, what: &str) -> Res<(usize, usize)> {
    let from = buf(vm, d, from, what)?;
    let to = written(vm, d, to, what)?;
    f32_buf(vm, d, from, what)?;
    f32_buf(vm, d, to, what)?;
    if from == to {
        return Err(format!("{what}: two buffers").into());
    }
    if d.device.buffer(from).len_words() != from_words || d.device.buffer(to).len_words() != to_words {
        return Err(format!("{what}: the levels hold {from_words} and {to_words} values").into());
    }
    Ok((from, to))
}

fn op_restrict(vm: &mut ScriptVm, d: &mut Driver, fine: ScriptValue, coarse: ScriptValue, opts: ScriptValue) -> Res<ScriptValue> {
    let what = "s.restrict(fine, coarse, opts)";
    let (grid, kind) = grid_opts(vm, opts, what)?;
    let kind = match (grid.vertex, kind.as_deref()) {
        (true, None | Some("transpose")) => Transfer::Transpose,
        (true, Some("full_weight")) => Transfer::FullWeight,
        (false, None | Some("average")) => Transfer::Average,
        (false, Some("sum")) => Transfer::Sum,
        _ => return Err(format!("{what}: kind is \"transpose\" or \"full_weight\" (vertex), \"average\" or \"sum\" (cell)").into()),
    };
    let (fine, coarse) = transfer_bufs(vm, d, fine, coarse, grid.words(), grid.coarse().words(), what)?;
    record(vm, d, Step::Cpu(Op::Restrict { fine, coarse, grid, kind }))?;
    Ok(NIL)
}

fn op_prolong(vm: &mut ScriptVm, d: &mut Driver, coarse: ScriptValue, fine: ScriptValue, opts: ScriptValue) -> Res<ScriptValue> {
    let what = "s.prolong(coarse, fine, opts)";
    let (grid, kind) = grid_opts(vm, opts, what)?;
    let kind = match (grid.vertex, kind.as_deref()) {
        (true, None | Some("trilinear")) => Transfer::Trilinear,
        (false, None | Some("injection")) => Transfer::Injection,
        _ => return Err(format!("{what}: kind is \"trilinear\" (vertex) or \"injection\" (cell)").into()),
    };
    let (coarse, fine) = transfer_bufs(vm, d, coarse, fine, grid.coarse().words(), grid.words(), what)?;
    record(vm, d, Step::Cpu(Op::Prolong { coarse, fine, grid, kind }))?;
    Ok(NIL)
}

// ---------------------------------------------------------------------------------------------
// Segments

fn record(vm: &mut ScriptVm, d: &mut Driver, step: Step) -> Res<()> {
    d.pending.push(step);
    if d.pending.len() >= SEGMENT_OPS {
        boundary(vm, d)?;
    }
    Ok(())
}

/// A GPU op's name in the stats.
fn gpu_label(d: &Driver, op: &GpuOp) -> Option<String> {
    let kernel = |id: usize| d.kernels.iter().find(|k| k.gpu.as_ref().is_some_and(|g| g.0 == id)).map(|k| k.label.clone());
    let shader = |id: usize| d.shaders.iter().find(|k| k.gpu == id).map(|k| k.label.clone());
    match op {
        GpuOp::Run { kernel: id, .. } => kernel(*id),
        GpuOp::Dispatch { kernel: id, .. } => shader(*id),
        GpuOp::Fill { .. } => Some("fill".into()),
        GpuOp::Copy { .. } => Some("copy".into()),
        GpuOp::Upload { .. } => Some("upload".into()),
        GpuOp::Kernel { .. } | GpuOp::Alloc { .. } | GpuOp::Free { .. } | GpuOp::Read { .. } => None,
    }
}

fn note(d: &mut Driver, label: String, done: &OpDone) {
    let stat = match d.stats.iter_mut().position(|(l, _)| *l == label) {
        Some(k) => &mut d.stats[k].1,
        None => {
            d.stats.push((label, Stat::default()));
            &mut d.stats.last_mut().unwrap().1
        }
    };
    stat.calls += 1;
    stat.nanos += done.nanos;
    stat.work += done.work;
    stat.elements += done.elements;
}

/// The segment's steps not run (interrupted): its frees still run, and the GPU's kernels and
/// buffers still come and go, so the tables stay as the program made them.
fn drop_steps(d: &mut Driver, steps: impl Iterator<Item = Step>) {
    let shared = d.shared.clone();
    for step in steps {
        match step {
            Step::Cpu(op @ Op::Free { .. }) => {
                let _ = d.device.execute(op, 0, &shared.running);
            }
            Step::Gpu(op @ (GpuOp::Kernel { .. } | GpuOp::Alloc { .. } | GpuOp::Free { .. })) => {
                if let GpuOp::Free { buf } = op {
                    let _ = d.device.execute(Op::Free { buf }, 0, &shared.running);
                }
                if let Some(g) = d.gpu.as_mut() {
                    g.batch.push(op);
                }
            }
            _ => {}
        }
    }
}

/// Bytes as words (a GPU buffer's: little-endian, as every target makepad runs on).
fn words_of(bytes: &[u8]) -> Vec<u32> {
    let n = bytes.len() / 4;
    let mut words = vec![0u32; n];
    bytemuck::cast_slice_mut::<u32, u8>(&mut words).copy_from_slice(&bytes[..n * 4]);
    words
}

/// Words as a GPU buffer's bytes: one copy of their memory.
fn bytes_of(words: &[u32]) -> Vec<u8> {
    bytemuck::cast_slice::<u32, u8>(words).to_vec()
}

/// A GPU buffer's words `at..at + n`, once the work sent before ran: None where the session
/// was interrupted.
fn gpu_read(d: &mut Driver, b: usize, at: usize, n: usize) -> Res<Option<Vec<u32>>> {
    let g = gpu_of(d, "s.read")?;
    g.batch.push(GpuOp::Read { buf: b, offset: at as u64 * 4, bytes: n as u64 * 4 });
    let t = Instant::now();
    let answer = g.send(true);
    waited(d, t);
    match answer {
        Ok(mut reads) => Ok(Some(words_of(&reads.pop().unwrap_or_default()))),
        Err(GpuError::Stopped) if !d.shared.cancelled() => Ok(None),
        Err(e) => Err(Fail::Fatal(gpu_fatal(e))),
    }
}

/// The program's time waiting for its GPU device's answers since `t`, in its stats (what the
/// device's work and the reads' trips cost the program beyond its own ops).
fn waited(d: &mut Driver, t: Instant) {
    note(d, "(waits for the GPU)".into(), &OpDone { nanos: t.elapsed().as_nanos() as u64, ..OpDone::default() });
}

/// Runs the recorded segment: the CPU's ops here, in order, and the GPU's sent to its device
/// in order (it runs them as they come; a copy to the CPU waits for them). Ok(false): an
/// interrupting message came, and the segment's ops not yet run were dropped (its frees still
/// run).
fn boundary(vm: &mut ScriptVm, d: &mut Driver) -> Res<bool> {
    let shared = d.shared.clone();
    let mut steps = std::mem::take(&mut d.pending).into_iter();
    let mut ran_all = true;
    while let Some(step) = steps.next() {
        if shared.cancelled() {
            return Err(Fail::Fatal(ProgramError::Cancelled));
        }
        if shared.interrupted() {
            ran_all = false;
            drop_steps(d, steps.by_ref());
            break;
        }
        match step {
            Step::Cpu(op) => {
                let label = match &op {
                    Op::Run(run) | Op::Reduce { run, .. } => d.kernels[run.kernel].label.clone(),
                    op => op.label().to_string(),
                };
                let work_left = d.limits.work.saturating_sub(d.work);
                match d.device.execute(op, work_left, &shared.running) {
                    Ok(done) => {
                        d.work += done.work;
                        note(d, label, &done);
                    }
                    Err(OpError::Kernel(KernelError::Cancelled)) if !shared.cancelled() => {
                        ran_all = false;
                        drop_steps(d, steps.by_ref());
                        break;
                    }
                    Err(OpError::Kernel(KernelError::Cancelled)) => return Err(Fail::Fatal(ProgramError::Cancelled)),
                    Err(OpError::Kernel(KernelError::OverBudget { .. })) => return Err(Fail::Fatal(ProgramError::OverBudget("work"))),
                    Err(OpError::Kernel(e)) => return Err(Fail::Fatal(ProgramError::Program(format!("{label}: {e}")))),
                }
            }
            Step::Gpu(op) => {
                if let Some(label) = gpu_label(d, &op) {
                    let elements = match &op {
                        GpuOp::Run { count, .. } => *count as u64,
                        _ => 0,
                    };
                    note(d, label, &OpDone { elements, ..OpDone::default() });
                }
                if let GpuOp::Free { buf } = op {
                    let _ = d.device.execute(Op::Free { buf }, 0, &shared.running);
                }
                gpu_of(d, "a GPU op")?.batch.push(op);
            }
            Step::Upload { src, dst } => {
                let t = Instant::now();
                let bytes = bytes_of(d.device.buffer(src).data.words());
                note(d, "(copies to the GPU)".into(), &OpDone { nanos: t.elapsed().as_nanos() as u64, elements: bytes.len() as u64 / 4, ..OpDone::default() });
                gpu_of(d, "s.copy")?.batch.push(GpuOp::Upload { buf: dst, offset: 0, bytes });
            }
            Step::Download { src, dst } => {
                let n = d.device.buffer(src).len_words();
                match gpu_read(d, src, 0, n)? {
                    Some(words) => d.device.owned(dst).copy_from_slice(&words),
                    None => {
                        ran_all = false;
                        drop_steps(d, steps.by_ref());
                        break;
                    }
                }
            }
        }
    }
    if let Some(g) = d.gpu.as_mut() {
        g.send(false).map_err(|e| Fail::Fatal(gpu_fatal(e)))?;
    }
    // What the segment's ops left behind is collected at boundaries, the driver's only pauses.
    vm.bx.gc_if_needed();
    Ok(ran_all && !shared.interrupted())
}

fn words_to_array(vm: &mut ScriptVm, ty: ElemTy, words: &[u32]) -> ScriptValue {
    let a = vm.heap_mut().new_array();
    if ty == ElemTy::F64 {
        for w in words.chunks_exact(2) {
            vm.array_push_trap(a, num(f64::from_bits(w[0] as u64 | (w[1] as u64) << 32)));
        }
    } else {
        for w in words {
            let x = match ty {
                ElemTy::I32 => *w as i32 as f64,
                ElemTy::U32 => *w as f64,
                _ => f32::from_bits(*w) as f64,
            };
            vm.array_push_trap(a, num(x));
        }
    }
    a.into()
}

/// A read's buffer, element type and words: `at..at + n` of `b` (all of it from `at` by
/// default), within the read limit.
fn read_range(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, at: ScriptValue, n: ScriptValue, what: &str) -> Res<(usize, ElemTy, usize, usize)> {
    let b = any_buf(vm, d, b, what)?;
    let ty = d.device.buffer(b).ty;
    let len = d.device.buffer(b).len_words() / ty.words();
    let at = whole_or(at, 0, what)?;
    let n = whole_or(n, len.saturating_sub(at), what)?;
    if at + n > len {
        return Err(format!("{what}: elements {at}..{} are past its {len}", at + n).into());
    }
    if n * ty.words() > d.limits.read_words {
        return Err(format!("{what}: at most {} words at once", d.limits.read_words).into());
    }
    Ok((b, ty, at * ty.words(), n * ty.words()))
}

fn op_read(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, at: ScriptValue, n: ScriptValue) -> Res<ScriptValue> {
    let (b, ty, at, n) = read_range(vm, d, b, at, n, "s.read(buf, at, n)")?;
    if !boundary(vm, d)? {
        return Ok(NIL);
    }
    let words = match on_gpu(d, b) {
        true => match gpu_read(d, b, at, n)? {
            Some(words) => words,
            None => return Ok(NIL),
        },
        false => d.device.read(b, at, n).to_vec(),
    };
    Ok(words_to_array(vm, ty, &words))
}

/// `s.read` without the wait: the buffer's elements as the work before it leaves them, which
/// `s.take` gives. The work recorded between the two runs meanwhile (on the GPU, queued behind
/// the read, so the device never waits for the program to send more).
fn op_fetch(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, at: ScriptValue, n: ScriptValue) -> Res<ScriptValue> {
    let what = "s.fetch(buf, at, n)";
    let (b, ty, at, n) = read_range(vm, d, b, at, n, what)?;
    if d.fetches.len() >= MAX_FETCHES {
        return Err(format!("{what}: {MAX_FETCHES} fetches not taken yet").into());
    }
    let fetch = match boundary(vm, d)? {
        false => Fetch::Dropped,
        true if on_gpu(d, b) => {
            let g = gpu_of(d, what)?;
            g.batch.push(GpuOp::Read { buf: b, offset: at as u64 * 4, bytes: n as u64 * 4 });
            Fetch::Gpu(g.send_later().map_err(|e| Fail::Fatal(gpu_fatal(e)))?, ty)
        }
        true => Fetch::Ready(d.device.read(b, at, n).to_vec(), ty),
    };
    let id = d.next_fetch;
    d.next_fetch += 1;
    d.fetches.insert(id, fetch);
    Ok(num(id as f64))
}

/// A fetch's elements (nil where an interrupting message dropped it): the work recorded since
/// goes to its device first, then this waits for the fetch's answer.
fn op_take(vm: &mut ScriptVm, d: &mut Driver, f: ScriptValue) -> Res<ScriptValue> {
    let what = "s.take(fetch)";
    let fetch = f.as_number().filter(|x| *x >= 0.0 && x.fract() == 0.0).and_then(|x| d.fetches.remove(&(x as u64)));
    let Some(fetch) = fetch else {
        return Err(format!("{what}: not a fetch of this session, or taken already").into());
    };
    let sent = boundary(vm, d)?;
    let (words, ty) = match fetch {
        Fetch::Dropped => return Ok(NIL),
        Fetch::Ready(words, ty) => (words, ty),
        Fetch::Gpu(rx, ty) => match {
            let t = Instant::now();
            let answer = rx.recv().unwrap_or(Err(GpuError::DeviceLost));
            waited(d, t);
            answer
        } {
            Ok(mut reads) => (words_of(&reads.pop().unwrap_or_default()), ty),
            Err(GpuError::Stopped) if !d.shared.cancelled() => return Ok(NIL),
            Err(e) => return Err(Fail::Fatal(gpu_fatal(e))),
        },
    };
    // an interrupt since the fetch: as a read, nil until the program takes its inbox
    if !sent {
        return Ok(NIL);
    }
    Ok(words_to_array(vm, ty, &words))
}

fn op_flush(vm: &mut ScriptVm, d: &mut Driver) -> Res<ScriptValue> {
    Ok(ScriptValue::from_bool(boundary(vm, d)?))
}

/// The inbox as `{rev, interrupted, msgs}`. `msgs` holds every key of `defaults` (the messages
/// the program reads: a key no message came for reads as its default, where a missing key
/// would be an error), then the messages that came, the newest per key.
fn inbox_value(vm: &mut ScriptVm, d: &mut Driver, defaults: ScriptValue) -> Res<ScriptValue> {
    let defaults = match defaults.as_object() {
        Some(o) => vm.heap().map_entries_ordered(o).into_iter().filter(|e| e.key.as_id().is_some()).map(|e| (e.key, e.value)).collect::<Vec<_>>(),
        None if defaults.is_nil() => Vec::new(),
        None => return Err("s.inbox(defaults): defaults are an object {key: value if no message came}".into()),
    };
    let (rev, msgs, interrupted) = {
        let mut inbox = lock(&d.shared.inbox);
        let interrupted = d.shared.interrupted.swap(false, Ordering::Relaxed);
        (inbox.rev, std::mem::take(&mut inbox.msgs), interrupted)
    };
    d.rev = rev;
    let m = vm.new_object();
    for (key, value) in defaults {
        vm.set_value_def(m, key, value);
    }
    for (key, value) in &msgs {
        let v = to_script(vm, value);
        vm.set_value_def(m, key_id(key), v);
    }
    Ok(object(vm, &[("rev", num(rev as f64)), ("msgs", m.into()), ("interrupted", ScriptValue::from_bool(interrupted))]))
}

fn op_inbox(vm: &mut ScriptVm, d: &mut Driver, defaults: ScriptValue) -> Res<ScriptValue> {
    // the boundary collects garbage, and a literal `defaults` is referenced from nowhere else
    let _held = defaults.as_object().map(|o| vm.new_object_ref(o));
    boundary(vm, d)?;
    inbox_value(vm, d, defaults)
}

fn op_wait_inbox(vm: &mut ScriptVm, d: &mut Driver, defaults: ScriptValue) -> Res<ScriptValue> {
    let _held = defaults.as_object().map(|o| vm.new_object_ref(o));
    boundary(vm, d)?;
    let shared = d.shared.clone();
    {
        let mut inbox = lock(&shared.inbox);
        while inbox.rev == d.rev && !shared.cancelled() && !shared.interrupted() {
            inbox = shared.inbox_cv.wait(inbox).unwrap_or_else(|e| e.into_inner());
        }
    }
    if shared.cancelled() {
        return Err(Fail::Fatal(ProgramError::Cancelled));
    }
    inbox_value(vm, d, defaults)
}

fn version(d: &mut Driver, name: &str, state: bool) -> u64 {
    match d.versions.iter_mut().find(|(n, s, _)| n == name && *s == state) {
        Some(v) => {
            v.2 += 1;
            v.2
        }
        None => {
            d.versions.push((name.to_string(), state, 1));
            1
        }
    }
}

fn emit(vm: &mut ScriptVm, d: &mut Driver, name: String, b: usize, precision: Precision, state: bool) -> Res<ScriptValue> {
    if !boundary(vm, d)? {
        return Ok(ScriptValue::from_bool(false));
    }
    let ty = d.device.buffer(b).ty;
    let read;
    let words = match on_gpu(d, b) {
        true => {
            let n = d.device.buffer(b).len_words();
            match gpu_read(d, b, 0, n)? {
                Some(words) => {
                    read = words;
                    &read[..]
                }
                None => return Ok(ScriptValue::from_bool(false)),
            }
        }
        false => d.device.buffer(b).data.words(),
    };
    let data = match precision {
        Precision::Full => OutputData::Words(words.into()),
        Precision::F16 => OutputData::F16(words.iter().map(|w| f16_bits(f32::from_bits(*w))).collect()),
        Precision::U8 { lo, hi } => OutputData::U8 { lo, hi, levels: words.iter().map(|w| u8_level(f32::from_bits(*w), lo, hi)).collect() },
    };
    let output = Output { version: version(d, &name, state), name, state, rev: d.rev, ty, data };
    lock(&d.shared.outbox).output(output);
    d.shared.outbox_cv.notify_all();
    Ok(ScriptValue::from_bool(true))
}

fn op_publish(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, b: ScriptValue, opts: ScriptValue) -> Res<ScriptValue> {
    let what = "s.publish(name, buf, {precision: \"f32\" | \"f16\" | \"u8\", range: [lo, hi]})";
    let name = string(vm, name).ok_or_else(|| format!("{what}: the output's name"))?;
    let b = any_buf(vm, d, b, what)?;
    let o = opts.as_object();
    let precision = o.map(|o| field(vm, o, id!(precision))).and_then(|p| string(vm, p));
    let precision = match precision.as_deref() {
        None | Some("f32") => Precision::Full,
        Some(p @ ("f16" | "u8")) => {
            f32_buf(vm, d, b, what)?;
            if p == "f16" {
                Precision::F16
            } else {
                let range = o.map(|o| field(vm, o, id!(range))).and_then(|r| r.as_array()).filter(|r| vm.heap().array_len(*r) == 2);
                let range = range.and_then(|r| Some((vm.heap().array_index(r, 0, NoTrap).as_number()?, vm.heap().array_index(r, 1, NoTrap).as_number()?)));
                match range {
                    Some((lo, hi)) if hi > lo => Precision::U8 { lo: lo as f32, hi: hi as f32 },
                    _ => return Err(format!("{what}: u8 takes range: [lo, hi], lo < hi").into()),
                }
            }
        }
        Some(_) => return Err(format!("{what}: precision is \"f32\", \"f16\" or \"u8\"").into()),
    };
    emit(vm, d, name, b, precision, false)
}

/// A kept result's key: the program, the result's name and type, the content of the inputs it
/// was made from (by name) and what else it depends on (`with`, any value).
fn kept_key(vm: &ScriptVm, d: &mut Driver, program: u128, name: &str, ty: ElemTy, inputs: ScriptValue, with: ScriptValue, what: &str) -> Res<u128> {
    let mut key = program.to_le_bytes().to_vec();
    crate::wire::put_value(&mut key, &Value::String(format!("{name}:{}", ty.name())));
    let Value::Array(names) = from_script(vm, inputs)? else { return Err(format!("{what}: inputs are an array of input names").into()) };
    for n in &names {
        let Value::String(n) = n else { return Err(format!("{what}: inputs are an array of input names").into()) };
        let k = d.inputs.iter().position(|(i, _)| &i.name == n).ok_or_else(|| format!("{what}: no input `{n}`"))?;
        let hash = *d.input_hashes[k].get_or_insert_with(|| crate::wire::input_hash(&d.inputs[k].0.data));
        key.extend_from_slice(&hash.to_le_bytes());
    }
    crate::wire::put_value(&mut key, &from_script(vm, with)?);
    Ok(bytes_hash(&key))
}

/// Bytes' content hash: their words (the last one zero-padded) and their count.
fn bytes_hash(bytes: &[u8]) -> u128 {
    let mut words = vec![0u32; bytes.len().div_ceil(4) + 1];
    bytemuck::cast_slice_mut::<u32, u8>(&mut words)[..bytes.len()].copy_from_slice(bytes);
    *words.last_mut().unwrap() = bytes.len() as u32;
    crate::wire::input_hash(&words)
}

/// What an earlier session of this program kept as `s.remember(name, buf, inputs, with)` did,
/// from the same inputs and `with`: a read-only buffer of `ty`, nil when nothing is kept (or the
/// session has no keep).
fn op_recall(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, ty: ScriptValue, inputs: ScriptValue, with: ScriptValue) -> Res<ScriptValue> {
    let what = "s.recall(name, type, inputs, with)";
    let name = string(vm, name).ok_or_else(|| format!("{what}: the result's name"))?;
    let ty = elem_ty(vm, ty).ok_or_else(|| format!("{what}: the type is f32, i32, u32, f64, vec2, vec3 or vec4"))?;
    let Some((keep, program)) = d.keep.clone() else { return Ok(NIL) };
    let key = kept_key(vm, d, program, &name, ty, inputs, with, what)?;
    let Some(words) = keep.recall(key) else { return Ok(NIL) };
    let len = words.len() / ty.words();
    let (slot, gen) = d.device.alloc(ty, Data::Shared(words));
    Ok(handle(vm, slot, gen, ty, len))
}

/// Keeps a buffer's elements as they stand for later sessions (see `s.recall`): true when kept,
/// false when the session has no keep (or an interrupt dropped the work before).
fn op_remember(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, b: ScriptValue, inputs: ScriptValue, with: ScriptValue) -> Res<ScriptValue> {
    let what = "s.remember(name, buf, inputs, with)";
    let name = string(vm, name).ok_or_else(|| format!("{what}: the result's name"))?;
    let b = any_buf(vm, d, b, what)?;
    let Some((keep, program)) = d.keep.clone() else { return Ok(ScriptValue::from_bool(false)) };
    let ty = d.device.buffer(b).ty;
    let key = kept_key(vm, d, program, &name, ty, inputs, with, what)?;
    if !boundary(vm, d)? {
        return Ok(ScriptValue::from_bool(false));
    }
    let words: Arc<[u32]> = match on_gpu(d, b) {
        true => {
            let n = d.device.buffer(b).len_words();
            match gpu_read(d, b, 0, n)? {
                Some(words) => words.into(),
                None => return Ok(ScriptValue::from_bool(false)),
            }
        }
        false => d.device.buffer(b).data.words().into(),
    };
    keep.remember(&name, key, words);
    Ok(ScriptValue::from_bool(true))
}

fn op_state(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, b: ScriptValue) -> Res<ScriptValue> {
    let what = "s.state(name, buf)";
    let name = string(vm, name).ok_or_else(|| format!("{what}: the state's name"))?;
    let b = any_buf(vm, d, b, what)?;
    emit(vm, d, name, b, Precision::Full, true)
}

fn op_value(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, value: ScriptValue) -> Res<ScriptValue> {
    let name = string(vm, name).ok_or("s.value(name, value): the value's name")?;
    let value = from_script(vm, value)?;
    lock(&d.shared.outbox).value(name, value);
    d.shared.outbox_cv.notify_all();
    Ok(NIL)
}

fn op_progress(vm: &mut ScriptVm, d: &mut Driver, fraction: ScriptValue, text: ScriptValue) -> Res<ScriptValue> {
    let f = fraction.as_number().ok_or("s.progress(fraction, text): a number")?;
    let text = if text.is_nil() { String::new() } else { string(vm, text).ok_or("s.progress(fraction, text): text is a string")? };
    lock(&d.shared.outbox).updates.progress = Some((f.clamp(0.0, 1.0), text));
    d.shared.outbox_cv.notify_all();
    Ok(NIL)
}

fn op_device(vm: &mut ScriptVm, d: &mut Driver) -> Res<ScriptValue> {
    let backend = match d.device.backend() {
        makepad_script_compute::Backend::Native => "native",
        makepad_script_compute::Backend::Interp => "interp",
    };
    let kind = vm.new_string("cpu");
    let backend = vm.new_string(backend);
    let gpu = match &d.gpu {
        Some(g) => {
            let info = g.link.info().clone();
            let target = vm.new_string(match info.target {
                ComputeTarget::Spirv => "spirv",
                ComputeTarget::Msl => "msl",
                ComputeTarget::Hlsl => "hlsl",
            });
            object(vm, &[("target", target), ("buffer_bytes", num(info.budget_bytes as f64))])
        }
        None => NIL,
    };
    Ok(object(vm, &[
        ("kind", kind),
        ("backend", backend),
        ("threads", num(d.device.threads() as f64)),
        ("f64", ScriptValue::from_bool(true)),
        ("f16", ScriptValue::from_bool(false)),
        ("buffer_bytes", num(d.limits.buffer_bytes as f64)),
        ("gpu", gpu),
    ]))
}

fn op_stats(vm: &mut ScriptVm, d: &mut Driver) -> Res<ScriptValue> {
    let a = vm.heap_mut().new_array();
    for (label, s) in d.stats.clone() {
        let l = vm.new_string(&label);
        let o = object(vm, &[
            ("label", l),
            ("calls", num(s.calls as f64)),
            ("ms", num(s.nanos as f64 / 1e6)),
            ("work", num(s.work as f64)),
            ("elements", num(s.elements as f64)),
        ]);
        vm.array_push_trap(a, o);
    }
    let work = num(d.work as f64);
    Ok(object(vm, &[("work", work), ("kernels", a.into())]))
}
