//! The VM's state seen from outside, as a few ops with a mode word each. `ScriptVm`'s flag,
//! limit, thread, error and registration methods keep their names and are inline decoders
//! of these: JIT-compiled code calls only ops, and each of these is a door into state rustc
//! lays out (the threads, the traps, the capture lists, the native table).
//!
//! - [`ScriptVmBase::parts`] / [`ScriptVmBase::parts_mut`]: the heap, the code, the current thread
//!   and its trap as borrows (the heap mutable in `parts_mut`), so heap calls that report at the
//!   thread's trap are ordinary code over them;
//! - [`ScriptVmBase::vm_stat`] reads a number, [`ScriptVmBase::vm_ctl`] sets one or acts;
//! - [`ScriptVmBase::vm_errors`] captures, takes, drains and formats errors;
//! - [`ScriptVmBase::vm_register`] adds natives, handle types, globals and hooks.
//!
//! Natively each is `#[inline]` and the mode is a constant at every call, so a call costs what
//! the method it replaces did.
use makepad_relax_api::relax_api;

use crate::function::NativeId;
use crate::heap::*;
use crate::makepad_live_id::LiveId;
use crate::mod_pod::ScriptPodBuiltins;
use crate::native::*;
use crate::*;
use crate::thread::*;
use crate::parser::ScriptParser;
use crate::tokenizer::ScriptTokenizer;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::vm_api::ScriptTextOut;
use std::rc::Rc;

/// The VM's parts as borrows (see [`ScriptVmBase::parts`]).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptVmParts<'a> {
    pub heap: &'a ScriptHeap,
    pub code: &'a ScriptCode,
    pub thread: &'a ScriptThread,
    pub trap: &'a ScriptTrapInner,
    pub pod: &'a ScriptPodBuiltins,
}

/// [`ScriptVmParts`] with the heap mutable: a heap call that reports at the thread's trap
/// borrows both at once.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptVmPartsMut<'a> {
    pub heap: &'a mut ScriptHeap,
    pub code: &'a ScriptCode,
    pub thread: &'a ScriptThread,
    pub trap: &'a ScriptTrapInner,
    pub pod: &'a ScriptPodBuiltins,
}

// ---- vm_stat

pub const STAT_RELOAD: u32 = 0;
pub const STAT_KEEP_NUMBERS: u32 = 1;
pub const STAT_LAST_LIMIT_CONSUMED: u32 = 2;
pub const STAT_CURRENT_THREAD: u32 = 3;
pub const STAT_THREAD_COUNT: u32 = 4;
pub const STAT_THREAD_ID: u32 = 5;
pub const STAT_PAUSED: u32 = 6;
/// The current thread's instruction: `body << 32 | index`.
pub const STAT_TRAP_IP: u32 = 7;
/// The instructions the current thread may still run; `u64::MAX`: no limit.
pub const STAT_LIMIT_REMAINING: u32 = 8;
/// `a`: a handle type's id; the answer: the type.
pub const STAT_HANDLE_TYPE: u32 = 9;
pub const STAT_BODY_COUNT: u32 = 10;
/// 1 and `budget` set when there is a run budget.
pub const STAT_RUN_BUDGET: u32 = 11;
pub const STAT_HAS_CAPTURED_ERRORS: u32 = 12;
pub const STAT_HAS_LET_HOOK: u32 = 13;
/// Whether a capture is on (`take_errors` has a list to take).
pub const STAT_CAPTURING: u32 = 14;

// ---- vm_ctl

