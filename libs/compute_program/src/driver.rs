//! The driver: the program's isolate (its own VM and heap) on the session thread, and
//! `mod.compute`, the session the program's `run(s, p)` gets as `s`.
use crate::cpu::*;
use crate::session::*;
use crate::value::*;
use makepad_script::*;
use makepad_script_compute::kernel::Access;
use makepad_script_compute::boundary::KernelCompilation;
use makepad_script_compute::kernel::MathMode;
use makepad_script_compute::object::{prepare_object, CountPolicy};
use makepad_script_compute_sched::kernel::KernelError;
use std::sync::atomic::Ordering;
use std::sync::Arc;

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
    pending: Vec<Op>,
    kernels: Vec<KernelEntry>,
    work: u64,
    buffer_bytes: u64,
    /// The newest message rev the program took.
    rev: u64,
    versions: Vec<(String, bool, u64)>,
    fatal: Option<ProgramError>,
    stats: Vec<(String, Stat)>,
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
    let driver = Driver {
        shared,
        device: CpuDevice::new(config.executor.clone(), config.threads, config.backend, config.seed),
        limits,
        inputs: config.inputs.iter().cloned().map(|i| (i, None)).collect(),
        pending: Vec::new(),
        kernels: Vec::new(),
        work: 0,
        buffer_bytes: 0,
        rev: 0,
        versions: Vec::new(),
        fatal: None,
        stats: Vec::new(),
    };
    let mut host = ScriptVmHost::new(driver, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let result = run_program(&mut vm, program, &config);
    // The kernels' roots point into this VM's heap: they go before it does.
    state(&mut vm).kernels.clear();
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
    let doc = limited(vm, &limits, &mut fuel, |vm| vm.eval(ScriptMod::new("", "", &program.name, 0, 0, &program.source, &[])));
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
    // The end of `run` is a boundary.
    let d = state(vm);
    match boundary(vm, d) {
        Ok(_) => {}
        Err(Fail::Fatal(e)) => return Err(e),
        Err(Fail::Arg(m)) => return Err(ProgramError::Program(m)),
    }
    from_script(vm, result).map_err(ProgramError::Program)
}

/// Runs `f` under the session's stack, call and instruction limits, charging its instructions.
fn limited<R>(vm: &mut ScriptVm, limits: &Limits, fuel: &mut usize, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
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
    op!(vm, m, buffer, (ty, n), op_buffer);
    op!(vm, m, free, (buf), op_free);
    op!(vm, m, input, (name), op_input);
    op!(vm, m, fill, (buf, value), op_fill);
    op!(vm, m, copy, (src, dst), op_copy);
    op!(vm, m, write, (buf, values, at), op_write);
    op!(vm, m, run, (kernel, n, params, bind), op_run);
    op!(vm, m, repeat, (kernel, rounds, n, params, bind), op_repeat);
    op!(vm, m, sweep, (kernel, layers, n, dir, params, bind), op_sweep);
    op!(vm, m, reduce, (kernel, n, params, bind, block, slot), op_reduce);
    op!(vm, m, sum, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Sum, block, slot));
    op!(vm, m, min, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Min, block, slot));
    op!(vm, m, max, (a, block, slot), |vm, d, a, block, slot| op_fold(vm, d, a, NIL, Fold::Max, block, slot));
    op!(vm, m, dot, (a, b, block, slot), |vm, d, a, b, block, slot| op_fold(vm, d, a, b, Fold::Sum, block, slot));
    op!(vm, m, scan, (src, dst), op_scan);
    op!(vm, m, restrict, (fine, coarse, opts), op_restrict);
    op!(vm, m, prolong, (coarse, fine, opts), op_prolong);
    op!(vm, m, read, (buf, at, n), op_read);
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

/// A buffer handle's slot.
fn buf(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let o = v.as_object().ok_or_else(|| format!("{what}: a buffer (from s.buffer or s.input)"))?;
    let slot = vm.value(o, id!(compute_buffer)).as_number();
    let gen = vm.value(o, id!(gen)).as_number();
    match (slot, gen) {
        (Some(slot), Some(gen)) if d.device.get(slot as usize, gen as u32).is_some() => Ok(slot as usize),
        (Some(_), Some(_)) => Err(format!("{what}: the buffer was freed").into()),
        _ => Err(format!("{what}: a buffer (from s.buffer or s.input)").into()),
    }
}