pub const CTL_GC: u32 = 0;
pub const CTL_GC_AND_COMPACT: u32 = 1;
pub const CTL_GC_WITH_STATUS: u32 = 2;
pub const CTL_GC_IF_NEEDED: u32 = 3;
/// `a`: the new flag; the answer: the previous one.
pub const CTL_SET_RELOAD: u32 = 4;
pub const CTL_SET_KEEP_NUMBERS: u32 = 5;
pub const CTL_SET_SILENCE_ERRORS: u32 = 6;
/// `a`: the limit to run under (already narrowed by the caller).
pub const CTL_LIMIT_SET: u32 = 7;
/// `a`: the remaining allowance before (`u64::MAX`: none), `b`: the limit that was applied.
pub const CTL_LIMIT_END: u32 = 8;
pub const CTL_SET_CURRENT_THREAD: u32 = 9;
pub const CTL_SELECT_FIRST_UNPAUSED_THREAD: u32 = 10;
/// Pauses the current thread; the answer: its id.
pub const CTL_PAUSE: u32 = 11;
/// `a` 1: set `budget`, 0: no budget.
pub const CTL_SET_RUN_BUDGET: u32 = 12;
/// `value`: a host-made transient, freed if the script kept nothing of it.
pub const CTL_RELEASE_TRANSIENT: u32 = 13;
/// `a`: the allocation limit to run under.
pub const CTL_ALLOC_BEGIN: u32 = 14;
/// The answer: the bytes charged; `b`: 1 when an allocation was refused.
pub const CTL_ALLOC_END: u32 = 15;
/// `name`: the module path of a `script_mod` about to register.
pub const CTL_CENSUS_BEGIN: u32 = 16;
pub const CTL_CENSUS_END: u32 = 17;
/// Logs errors again, dropping anything captured.
pub const CTL_STOP_CAPTURING: u32 = 18;
/// `value`: an object; its vector part moves into the VM's stash and `a`, `b` are the stash's
/// pointer and length.
pub const CTL_VEC_TAKE: u32 = 19;
/// `value`: the object [`CTL_VEC_TAKE`] took from; its vector part goes back.
pub const CTL_VEC_RESTORE: u32 = 20;
/// `value`: a heap string, kept alive until the unpin and written to `text`; the answer is
/// the pin's depth, `u64::MAX` when `value` is no heap string.
pub const CTL_STRING_PIN: u32 = 21;
/// `a`: the depth [`CTL_STRING_PIN`] answered (and any pin a panic left above it).
pub const CTL_STRING_UNPIN: u32 = 22;
/// Whether a host native may build a string now: under a strict allocation budget it is refused
/// and charged as unbounded.
pub const CTL_NATIVE_STRING_PREFLIGHT: u32 = 23;
/// `text`: a host native's text, stored (charged on its real length under a budget) into `value`.
pub const CTL_STORE_NATIVE_STRING: u32 = 24;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptCtlReq<'a> {
    pub op: u32,
    pub a: u64,
    pub b: u64,
    pub value: ScriptValue,
    pub text: &'a str,
    pub name: &'static str,
    pub budget: ScriptRunBudget,
}

impl<'a> ScriptCtlReq<'a> {
    #[inline(always)]
    pub fn new(op: u32) -> Self {
        Self {
            op,
            a: 0,
            b: 0,
            value: NIL,
            text: "",
            name: "",
            budget: ScriptRunBudget { soft_deadline: 0.0, hard_deadline: 0.0, sample_interval_instructions: 0, instructions_until_sample: 0 },
        }
    }
    #[inline(always)]
    pub fn with(op: u32, a: u64) -> Self {
        let mut req = Self::new(op);
        req.a = a;
        req
    }
}

// ---- vm_errors

/// `text`: pushed to the capture list, if there is one.
pub const ERR_PUSH_CAPTURED: u32 = 0;
/// [`ERR_PUSH_CAPTURED`] unless the capture already holds `text`.
pub const ERR_PUSH_CAPTURED_ONCE: u32 = 1;
/// `text`: a type-mismatch error queued at the current thread's trap.
pub const ERR_PUSH_TYPE_MISMATCH: u32 = 2;
/// Collects the errors raised from now on instead of logging them.
pub const ERR_CAPTURE: u32 = 3;
/// Like [`ERR_CAPTURE`] over a fresh list, keeping the capture that was active.
pub const ERR_CAPTURE_NESTED: u32 = 4;
/// Ends an [`ERR_CAPTURE_NESTED`]: its errors are dropped, the previous capture is back.
pub const ERR_CAPTURE_RESTORE: u32 = 5;
/// Drains the captured and the pending errors in order into `push`.
pub const ERR_TAKE: u32 = 6;
/// Reports (or captures) the errors still pending at the current thread's trap.
pub const ERR_DRAIN: u32 = 7;
/// `value`: written to `out` as an enum variant error describes it.
pub const ERR_FORMAT_ENUM_VARIANT: u32 = 8;
/// `value`: an object, written to `out` briefly (its proto chain and key properties).
pub const ERR_FORMAT_OBJECT: u32 = 9;

pub type ScriptErrorPush = unsafe extern "C-unwind" fn(*mut u8, *const ScriptErrorRecord);

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptErrorsReq<'a> {
    pub op: u32,
    pub text: &'a str,
    pub value: ScriptValue,
    pub out: ScriptTextOut<'a>,
    pub ctx: *mut u8,
    pub push: ScriptErrorPush,
}

unsafe extern "C-unwind" fn no_record(_ctx: *mut u8, _record: *const ScriptErrorRecord) {}

impl<'a> ScriptErrorsReq<'a> {
    #[inline(always)]
    pub fn new(op: u32) -> Self {
        Self { op, text: "", value: NIL, out: ScriptTextOut::discard(), ctx: std::ptr::null_mut(), push: no_record }
    }
}

// ---- vm_body

/// `body`, `text`: the file the body's messages name.
pub const BODY_SET_SHOWN: u32 = 0;
/// `body`, `scope`: brings every name of the object into the body's scope.
pub const BODY_USE_ALL: u32 = 1;
/// `body`, `uses`: the names the body defined at its top level; the answer is false when
/// the body has not run, else `value` is the exports object.
pub const BODY_EXPORTS: u32 = 2;
/// `module`: whether the code parses without error.
pub const BODY_VALIDATE: u32 = 3;
/// `body`, `scope`, `rooted`: the body's initial scope becomes `scope`, a GC root or not.
pub const BODY_SET_SCOPE: u32 = 4;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptBodyReq<'a> {
    pub op: u32,
    pub body: u16,
    pub scope: ScriptObject,
    pub uses: &'a [ScriptObject],
    pub text: &'a str,
    pub module: ScriptModView<'a>,
    pub rooted: bool,
    /// out
    pub value: ScriptValue,
}

impl<'a> ScriptBodyReq<'a> {
    #[inline(always)]
    pub fn new(op: u32, body: u16) -> Self {
        Self { op, body, scope: ScriptObject::ZERO, uses: &[], text: "", module: ScriptModView::empty(), rooted: false, value: NIL }
    }
}

// ---- vm_register

/// `module`, `id` (the method), `args`, `native`: a native method of a module.
pub const REG_METHOD: u32 = 0;
/// `handle_type`, `id`, `args`, `native`: a native method of a handle type.
pub const REG_HANDLE_METHOD: u32 = 1;
/// `handle_type`, `native`: the type's field read.
pub const REG_HANDLE_GETTER: u32 = 2;
/// `handle_type`, `native`: the type's field write.
pub const REG_HANDLE_SETTER: u32 = 3;
/// `handle_type`, `native`: the type's catch-all method.
pub const REG_HANDLE_CALL: u32 = 4;
/// `type_redux`, `native`: a value type's field read.
pub const REG_TYPE_GETTER: u32 = 5;
/// `native`: an apply transform; the answer: its [`NativeId`] index.
pub const REG_APPLY_TRANSFORM: u32 = 6;
/// `id`: a new handle type; the answer: its number.
pub const REG_HANDLE_TYPE: u32 = 7;
/// `id`, `value`: a global every body's scope sees.
pub const REG_INJECTED_GLOBAL: u32 = 8;
/// `hook`: the let hook (null: none).
pub const REG_LET_HOOK: u32 = 9;
/// `hook`: the object-op hook (null: none).
pub const REG_OBJECT_OP_HOOK: u32 = 10;
/// `text`, `text2`: a crate's name and its manifest dir.
pub const REG_CRATE_MANIFEST: u32 = 11;
/// `module` (the shader fn), `renames`, `input`: its namespace.
pub const REG_SHADER_FN_NAMESPACE: u32 = 12;

/// A native as `{data, call, drop}`: what the four native kinds share. Made by
/// `into_raw`, which gives up the native's ownership; the op rebuilds the kind it registers.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptNativeRaw {
    pub data: *mut u8,
    pub call: *const (),
    pub drop: unsafe extern "C-unwind" fn(*mut u8),
}

unsafe extern "C-unwind" fn no_drop(_data: *mut u8) {}