/// A buffer the op writes: not a session input.
fn written(vm: &ScriptVm, d: &Driver, v: ScriptValue, what: &str) -> Res<usize> {
    let b = buf(vm, d, v, what)?;
    if d.device.buffer(b).read_only() {
        return Err(format!("{what}: a session input is read-only").into());
    }
    Ok(b)
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

fn op_buffer(vm: &mut ScriptVm, d: &mut Driver, ty: ScriptValue, n: ScriptValue) -> Res<ScriptValue> {
    let ty = elem_ty(vm, ty).ok_or("s.buffer(type, n): the type is f32, i32, u32, f64, vec2, vec3 or vec4")?;
    let n = count(vm, n, "s.buffer(type, n)")?;
    if n == 0 {
        return Err("s.buffer(type, n): at least one element".into());
    }
    let words = n.saturating_mul(ty.words());
    let bytes = words as u64 * 4;
    let available = d.limits.buffer_bytes.saturating_sub(d.buffer_bytes);
    if words > i32::MAX as usize || bytes > available {
        // A value the program answers (a coarser level, fewer buffers), not an error.
        return Ok(object(vm, &[("refused", ScriptValue::from_bool(true)), ("wanted", num(bytes as f64)), ("available", num(available as f64))]));
    }
    d.buffer_bytes += bytes;
    let (slot, gen) = d.device.alloc(ty, Data::Owned(vec![0; words]));
    Ok(handle(vm, slot, gen, ty, n))
}

fn op_free(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue) -> Res<ScriptValue> {
    let b = written(vm, d, b, "s.free(buf)")?;
    d.buffer_bytes -= d.device.buffer(b).len_words() as u64 * 4;
    d.device.retire(b);
    record(vm, d, Op::Free { buf: b })?;
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
    let b = written(vm, d, b, "s.fill(buf, value)")?;
    let ty = d.device.buffer(b).ty;
    if ty == ElemTy::F64 {
        return Err("s.fill(buf, value): fill an f64 buffer with a kernel or s.write".into());
    }
    let x = value.as_number().ok_or("s.fill(buf, value): a number")?;
    record(vm, d, Op::Fill { buf: b, word: word(ty, x) })?;
    Ok(NIL)
}

fn op_copy(vm: &mut ScriptVm, d: &mut Driver, src: ScriptValue, dst: ScriptValue) -> Res<ScriptValue> {
    let src = buf(vm, d, src, "s.copy(src, dst)")?;
    let dst = written(vm, d, dst, "s.copy(src, dst)")?;
    if src == dst {
        return Err("s.copy(src, dst): two buffers".into());
    }
    if d.device.buffer(src).len_words() != d.device.buffer(dst).len_words() {
        return Err("s.copy(src, dst): buffers of the same size".into());
    }
    record(vm, d, Op::Copy { src, dst })?;
    Ok(NIL)
}

fn op_write(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, values: ScriptValue, at: ScriptValue) -> Res<ScriptValue> {
    let b = written(vm, d, b, "s.write(buf, values, at)")?;
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
    record(vm, d, Op::Write { buf: b, at, words })?;
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
    let mut prepared = prepare_object(vm, obj, &label, &["label", "math"], &[], &[], counts, math);
    let program = prepared.take_program().ok_or_else(|| compile_errors(&prepared))?;
    let mut compiled = program.compile(d.device.backend(), &[]);
    let compiled = compiled.take_kernel().ok_or_else(|| compile_errors(&compiled))?;
    let root = vm.new_object_ref(obj);
    let k = d.device.add_kernel(compiled);
    debug_assert_eq!(k, d.kernels.len());
    d.kernels.push(KernelEntry { obj, _root: root, label });
    Ok(k)
}

fn compile_errors(c: &KernelCompilation) -> String {
    (0..c.error_count()).map(|k| c.error(k).message.to_string()).collect::<Vec<_>>().join("\n")
}

/// A kernel run, checked against the kernel: every param it is given exists, every buffer it
/// declares is bound, written buffers are the session's own and bound once, and an
/// element-local kernel's written buffers hold every element (so how a call splits never
/// changes whether it runs).
fn make_run(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue, what: &str) -> Res<Run> {
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
        let slot = buf(vm, d, e.value, &format!("{what}: `{name}`"))?;
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
    Ok(Run { kernel: kernel_index, count, params: run_params, bind: run_bind, repeat: None })
}

fn lanes(d: &Driver, run: &Run) -> usize {
    d.device.kernel(run.kernel).reduce_parts().1
}

fn op_run(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let run = make_run(vm, d, k, n, params, bind, "s.run(kernel, n, params, bind)")?;
    if lanes(d, &run) > 0 {
        return Err("s.run: a reduce kernel runs with s.reduce(kernel, n, params, bind, block, slot)".into());
    }
    record(vm, d, Op::Run(run))?;
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
    record(vm, d, Op::Run(run))?;
    Ok(NIL)
}

fn op_repeat(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, rounds: ScriptValue, n: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.repeat(kernel, rounds, n, params, bind)";
    let run = make_run(vm, d, k, n, params, bind, what)?;
    repeated(vm, d, run, rounds, "round", false, what)
}

fn op_sweep(vm: &mut ScriptVm, d: &mut Driver, k: ScriptValue, layers: ScriptValue, n: ScriptValue, dir: ScriptValue, params: ScriptValue, bind: ScriptValue) -> Res<ScriptValue> {
    let what = "s.sweep(kernel, layers, n, \"up\" | \"down\", params, bind)";
    let down = match string(vm, dir).as_deref() {
        Some("up") => false,
        Some("down") => true,
        _ => return Err(format!("{what}: the direction is \"up\" or \"down\"").into()),
    };
    let run = make_run(vm, d, k, n, params, bind, what)?;
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
    let run = make_run(vm, d, k, n, params, bind, what)?;
    let lanes = lanes(d, &run);
    if lanes == 0 {
        return Err(format!("{what}: a reduce kernel (reduce_sum, reduce_min or reduce_max)").into());
    }
    let (block, slot) = block_slot(vm, d, block, slot, lanes, what)?;
    if run.bind.iter().any(|(kb, b)| *b == block && d.device.kernel(run.kernel).buffer_view(*kb).access != Access::Read) {
        return Err(format!("{what}: the block is not one of the kernel's written buffers").into());
    }
    record(vm, d, Op::Reduce { run, block, slot })?;
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
    record(vm, d, Op::Fold { a, b, fold, block, slot })?;
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
    record(vm, d, Op::Scan { src, dst })?;
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
    record(vm, d, Op::Restrict { fine, coarse, grid, kind })?;
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
    record(vm, d, Op::Prolong { coarse, fine, grid, kind })?;
    Ok(NIL)
}

// ---------------------------------------------------------------------------------------------
// Segments

fn record(vm: &mut ScriptVm, d: &mut Driver, op: Op) -> Res<()> {
    d.pending.push(op);
    if d.pending.len() >= SEGMENT_OPS {
        boundary(vm, d)?;
    }
    Ok(())
}

/// Runs the recorded segment. Ok(false): an interrupting message came, and the segment's ops not
/// yet run were dropped (its frees still run).
fn boundary(vm: &mut ScriptVm, d: &mut Driver) -> Res<bool> {
    let shared = d.shared.clone();
    let mut ops = std::mem::take(&mut d.pending).into_iter();
    let mut ran_all = true;
    while let Some(op) = ops.next() {
        if shared.cancelled() {
            return Err(Fail::Fatal(ProgramError::Cancelled));
        }
        if shared.interrupted() {
            ran_all = false;
            for op in ops.by_ref() {
                if let Op::Free { .. } = op {
                    let _ = d.device.execute(op, 0, &shared.running);
                }
            }
            break;
        }
        let label = match &op {
            Op::Run(run) | Op::Reduce { run, .. } => d.kernels[run.kernel].label.clone(),
            op => op.label().to_string(),
        };
        let work_left = d.limits.work.saturating_sub(d.work);
        match d.device.execute(op, work_left, &shared.running) {
            Ok(done) => {
                d.work += done.work;
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
            Err(OpError::Kernel(KernelError::Cancelled)) if !shared.cancelled() => {
                ran_all = false;
                for op in ops.by_ref() {
                    if let Op::Free { .. } = op {
                        let _ = d.device.execute(op, 0, &shared.running);
                    }
                }
                break;
            }
            Err(OpError::Kernel(KernelError::Cancelled)) => return Err(Fail::Fatal(ProgramError::Cancelled)),
            Err(OpError::Kernel(KernelError::OverBudget { .. })) => return Err(Fail::Fatal(ProgramError::OverBudget("work"))),
            Err(OpError::Kernel(e)) => return Err(Fail::Fatal(ProgramError::Program(format!("{label}: {e}")))),
        }
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

fn op_read(vm: &mut ScriptVm, d: &mut Driver, b: ScriptValue, at: ScriptValue, n: ScriptValue) -> Res<ScriptValue> {
    let what = "s.read(buf, at, n)";
    let b = buf(vm, d, b, what)?;
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
    if !boundary(vm, d)? {
        return Ok(NIL);
    }
    let words = d.device.read(b, at * ty.words(), n * ty.words()).to_vec();
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
    boundary(vm, d)?;
    inbox_value(vm, d, defaults)
}

fn op_wait_inbox(vm: &mut ScriptVm, d: &mut Driver, defaults: ScriptValue) -> Res<ScriptValue> {
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
    let buffer = d.device.buffer(b);
    let ty = buffer.ty;
    let words = buffer.data.words();
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
    let b = buf(vm, d, b, what)?;
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

fn op_state(vm: &mut ScriptVm, d: &mut Driver, name: ScriptValue, b: ScriptValue) -> Res<ScriptValue> {
    let what = "s.state(name, buf)";
    let name = string(vm, name).ok_or_else(|| format!("{what}: the state's name"))?;
    let b = buf(vm, d, b, what)?;
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
    Ok(object(vm, &[
        ("kind", kind),
        ("backend", backend),
        ("threads", num(d.device.threads() as f64)),
        ("f64", ScriptValue::from_bool(true)),
        ("f16", ScriptValue::from_bool(false)),
        ("buffer_bytes", num(d.limits.buffer_bytes as f64)),
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