impl ScriptNativeRaw {
    #[inline(always)]
    pub fn none() -> Self {
        Self { data: std::ptr::null_mut(), call: std::ptr::null(), drop: no_drop }
    }
}

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptRegReq<'a> {
    pub kind: u32,
    pub module: ScriptObject,
    pub handle_type: ScriptHandleType,
    pub type_redux: ScriptTypeRedux,
    pub id: LiveId,
    pub value: ScriptValue,
    pub args: &'a [(LiveId, ScriptValue)],
    pub native: ScriptNativeRaw,
    pub text: &'a str,
    pub text2: &'a str,
    pub renames: &'a [ShaderFnRename],
    pub input: *const ShaderFnInput,
    pub let_hook: *const ScriptLetHookAbi,
    pub object_op_hook: *const ScriptObjectOpHookAbi,
}

impl<'a> ScriptRegReq<'a> {
    #[inline(always)]
    pub fn new(kind: u32) -> Self {
        Self {
            kind,
            module: ScriptObject::ZERO,
            handle_type: ScriptHandleType::default(),
            type_redux: ScriptTypeRedux::default(),
            id: LiveId(0),
            value: NIL,
            args: &[],
            native: ScriptNativeRaw::none(),
            text: "",
            text2: "",
            renames: &[],
            input: std::ptr::null(),
            let_hook: std::ptr::null(),
            object_op_hook: std::ptr::null(),
        }
    }
}

#[relax_api]
impl ScriptVmBase {
    /// The heap, the code, the current thread and its trap.
    #[inline]
    #[relax_api]
    pub fn parts(&self) -> ScriptVmParts<'_> {
        let thread = self.threads.cur_ref();
        ScriptVmParts { heap: &self.heap, code: &self.code, thread, trap: &thread.trap, pod: &self.code.builtins.pod }
    }

    /// [`Self::parts`] with the heap mutable.
    #[inline]
    #[relax_api]
    pub fn parts_mut(&mut self) -> ScriptVmPartsMut<'_> {
        let thread = self.threads.cur_ref();
        ScriptVmPartsMut { heap: &mut self.heap, code: &self.code, thread, trap: &thread.trap, pod: &self.code.builtins.pod }
    }

    /// Reads a number of the VM by `req.op` (STAT_*).
    #[inline]
    #[relax_api]
    pub fn vm_stat(&self, req: &mut ScriptCtlReq) -> u64 {
        match req.op {
            STAT_RELOAD => self.is_reload as u64,
            STAT_KEEP_NUMBERS => self.keep_numbers as u64,
            STAT_LAST_LIMIT_CONSUMED => self.last_limit_consumed as u64,
            STAT_CURRENT_THREAD => self.threads.current() as u64,
            STAT_THREAD_COUNT => self.threads.len() as u64,
            STAT_THREAD_ID => self.threads.cur_ref().thread_id().0 as u64,
            STAT_PAUSED => self.threads.cur_ref().is_paused() as u64,
            STAT_TRAP_IP => {
                let ip = self.threads.cur_ref().trap.ip;
                ((ip.body as u64) << 32) | ip.index as u64
            }
            STAT_LIMIT_REMAINING => self.threads.cur_ref().instruction_limit_remaining.map_or(u64::MAX, |n| n as u64),
            STAT_HANDLE_TYPE => self.code.native.borrow().handle_type.get(&LiveId(req.a)).unwrap().0 as u64,
            STAT_BODY_COUNT => self.code.bodies.borrow().len() as u64,
            STAT_RUN_BUDGET => match self.run_budget {
                Some(budget) => {
                    req.budget = budget;
                    1
                }
                None => 0,
            },
            STAT_HAS_CAPTURED_ERRORS => self.captured_errors.as_ref().is_some_and(|e| !e.is_empty()) as u64,
            STAT_HAS_LET_HOOK => self.let_hook.is_some() as u64,
            STAT_CAPTURING => self.captured_errors.is_some() as u64,
            op => panic!("vm_stat: unknown op {op}"),
        }
    }

    /// Sets a number of the VM or acts on it by `req.op` (CTL_*).
    #[inline]
    #[relax_api]
    pub fn vm_ctl(&mut self, req: &mut ScriptCtlReq) -> u64 {
        match req.op {
            CTL_GC => {
                self.gc_now();
                0
            }
            CTL_GC_AND_COMPACT => {
                self.gc_now();
                self.heap.shrink_to_fit();
                0
            }
            CTL_GC_WITH_STATUS => {
                self.heap.mark(&self.threads, &self.code);
                self.heap.sweep(true);
                0
            }
            CTL_GC_IF_NEEDED => {
                if self.heap.needs_gc() {
                    self.gc_now();
                }
                0
            }
            CTL_SET_RELOAD => std::mem::replace(&mut self.is_reload, req.a != 0) as u64,
            CTL_SET_KEEP_NUMBERS => {
                self.keep_numbers = req.a != 0;
                0
            }
            CTL_SET_SILENCE_ERRORS => std::mem::replace(&mut self.silence_errors, req.a != 0) as u64,
            CTL_LIMIT_SET => {
                self.threads.cur().instruction_limit_remaining = Some(req.a as usize);
                // If the limited code runs no script at all, exit_remaining is never written:
                // seed it so consumed reads 0 in that case.
                self.last_limit_exit_remaining = req.a as usize;
                0
            }
            CTL_LIMIT_END => {
                let (previous, applied) = (if req.a == u64::MAX { None } else { Some(req.a as usize) }, req.b as usize);
                // Record what the call actually charged: hosts running several calls against ONE
                // cumulative budget (a game tick where on_tick + timers + touch events share a
                // pool) read it back via last_limit_consumed and shrink the next call's limit
                // accordingly. A completed run wipes the thread's remaining to None (Return/Bail
                // in handle_trap_on), which stashes it in last_limit_exit_remaining.
                let remaining_now = self.threads.cur_ref().instruction_limit_remaining.unwrap_or(self.last_limit_exit_remaining);
                self.last_limit_consumed = applied.saturating_sub(remaining_now);
                if !self.threads.cur_ref().is_paused() {
                    self.threads.cur().instruction_limit_remaining = previous;
                }
                0
            }
            CTL_SET_CURRENT_THREAD => {
                self.threads.set_current(req.a as usize);
                0
            }
            CTL_SELECT_FIRST_UNPAUSED_THREAD => {
                self.threads.set_current_to_first_unpaused_thread();
                0
            }
            CTL_PAUSE => self.threads.cur().pause().0 as u64,
            CTL_SET_RUN_BUDGET => {
                self.run_budget = if req.a != 0 { Some(req.budget) } else { None };
                0
            }
            CTL_RELEASE_TRANSIENT => {
                self.heap.free_value_if_unreffed(req.value);
                0
            }
            CTL_ALLOC_BEGIN => {
                let previous = self.heap.begin_allocation_budget(req.a as usize);
                self.alloc_budgets.push(previous);
                0
            }
            CTL_ALLOC_END => {
                // Parser/native work can be the last action in the limited code, with no
                // following opcode for run_core's poll. Surface that pending refusal here too.
                if let Some(message) = self.heap.take_allocation_error() {
                    let _ = script_err_limit!(self.threads.cur_ref().trap, "{}", message);
                    self.drain_errors();
                }
                let previous = self.alloc_budgets.pop().flatten();
                let report = self.heap.end_allocation_budget(previous);
                req.b = report.exceeded as u64;
                report.allocated_bytes as u64
            }
            CTL_CENSUS_BEGIN => {
                let Some(mut census) = self.heap.census.take() else {
                    if let Some(note) = crate::census::CENSUS_OTHER_VM.get() {
                        note(req.name);
                    }
                    return 0;
                };
                census.begin(&self.heap, &self.code, req.name);
                self.heap.census = Some(census);
                0
            }
            CTL_CENSUS_END => {
                let Some(mut census) = self.heap.census.take() else { return 0 };
                census.end(&self.heap, &self.code);
                self.heap.census = Some(census);
                0
            }
            CTL_STOP_CAPTURING => {
                self.captured_errors = None;
                0
            }
            CTL_VEC_TAKE => {
                let object = req.value.as_object().expect("vec_take of an object");
                let vec = std::mem::take(&mut self.heap.objects[object].vec);
                self.vec_stash.push((object, vec));
                let taken = &self.vec_stash.last().unwrap().1;
                req.a = taken.as_ptr() as u64;
                req.b = taken.len() as u64;
                0
            }
            CTL_VEC_RESTORE => {
                let object = req.value.as_object().expect("vec_restore of an object");
                if let Some(i) = self.vec_stash.iter().rposition(|(o, _)| *o == object) {
                    let (_, vec) = self.vec_stash.remove(i);
                    self.heap.objects[object].vec = vec;
                }
                0
            }
            CTL_STRING_PIN => {
                let Some(s) = req.value.as_string() else { return u64::MAX };
                let Some(rc) = self.heap.strings[s].as_ref().map(|s| s.string.clone()) else { return u64::MAX };
                let depth = self.string_pins.len();
                // SAFETY: the pinned Rc keeps the text alive until the unpin
                req.text = unsafe { std::mem::transmute::<&str, &str>(rc.0.as_str()) };
                self.string_pins.push(rc);
                depth as u64
            }
            CTL_STRING_UNPIN => {
                self.string_pins.truncate(req.a as usize);
                0
            }
            CTL_NATIVE_STRING_PREFLIGHT => {
                if self.heap.has_allocation_budget() && !self.heap.charge_native_strings_after {
                    let _ = self.heap.charge_allocation(usize::MAX, "building a native string without an allocation preflight");
                    return 0;
                }
                1
            }
            CTL_STORE_NATIVE_STRING => {
                let mut out = self.heap.strings_reuse.pop().unwrap_or_default();
                out.clear();
                out.push_str(req.text);
                req.value = self.heap.intern_or_store_string(out);
                0
            }
            op => panic!("vm_ctl: unknown op {op}"),
        }
    }

    #[inline(never)]
    fn gc_now(&mut self) {
        self.heap.mark(&self.threads, &self.code);
        self.heap.sweep(false);
    }

    /// Captures, takes, drains and formats errors by `req.op` (ERR_*).
    #[relax_api]
    pub fn vm_errors(&mut self, req: &mut ScriptErrorsReq) {
        match req.op {
            ERR_PUSH_CAPTURED => {
                if let Some(sink) = &mut self.captured_errors {
                    sink.push(ScriptErrorRecord::plain_str(req.text));
                }
            }
            ERR_PUSH_CAPTURED_ONCE => {
                if let Some(sink) = &mut self.captured_errors {
                    if !sink.iter().any(|error| error.text == req.text) {
                        sink.push(ScriptErrorRecord::plain_str(req.text));
                    }
                }
            }
            ERR_PUSH_TYPE_MISMATCH => {
                if let ScriptTrap::Inner(trap) = self.threads.cur().trap.pass() {
                    trap.push_err(ScriptValue::script_err_type_mismatch(trap.ip), req.text, file!(), line!());
                }
            }
            ERR_CAPTURE => self.captured_errors = Some(Vec::new()),
            ERR_CAPTURE_NESTED => {
                let prev = self.captured_errors.replace(Vec::new());
                self.captured_stash.push(prev);
            }
            ERR_CAPTURE_RESTORE => self.captured_errors = self.captured_stash.pop().flatten(),
            ERR_TAKE => {
                for record in self.captured_errors.take().unwrap_or_default() {
                    unsafe { (req.push)(req.ctx, &record) };
                }
                while let Some(err) = self.threads.cur().trap.err_pop_front() {
                    let record = self.format_error(&err);
                    unsafe { (req.push)(req.ctx, &record) };
                }
            }
            ERR_DRAIN => self.drain_errors(),
            ERR_FORMAT_ENUM_VARIANT => req.out.push(&crate::suggest::format_enum_variant_error(&self.heap, req.value)),
            ERR_FORMAT_OBJECT => {
                let mut text = String::new();
                let mut recur = Vec::new();
                // Use the heap's debug string but limit depth to keep it concise
                self.heap.to_debug_string(req.value, &mut recur, &mut text, false, 0);
                // Truncate if too long
                // count chars: a byte offset can land inside a multibyte char
                if text.chars().count() > 200 {
                    text = text.chars().take(197).collect();
                    text.push_str("...");
                }
                req.out.push(&text);
            }
            op => panic!("vm_errors: unknown op {op}"),
        }
    }

    /// Acts on the body table by `req.op` (BODY_*).
    #[relax_api]
    pub fn vm_body(&mut self, req: &mut ScriptBodyReq) -> bool {
        match req.op {
            BODY_SET_SHOWN => {
                self.set_body_shown(req.body, req.text);
                true
            }
            BODY_USE_ALL => {
                self.body_use_all(req.body, req.scope);
                true
            }
            BODY_EXPORTS => match self.scope_exports(req.body, req.uses) {
                Some(exports) => {
                    req.value = exports.into();
                    true
                }
                None => false,
            },
            BODY_SET_SCOPE => {
                let scope = if req.rooted { self.heap.new_object_ref(req.scope) } else { ScriptObjectRef::unrooted(req.scope) };
                self.code.bodies.borrow_mut()[req.body as usize].scope = scope;
                true
            }
            BODY_VALIDATE => {
                let m = &req.module;
                let mut tokenizer = ScriptTokenizer::default();
                let mut parser = ScriptParser::default();
                tokenizer.tokenize(m.code, &mut self.heap);
                tokenizer.finish(&mut self.heap);
                parser.parse(&tokenizer, m.file, (m.line, m.column), m.values);
                !parser.had_error
            }
            op => panic!("vm_body: unknown op {op}"),
        }
    }

    /// Registers natives, handle types, globals and hooks by `req.kind` (REG_*). The answer is
    /// the kind's: a number for the kinds that make one, else 0.
    #[relax_api]
    pub fn vm_register(&mut self, req: &ScriptRegReq) -> u64 {
        // SAFETY (all `from_raw`): the request's native was made by `into_raw` of the kind
        // registered and its ownership passes to the table.
        match req.kind {
            REG_METHOD => {
                let f = unsafe { NativeFn::from_raw(req.native) };
                self.code.native.borrow_mut().add_method_boxed(&mut self.heap, req.module, req.id, req.args, f);
                0
            }
            REG_HANDLE_METHOD => {
                let f = unsafe { NativeFn::from_raw(req.native) };
                self.code.native.borrow_mut().add_type_method_native(&mut self.heap, req.handle_type.to_redux(), req.id, req.args, f);
                0
            }
            REG_HANDLE_GETTER => {
                let f = unsafe { NativeGetterFn::from_raw(req.native) };
                self.code.native.borrow_mut().set_type_getter_native(req.handle_type.to_redux(), f);
                0
            }
            REG_HANDLE_SETTER => {
                let f = unsafe { NativeSetterFn::from_raw(req.native) };
                self.code.native.borrow_mut().set_type_setter_native(req.handle_type.to_redux(), f);
                0
            }
            REG_HANDLE_CALL => {
                let f = unsafe { NativeCallFn::from_raw(req.native) };
                self.code.native.borrow_mut().set_type_call_native(req.handle_type.to_redux(), f);
                0
            }
            REG_TYPE_GETTER => {
                let f = unsafe { NativeGetterFn::from_raw(req.native) };
                self.code.native.borrow_mut().set_type_getter_native(req.type_redux, f);
                0
            }
            REG_APPLY_TRANSFORM => {
                let f = unsafe { NativeFn::from_raw(req.native) };
                let id: NativeId = self.code.native.borrow_mut().add_apply_transform_native(f);
                id.index as u64
            }
            REG_HANDLE_TYPE => {
                let ht = self.code.native.borrow_mut().new_handle_type(&mut self.heap, req.id);
                ht.0 as u64
            }
            REG_INJECTED_GLOBAL => {
                self.injected_globals.insert(req.id, req.value);
                self.apply_injected_globals_to_all_scopes();
                0
            }
            REG_LET_HOOK => {
                self.let_hook = unsafe { req.let_hook.as_ref() }.copied();
                0
            }
            REG_OBJECT_OP_HOOK => {
                self.object_op_hook = unsafe { req.object_op_hook.as_ref() }.copied();
                0
            }
            REG_CRATE_MANIFEST => {
                self.code.crate_manifests.borrow_mut().insert(req.text.to_owned(), req.text2.to_owned());
                0
            }
            REG_SHADER_FN_NAMESPACE => {
                let input = unsafe { req.input.as_ref() }.copied();
                let mut map = self.code.shader_fn_namespaces.borrow_mut();
                if req.renames.is_empty() && input.is_none() {
                    map.remove(&req.module);
                } else {
                    map.insert(req.module, Rc::new(ShaderFnNamespace { renames: req.renames.iter().map(|r| (r.from, r.to)).collect(), input }));
                }
                0
            }
            kind => panic!("vm_register: unknown kind {kind}"),
        }
    }
}
