//! The VM calls engine code makes most, as ScriptVm methods that take the heap and thread
//! borrows themselves: `vm.value(obj, id!(x))` instead of
//! `vm.bx.heap.value(obj, id!(x).into(), NoTrap)`. Engine code then reaches nothing inside the
//! VM's state but through methods, which is what lets the VM be a rustc-built part of the
//! makepad loader with Rapid-compiled apps (local/plans/loader-core.md section 8, step 6).
//! `*_trap` forms report failures at the current thread's trap (as
//! `vm.bx.threads.cur().trap.pass()` did); the plain forms report nothing (NoTrap).
//! `vm.heap()` / `vm.heap_mut()` cover the remaining heap calls.
use makepad_relax_api::relax_api;

use crate::heap::ScriptHeap;
use crate::object::*;
use crate::trap::*;
use crate::mod_pod::ScriptPodBuiltins;
use crate::thread::ScriptThreadId;
use crate::value::*;
use crate::vm::*;
use crate::vm_ctl::*;
use crate::vm_query::*;

/// Where a VM call writes text the caller keeps: `{ctx, write}` with a C-ABI `write`, made on
/// the caller's side ([`ScriptTextOut::new`] is inline) so the caller's own `String` grows; the
/// VM only calls `write` with views of its text. No owned string crosses (loader-core.md
/// section 8: the VM is rustc-built, its callers may be Rapid-built).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptTextOut<'a> {
    ctx: *mut u8,
    write: unsafe extern "C-unwind" fn(*mut u8, *const u8, usize),
    _out: std::marker::PhantomData<&'a mut String>,
}

impl<'a> ScriptTextOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut String) -> Self {
        unsafe extern "C-unwind" fn write(ctx: *mut u8, ptr: *const u8, len: usize) {
            let out = &mut *(ctx as *mut String);
            out.push_str(std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)));
        }
        unsafe { Self::from_callback(out as *mut String as *mut u8, write) }
    }

    /// # Safety
    /// The callback and context must remain valid for the adapter's lifetime.
    pub unsafe fn from_callback(ctx: *mut u8, write: unsafe extern "C-unwind" fn(*mut u8, *const u8, usize)) -> Self {
        Self { ctx, write, _out: std::marker::PhantomData }
    }

    /// A writer that drops what it is given, for a request that writes no text.
    #[inline(always)]
    pub fn discard() -> ScriptTextOut<'static> {
        unsafe extern "C-unwind" fn nowhere(_ctx: *mut u8, _ptr: *const u8, _len: usize) {}
        ScriptTextOut { ctx: std::ptr::null_mut(), write: nowhere, _out: std::marker::PhantomData }
    }

    /// Appends `s` to the caller's text.
    #[inline]
    pub fn push(&mut self, s: &str) {
        unsafe { (self.write)(self.ctx, s.as_ptr(), s.len()) }
    }
}

/// A source line and column (1-based as ScriptLoc's), an out-parameter of the `*_into` calls.
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptLineCol {
    pub line: u32,
    pub col: u32,
}

#[relax_api]
impl ScriptVm<'_> {
    #[inline(always)]
    pub fn set_type_getter_native(&mut self, ty: ScriptTypeRedux, f: crate::native::NativeGetterFn) {
        let mut req = ScriptRegReq::new(REG_TYPE_GETTER);
        req.type_redux = ty;
        req.native = f.into_raw();
        self.bx.vm_register(&req);
    }

    #[inline(always)]
    pub fn push_captured_error(&mut self, message: &str) {
        let mut req = ScriptErrorsReq::new(ERR_PUSH_CAPTURED);
        req.text = message;
        self.bx.vm_errors(&mut req);
    }
    /// A non-fatal script diagnostic raised by host code at the current script ip (a
    /// wrong-typed option, a verb naming an id that has no entity): the active capture gets it
    /// (once), the log once per site, and `makepad inspect <app> script errors` lists it per site
    /// with its count. Never aborts the evaluation. The text says what was wrong and what to write;
    /// the script location is added.
    #[inline(always)]
    pub fn push_diagnostic(&mut self, message: &str) {
        let mut req = ScriptErrorsReq::new(ERR_PUSH_DIAGNOSTIC);
        req.text = message;
        self.bx.vm_errors(&mut req);
    }
    /// Captures a message only if the active capture does not already contain it.
    #[inline(always)]
    pub fn push_captured_error_once(&mut self, message: &str) {
        let mut req = ScriptErrorsReq::new(ERR_PUSH_CAPTURED_ONCE);
        req.text = message;
        self.bx.vm_errors(&mut req);
    }
    #[inline(always)]
    pub fn set_object_op_hook(&mut self, hook: Option<ScriptObjectOpHook>) {
        unsafe extern "C-unwind" fn call(data: *const (), heap: *mut ScriptHeap, op: *const ScriptBinOp, a: ScriptValue, b: ScriptValue, ip: ScriptIp, out: *mut ScriptValue) -> bool {
            let hook: ScriptObjectOpHook = std::mem::transmute(data);
            if let Some(value) = hook(&mut *heap, *op, a, b, ip) { *out = value; true } else { false }
        }
        let hook = hook.map(|hook| ScriptObjectOpHookAbi { data: hook as *const (), call });
        let mut req = ScriptRegReq::new(REG_OBJECT_OP_HOOK);
        req.object_op_hook = hook.as_ref().map_or(std::ptr::null(), |hook| hook as *const ScriptObjectOpHookAbi);
        self.bx.vm_register(&req);
    }
    #[inline(always)]
    pub fn set_let_hook(&mut self, hook: Option<ScriptLetHook>) {
        unsafe extern "C-unwind" fn call(data: *const (), heap: *const ScriptHeap, ip: ScriptIp, id: crate::makepad_live_id::LiveId, value: ScriptValue) {
            let hook: ScriptLetHook = std::mem::transmute(data);
            hook(&*heap, ip, id, value);
        }
        let hook = hook.map(|hook| ScriptLetHookAbi { data: hook as *const (), call });
        let mut req = ScriptRegReq::new(REG_LET_HOOK);
        req.let_hook = hook.as_ref().map_or(std::ptr::null(), |hook| hook as *const ScriptLetHookAbi);
        self.bx.vm_register(&req);
    }

    /// Whether a let hook is set.
    #[inline(always)]
    pub fn has_let_hook(&self) -> bool {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_HAS_LET_HOOK)) != 0
    }
    #[inline(always)]
    pub fn set_keep_numbers(&mut self, keep: bool) { self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_SET_KEEP_NUMBERS, keep as u64)); }
    #[inline(always)]
    pub fn keep_numbers(&self) -> bool { self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_KEEP_NUMBERS)) != 0 }

    #[inline(always)]
    pub fn numeric_vec3(&mut self, value: crate::makepad_math::Vec3f) -> ScriptValue {
        let p = self.bx.parts_mut();
        crate::numeric::NumericValue::Vec3(value).to_script_value_heap(p.heap, p.code)
    }

    #[inline(always)]
    pub fn numeric_vec2(&mut self, value: crate::makepad_math::Vec2f) -> ScriptValue {
        let p = self.bx.parts_mut();
        crate::numeric::NumericValue::Vec2(value).to_script_value_heap(p.heap, p.code)
    }

    #[inline(always)]
    pub fn numeric_vec4(&mut self, value: crate::makepad_math::Vec4f) -> ScriptValue {
        let p = self.bx.parts_mut();
        crate::numeric::NumericValue::Vec4(value).to_script_value_heap(p.heap, p.code)
    }

    #[inline(always)]
    pub fn numeric_color(&mut self, value: crate::makepad_math::Vec4f) -> ScriptValue {
        let p = self.bx.parts_mut();
        crate::numeric::NumericValue::Color(value).to_script_value_heap(p.heap, p.code)
    }

    /// Captures errors into a fresh list until [`Self::capture_errors_restore`], keeping the
    /// capture that was active (none or a list) to put back.
    #[inline(always)]
    pub fn capture_errors_nested(&mut self) {
        self.bx.vm_errors(&mut ScriptErrorsReq::new(ERR_CAPTURE_NESTED));
    }

    /// Ends a [`Self::capture_errors_nested`]: its errors are dropped, the previous capture is back.
    #[inline(always)]
    pub fn capture_errors_restore(&mut self) {
        self.bx.vm_errors(&mut ScriptErrorsReq::new(ERR_CAPTURE_RESTORE));
    }

    #[inline(always)]
    pub fn register_crate_manifest(&mut self, name: &str, dir: &str) {
        let mut req = ScriptRegReq::new(REG_CRATE_MANIFEST);
        req.text = name;
        req.text2 = dir;
        self.bx.vm_register(&req);
    }

    /// `obj.key` (prototype chain included); nil when absent.
    #[inline(always)]
    pub fn value(&self, obj: ScriptObject, key: impl Into<ScriptValue>) -> ScriptValue {
        self.value_of(obj, key.into())
    }

    /// [`Self::value`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn value_of(&self, obj: ScriptObject, key: ScriptValue) -> ScriptValue {
        self.heap().value(obj, key, NoTrap)
    }

    /// [`Self::value`], reporting a failure at the current thread's trap.
    #[inline(always)]
    pub fn value_trap(&self, obj: ScriptObject, key: impl Into<ScriptValue>) -> ScriptValue {
        self.value_trap_of(obj, key.into())
    }

    /// [`Self::value_trap`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn value_trap_of(&self, obj: ScriptObject, key: ScriptValue) -> ScriptValue {
        let p = self.bx.parts();
        p.heap.value(obj, key, p.trap.pass())
    }

    /// Sets `obj.key = value`.
    #[inline(always)]
    pub fn set_value(&mut self, obj: ScriptObject, key: impl Into<ScriptValue>, value: impl Into<ScriptValue>) -> ScriptValue {
        self.set_value_of(obj, key.into(), value.into())
    }

    /// [`Self::set_value`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn set_value_of(&mut self, obj: ScriptObject, key: ScriptValue, value: ScriptValue) -> ScriptValue {
        self.heap_mut().set_value(obj, key, value, NoTrap)
    }

    /// [`Self::set_value`], reporting a failure at the current thread's trap.
    #[inline(always)]
    pub fn set_value_trap(&mut self, obj: ScriptObject, key: impl Into<ScriptValue>, value: impl Into<ScriptValue>) -> ScriptValue {
        self.set_value_trap_of(obj, key.into(), value.into())
    }

    /// [`Self::set_value_trap`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn set_value_trap_of(&mut self, obj: ScriptObject, key: ScriptValue, value: ScriptValue) -> ScriptValue {
        let p = self.bx.parts_mut();
        p.heap.set_value(obj, key, value, p.trap.pass())
    }

    /// Defines `obj.key = value` (a definition: no type checks, no errors).
    #[inline(always)]
    pub fn set_value_def(&mut self, obj: ScriptObject, key: impl Into<ScriptValue>, value: impl Into<ScriptValue>) {
        self.set_value_def_of(obj, key.into(), value.into())
    }

    /// [`Self::set_value_def`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn set_value_def_of(&mut self, obj: ScriptObject, key: ScriptValue, value: ScriptValue) {
        self.heap_mut().set_value_def(obj, key, value)
    }

    /// Element `index` of `obj`'s vector part; nil when out of range.
    #[inline(always)]
    pub fn vec_value(&self, obj: ScriptObject, index: usize) -> ScriptValue {
        self.heap().vec_value(obj, index, NoTrap)
    }

    /// [`Self::vec_value`], reporting a failure at the current thread's trap.
    #[inline(always)]
    pub fn vec_value_trap(&self, obj: ScriptObject, index: usize) -> ScriptValue {
        let p = self.bx.parts();
        p.heap.vec_value(obj, index, p.trap.pass())
    }

    /// Appends `value` under `key` (NIL: a plain element) to `obj`'s vector part.
    #[inline(always)]
    pub fn vec_push(&mut self, obj: ScriptObject, key: impl Into<ScriptValue>, value: impl Into<ScriptValue>) -> ScriptValue {
        self.vec_push_of(obj, key.into(), value.into())
    }

    /// [`Self::vec_push`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn vec_push_of(&mut self, obj: ScriptObject, key: ScriptValue, value: ScriptValue) -> ScriptValue {
        self.heap_mut().vec_push(obj, key, value, NoTrap)
    }

    /// [`Self::vec_push`], reporting a failure at the current thread's trap.
    #[inline(always)]
    pub fn vec_push_trap(&mut self, obj: ScriptObject, key: impl Into<ScriptValue>, value: impl Into<ScriptValue>) -> ScriptValue {
        self.vec_push_trap_of(obj, key.into(), value.into())
    }

    /// [`Self::vec_push_trap`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn vec_push_trap_of(&mut self, obj: ScriptObject, key: ScriptValue, value: ScriptValue) -> ScriptValue {
        let p = self.bx.parts_mut();
        p.heap.vec_push(obj, key, value, p.trap.pass())
    }

    /// Copies `source`'s fields into `target`, reporting at the current thread's trap.
    #[inline(always)]
    pub fn merge_object_trap(&mut self, target: ScriptObject, source: ScriptObject) -> ScriptValue {
        let p = self.bx.parts_mut();
        p.heap.merge_object(target, source, p.trap.pass())
    }

    /// The length of array `arr`.
    #[inline(always)]
    pub fn array_len(&self, arr: ScriptArray) -> usize {
        self.heap().array_len(arr)
    }

    /// Element `index` of array `arr`, reporting at the current thread's trap.
    #[inline(always)]
    pub fn array_index_trap(&self, arr: ScriptArray, index: usize) -> ScriptValue {
        let p = self.bx.parts();
        p.heap.array_index(arr, index, p.trap.pass())
    }

    /// Appends `value` to array `arr`, reporting at the current thread's trap.
    #[inline(always)]
    pub fn array_push_trap(&mut self, arr: ScriptArray, value: impl Into<ScriptValue>) {
        self.array_push_trap_of(arr, value.into())
    }

    /// [`Self::array_push_trap`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn array_push_trap_of(&mut self, arr: ScriptArray, value: ScriptValue) {
        let p = self.bx.parts_mut();
        p.heap.array_push(arr, value, p.trap.pass())
    }

    /// The host and the heap at once (a host call that reads the heap).
    #[inline(always)]
    pub fn host_heap(&mut self) -> (ScriptHostContext<'_>, &ScriptHeap) {
        (self.host.reborrow(), self.bx.heap_ref())
    }

    /// A method of `obj` by name (prototype chain included).
    #[inline(always)]
    pub fn object_method(&self, obj: ScriptObject, key: impl Into<ScriptValue>) -> ScriptValue {
        self.object_method_of(obj, key.into())
    }

    /// [`Self::object_method`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn object_method_of(&self, obj: ScriptObject, key: ScriptValue) -> ScriptValue {
        self.heap().object_method(obj, key, NoTrap)
    }

    /// A new empty object.
    #[inline(always)]
    pub fn new_object(&mut self) -> ScriptObject {
        self.heap_mut().new_object()
    }

    /// A new object with prototype `proto`.
    #[inline(always)]
    pub fn new_with_proto(&mut self, proto: impl Into<ScriptValue>) -> ScriptObject {
        self.new_with_proto_of(proto.into())
    }

    /// [`Self::new_with_proto`] with the key/value already a ScriptValue (the non-generic body: under
    /// `makepad run` it runs in the loader, loader-core.md section 8).
    #[inline(always)]
    pub fn new_with_proto_of(&mut self, proto: ScriptValue) -> ScriptObject {
        self.heap_mut().new_with_proto(proto)
    }

    /// A Rust-held reference to `obj` (a GC root while it lives).
    #[inline(always)]
    pub fn new_object_ref(&mut self, obj: ScriptObject) -> ScriptObjectRef {
        self.heap_mut().new_object_ref(obj)
    }

    /// A script string with `s`'s text.
    #[inline(always)]
    pub fn new_string(&mut self, s: &str) -> ScriptValue {
        self.heap_mut().new_string_from_str(s)
    }

    /// The root of the module tree (`mod`).
    #[inline(always)]
    pub fn modules(&self) -> ScriptObject {
        self.heap().modules_root()
    }

    /// This VM's heap identity ([`ScriptObjectRef::heap_key`] of every ref it makes).
    #[inline(always)]
    pub fn heap_key(&self) -> usize {
        self.heap().heap_key()
    }

    /// Is `value` a function?
    #[inline(always)]
    pub fn is_fn(&self, obj: ScriptObject) -> bool {
        self.heap().is_fn(obj)
    }

    // ---- threads

    /// Index of the current script thread.
    #[inline(always)]
    pub fn current_thread(&self) -> usize {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_CURRENT_THREAD)) as usize
    }

    /// Makes thread `index` current.
    #[inline(always)]
    pub fn set_current_thread(&mut self, index: usize) {
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_SET_CURRENT_THREAD, index as u64));
    }

    /// The current thread's id.
    #[inline(always)]
    pub fn thread_id(&self) -> ScriptThreadId {
        ScriptThreadId(self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_THREAD_ID)) as u32)
    }

    /// Is the current thread paused?
    #[inline(always)]
    pub fn is_paused(&self) -> bool {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_PAUSED)) != 0
    }

    /// Pauses the current thread (a native waiting on something); its id resumes it.
    #[inline(always)]
    pub fn pause(&mut self) -> ScriptThreadId {
        ScriptThreadId(self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_PAUSE)) as u32)
    }

    /// How many script threads exist.
    #[inline(always)]
    pub fn thread_count(&self) -> usize {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_THREAD_COUNT)) as usize
    }

    /// Thread `index`'s root footprint and paused state (a leak hunt's reading).
    #[inline(always)]
    pub fn thread_footprint(&self, index: usize) -> Option<([usize; 5], bool)> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, bytes: *const u8, _len: usize) {
            *(ctx as *mut [usize; 5]) = std::ptr::read_unaligned(bytes as *const [usize; 5]);
        }
        let mut footprint = [0usize; 5];
        let mut sink = ScriptQuerySink::new();
        sink.records = Some((&mut footprint as *mut [usize; 5] as *mut u8, push));
        let answer = self.bx.query(QUERY_THREAD_FOOTPRINT, &mut sink, |req| req.index = index);
        answer.found.then_some((footprint, answer.flag))
    }

    // ---- errors and budgets

    /// Collects the errors raised from now on (take_errors / take_error_records) instead of
    /// logging them.
    #[inline(always)]
    pub fn capture_errors(&mut self) {
        self.bx.vm_errors(&mut ScriptErrorsReq::new(ERR_CAPTURE));
    }

    /// Were errors captured since capture_errors (and not taken yet)?
    #[inline(always)]
    pub fn has_captured_errors(&self) -> bool {
        self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_HAS_CAPTURED_ERRORS)) != 0
    }

    /// Logs errors again (drops anything captured).
    #[inline(always)]
    pub fn stop_capturing_errors(&mut self) {
        self.bx.vm_ctl(&mut ScriptCtlReq::new(CTL_STOP_CAPTURING));
    }

    /// Sets whether errors are silenced; returns the previous setting.
    #[inline(always)]
    pub fn set_silence_errors(&mut self, silence: bool) -> bool {
        self.bx.vm_ctl(&mut ScriptCtlReq::with(CTL_SET_SILENCE_ERRORS, silence as u64)) != 0
    }

    /// The run budget (soft/hard deadlines) script runs are held to, if any.
    #[inline(always)]
    pub fn run_budget(&self) -> Option<ScriptRunBudget> {
        let mut req = ScriptCtlReq::new(STAT_RUN_BUDGET);
        if self.bx.vm_stat(&mut req) != 0 { Some(req.budget) } else { None }
    }

    /// Sets the run budget; returns the previous one.
    #[inline(always)]
    pub fn set_run_budget(&mut self, budget: Option<ScriptRunBudget>) -> Option<ScriptRunBudget> {
        let previous = self.run_budget();
        let mut req = ScriptCtlReq::new(CTL_SET_RUN_BUDGET);
        if let Some(budget) = budget {
            req.a = 1;
            req.budget = budget;
        }
        self.bx.vm_ctl(&mut req);
        previous
    }

    // ---- code

    /// The built-in pod types (vec2f, mat4f ...).
    #[inline(always)]
    pub fn pod_builtins(&self) -> &ScriptPodBuiltins {
        self.bx.parts().pod
    }

    /// Where instruction `ip` came from in the source: the file into `file`, line/column into
    /// `at`; false when unknown.
    #[inline(always)]
    pub fn ip_to_loc_into(&self, ip: ScriptIp, file: &mut ScriptTextOut, at: &mut ScriptLineCol) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        let answer = self.bx.code_ref().query(QUERY_IP_TO_LOC, &mut sink, |req| req.ip = ip);
        *at = answer.at;
        answer.found
    }

    /// Where instruction `ip` came from in the source.
    #[inline(always)]
    pub fn ip_to_loc(&self, ip: ScriptIp) -> Option<ScriptLoc> {
        let (mut file, mut at) = (String::new(), ScriptLineCol::default());
        self.ip_to_loc_into(ip, &mut ScriptTextOut::new(&mut file), &mut at)
            .then(|| ScriptLoc::new(&file, at.line, at.col))
    }

    /// The text of the script function whose code holds `ip` and where it is written:
    /// `source` false gives the fn from its `fn` keyword (or a closure) through its closing
    /// brace (ScriptCode::fn_text), true the source lines from its header line
    /// (ScriptCode::fn_source_text). False when there is none.
    #[inline(always)]
    pub fn fn_text_into(&self, ip: ScriptIp, source: bool, file: &mut ScriptTextOut, at: &mut ScriptLineCol, text: &mut ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(file);
        sink.text1(text);
        let answer = self.bx.code_ref().query(QUERY_FN_TEXT, &mut sink, |req| {
            req.ip = ip;
            req.flag = source;
        });
        *at = answer.at;
        answer.found
    }

    /// The script fn `f` as a [`ScriptFnImage`] another VM can adopt
    /// ([`ScriptVm::adopt_fn`]); None when `f` is no script fn.
    #[inline(always)]
    pub fn fn_image(&self, f: ScriptObject) -> Option<ScriptFnImage> {
        let image = self.fn_image_of(f);
        (!image.is_empty()).then_some(image)
    }

    /// [`Self::fn_image`] as the handle crosses to Relax-compiled code: an
    /// empty image when `f` is no script fn.
    #[relax_api]
    pub fn fn_image_of(&self, f: ScriptObject) -> ScriptFnImage {
        self.bx.heap.fn_script_ip(f).and_then(|ip| self.bx.code.fn_image(&self.bx.heap, ip)).unwrap_or_else(ScriptFnImage::empty)
    }

    /// The tokens of the script fn whose code holds `ip`, each with its
    /// document place (ScriptCode::fn_tokens_placed). None when there is none.
    // (an op: marked one by one in this impl, and not inline(always), which compiles on the caller's side)
    #[relax_api]
    pub fn fn_tokens_placed_raw(&self, ip: ScriptIp, out: &mut ScriptFnTokensRaw) -> bool {
        match self.bx.code.fn_tokens_placed(ip) {
            Some(t) => {
                out.file = makepad_relax_api::string_to_words(t.file);
                out.tokens = makepad_relax_api::vec_to_words(t.tokens);
                out.closure = t.closure;
                true
            }
            None => false,
        }
    }

    /// See [`Self::fn_text_into`] (`source` false).
    #[inline(always)]
    pub fn fn_text(&self, ip: ScriptIp) -> Option<(ScriptLoc, String)> {
        self.fn_text_with(ip, false)
    }

    /// See [`Self::fn_text_into`] (`source` true).
    #[inline(always)]
    pub fn fn_source_text(&self, ip: ScriptIp) -> Option<(ScriptLoc, String)> {
        self.fn_text_with(ip, true)
    }

    #[inline(always)]
    fn fn_text_with(&self, ip: ScriptIp, source: bool) -> Option<(ScriptLoc, String)> {
        let (mut file, mut at, mut text) = (String::new(), ScriptLineCol::default(), String::new());
        let ok = self.fn_text_into(ip, source, &mut ScriptTextOut::new(&mut file), &mut at, &mut ScriptTextOut::new(&mut text));
        ok.then(|| (ScriptLoc::new(&file, at.line, at.col), text))
    }

    /// The number of script bodies (modules, evals) loaded.
    #[inline(always)]
    pub fn body_count(&self) -> usize {
        self.bx.body_count()
    }

    /// The body whose script module has `module_path`.
    #[inline(always)]
    pub fn body_by_module_path(&self, module_path: &str) -> Option<u16> {
        let answer = self.bx.code_ref().query(QUERY_BODY_BY_MODULE_PATH, &mut ScriptQuerySink::new(), |req| req.text = module_path);
        answer.found.then_some(answer.index as u16)
    }

    /// The objects every loaded body holds alive: its scope, its `me` and
    /// the scope it ended in. A leak hunt counts what these reach.
    #[inline(always)]
    pub fn body_root_ids(&self) -> Vec<ScriptObject> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, bytes: *const u8, _len: usize) {
            (&mut *(ctx as *mut Vec<ScriptObject>)).push(std::ptr::read_unaligned(bytes as *const ScriptObject));
        }
        let mut out = Vec::new();
        let mut sink = ScriptQuerySink::new();
        sink.records = Some((&mut out as *mut Vec<ScriptObject> as *mut u8, push));
        self.bx.code_ref().query(QUERY_BODY_ROOT_IDS, &mut sink, |_| {});
        out
    }

    /// The scope body `body` ended in once it ran (else its own scope).
    #[inline(always)]
    pub fn body_end_scope(&self, body: u16) -> Option<ScriptObject> {
        let answer = self.bx.code_ref().query(QUERY_BODY_END_SCOPE, &mut ScriptQuerySink::new(), |req| req.index = body as usize);
        if answer.found { answer.value.as_object() } else { None }
    }

    /// The first module body from `file`, optionally at a particular source column.
    #[inline(always)]
    pub fn body_by_file(&self, file: &str, column: Option<usize>) -> Option<u16> {
        let answer = self.bx.code_ref().query(QUERY_BODY_BY_FILE, &mut ScriptQuerySink::new(), |req| {
            req.text = file;
            req.flag = column.is_some();
            req.index = column.unwrap_or(0);
        });
        answer.found.then_some(answer.index as u16)
    }

    /// Retains a body's initial scope, preserving whether it is a GC root.
    #[inline(always)]
    pub fn body_initial_scope(&self, body: u16) -> Option<ScriptObjectRef> {
        let answer = self.bx.code_ref().query(QUERY_BODY_INITIAL_SCOPE, &mut ScriptQuerySink::new(), |req| req.index = body as usize);
        let obj = if answer.found { answer.value.as_object()? } else { return None };
        if !answer.flag {
            return Some(ScriptObjectRef::unrooted(obj));
        }
        let roots = self.heap().heap_key() as *const crate::roots::ScriptRoots;
        crate::roots::script_root_retain(roots, crate::roots::ROOT_OBJECT, obj.index);
        Some(ScriptObjectRef { roots, obj })
    }

    /// Replaces a body's initial scope with a previously retained scope.
    #[inline(always)]
    pub fn set_body_initial_scope(&mut self, body: u16, scope: ScriptObjectRef) {
        let mut req = ScriptBodyReq::new(BODY_SET_SCOPE, body);
        req.scope = scope.as_object();
        req.rooted = !scope.is_unrooted();
        self.bx.vm_body(&mut req);
    }

    /// Body `body`'s scope objects (scope, me, end scope): what keeps it alive.
    #[inline(always)]
    pub fn body_scopes(&self, body: u16) -> Option<[Option<ScriptObject>; 3]> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, bytes: *const u8, _len: usize) {
            (&mut *(ctx as *mut Vec<ScriptObject>)).push(std::ptr::read_unaligned(bytes as *const ScriptObject));
        }
        let mut objects: Vec<ScriptObject> = Vec::new();
        let mut sink = ScriptQuerySink::new();
        sink.records = Some((&mut objects as *mut Vec<ScriptObject> as *mut u8, push));
        let answer = self.bx.code_ref().query(QUERY_BODY_SCOPES, &mut sink, |req| req.index = body as usize);
        if !answer.found {
            return None;
        }
        Some([Some(objects[0]), Some(objects[1]), (objects[2] != ScriptObject::ZERO).then_some(objects[2])])
    }

    /// The value of the global `key` the VM injects into every scope.
    #[inline(always)]
    pub fn injected_global(&self, key: crate::makepad_live_id::LiveId) -> Option<ScriptValue> {
        let answer = self.bx.query(QUERY_INJECTED_GLOBAL, &mut ScriptQuerySink::new(), |req| req.id = key);
        answer.found.then_some(answer.value)
    }

    /// Prints body `body`'s opcodes (debugging).
    pub fn dump_body_opcodes(&self, body: u16) {
        if let Some(b) = self.bx.code.bodies.borrow().get(body as usize) {
            b.parser.dump_opcodes();
        }
    }

    /// Where the current thread is (its body and instruction).
    #[inline(always)]
    pub fn trap_ip(&self) -> ScriptIp {
        let ip = self.bx.vm_stat(&mut ScriptCtlReq::new(STAT_TRAP_IP));
        ScriptIp { body: (ip >> 32) as u16, index: ip as u32 }
    }

    /// Body `body`'s script module: its cargo manifest dir and module path. False when the body
    /// is no `script_mod!` module.
    #[inline(always)]
    pub fn body_module_into(&self, body: u16, manifest: &mut ScriptTextOut, module_path: &mut ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(manifest);
        sink.text1(module_path);
        self.bx.code_ref().query(QUERY_BODY_MODULE, &mut sink, |req| req.index = body as usize).found
    }

    /// [`Self::body_module_into`] as strings: (cargo manifest dir, module path).
    #[inline(always)]
    pub fn body_module(&self, body: u16) -> Option<(String, String)> {
        let (mut manifest, mut module_path) = (String::new(), String::new());
        if self.body_module_into(body, &mut ScriptTextOut::new(&mut manifest), &mut ScriptTextOut::new(&mut module_path)) {
            Some((manifest, module_path))
        } else {
            None
        }
    }

    /// The manifest dir registered for crate `name` (a script module of it was loaded).
    #[inline(always)]
    pub fn crate_manifest_into(&self, name: &str, out: &mut ScriptTextOut) -> bool {
        let mut sink = ScriptQuerySink::new();
        sink.text0(out);
        self.bx.code_ref().query(QUERY_CRATE_MANIFEST, &mut sink, |req| req.text = name).found
    }

    /// [`Self::crate_manifest_into`] as a string.
    #[inline(always)]
    pub fn crate_manifest(&self, name: &str) -> Option<String> {
        let mut out = String::new();
        self.crate_manifest_into(name, &mut ScriptTextOut::new(&mut out)).then_some(out)
    }

    /// Every registered crate manifest as `name\tdir\n` lines.
    #[inline(always)]
    pub fn crate_manifests_into(&self, out: &mut ScriptTextOut) {
        let mut sink = ScriptQuerySink::new();
        sink.text0(out);
        self.bx.code_ref().query(QUERY_CRATE_MANIFESTS, &mut sink, |_| {});
    }

    /// [`Self::crate_manifests_into`] as a map (crate name -> manifest dir).
    #[inline(always)]
    pub fn crate_manifests(&self) -> std::collections::HashMap<String, String> {
        let mut text = String::new();
        self.crate_manifests_into(&mut ScriptTextOut::new(&mut text));
        text.lines().filter_map(|l| l.split_once('\t')).map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    // ---- strings

    /// `value` as text (strings as they are, numbers formatted, ...).
    #[inline(always)]
    pub fn cast_to_string(&self, value: ScriptValue) -> String {
        let mut out = String::new();
        self.heap().cast_to_text(value, &mut ScriptTextOut::new(&mut out));
        out
    }

    /// [`Self::cast_to_string`] for text a native keeps: charged against the heap's allocation
    /// budget first (None: over budget).
    #[inline(always)]
    pub fn cast_to_owned_string(&mut self, value: ScriptValue, operation: &'static str) -> Option<String> {
        if !self.heap_mut().charge_owned_string(value, operation) {
            return None;
        }
        Some(self.cast_to_string(value))
    }
}

/// Shared live-reload overrides. The collection stays on the VM side; snapshots are
/// built by callbacks on the caller side, so no Rust collection layout crosses the boundary.
#[relax_api(handle, default)]
#[derive(Clone, Default)]
pub struct ScriptModOverrides(std::rc::Rc<std::cell::RefCell<std::collections::HashMap<ScriptModKey, String>>>);

/// What [`ScriptModOverrides::edit`] does.
pub const OVR_LEN: u32 = 0;
/// `key`: removed; the answer: whether it was there.
pub const OVR_REMOVE: u32 = 1;
/// `key`, `text`: the code to run instead of the module's own.
pub const OVR_INSERT: u32 = 2;
/// `other`: its overrides replace these.
pub const OVR_REPLACE_FROM: u32 = 3;
/// Calls `push(ctx, key, text, len)` for each override; the key and text are borrowed for the call.
pub const OVR_SNAPSHOT: u32 = 4;

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptModOverridesReq<'a> {
    pub op: u32,
    pub key: *const ScriptModKey,
    pub other: *const ScriptModOverrides,
    pub text: &'a str,
    pub ctx: *mut u8,
    pub push: unsafe extern "C-unwind" fn(*mut u8, *const ScriptModKey, *const u8, usize),
}

impl<'a> ScriptModOverridesReq<'a> {
    #[inline(always)]
    pub fn new(op: u32) -> Self {
        unsafe extern "C-unwind" fn nowhere(_ctx: *mut u8, _key: *const ScriptModKey, _text: *const u8, _len: usize) {}
        Self { op, key: std::ptr::null(), other: std::ptr::null(), text: "", ctx: std::ptr::null_mut(), push: nowhere }
    }
}

#[relax_api]
impl ScriptModOverrides {
    /// Reads or changes the overrides by `req.op` (OVR_*).
    #[relax_api]
    pub fn edit(&self, req: &ScriptModOverridesReq) -> u64 {
        // SAFETY: the request's pointers are the caller's borrows for this call
        match req.op {
            OVR_LEN => self.0.borrow().len() as u64,
            OVR_REMOVE => self.0.borrow_mut().remove(unsafe { &*req.key }).is_some() as u64,
            OVR_INSERT => {
                self.0.borrow_mut().insert(unsafe { &*req.key }.clone(), req.text.to_owned());
                0
            }
            OVR_REPLACE_FROM => {
                let values = unsafe { &*req.other }.0.borrow().clone();
                *self.0.borrow_mut() = values;
                0
            }
            _ => {
                for (key, code) in self.0.borrow().iter() {
                    unsafe { (req.push)(req.ctx, key, code.as_ptr(), code.len()) };
                }
                0
            }
        }
    }
}

impl ScriptModOverrides {
    #[inline(always)]
    pub fn len(&self) -> usize { self.edit(&ScriptModOverridesReq::new(OVR_LEN)) as usize }
    #[inline(always)]
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    #[inline(always)]
    pub fn remove(&self, key: &ScriptModKey) -> bool {
        let mut req = ScriptModOverridesReq::new(OVR_REMOVE);
        req.key = key;
        self.edit(&req) != 0
    }
    #[inline(always)]
    pub fn insert(&self, key: &ScriptModKey, code: &str) {
        let mut req = ScriptModOverridesReq::new(OVR_INSERT);
        req.key = key;
        req.text = code;
        self.edit(&req);
    }
    #[inline(always)]
    pub fn replace_from(&self, other: &Self) {
        let mut req = ScriptModOverridesReq::new(OVR_REPLACE_FROM);
        req.other = other;
        self.edit(&req);
    }

    pub fn snapshot(&self) -> std::collections::HashMap<ScriptModKey, String> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, key: *const ScriptModKey, text: *const u8, len: usize) {
            let out = &mut *(ctx as *mut std::collections::HashMap<ScriptModKey, String>);
            out.insert((&*key).clone(), std::str::from_utf8_unchecked(std::slice::from_raw_parts(text, len)).to_owned());
        }
        let mut out = std::collections::HashMap::new();
        let mut req = ScriptModOverridesReq::new(OVR_SNAPSHOT);
        // SAFETY: out lives until the synchronous callback walk returns.
        req.ctx = &mut out as *mut _ as *mut u8;
        req.push = push;
        self.edit(&req);
        out
    }

    pub fn replace(&self, values: std::collections::HashMap<ScriptModKey, String>) {
        let next = Self::default();
        for (key, code) in values { next.insert(&key, &code); }
        self.replace_from(&next);
    }
}

/// Shared manifest lookup, available even while the host has parked its VM.
#[relax_api(handle, default)]
#[derive(Clone, Default)]
pub struct ScriptCrateManifests(std::rc::Rc<std::cell::RefCell<std::collections::HashMap<String, String>>>);

impl ScriptCrateManifests {
    /// # Safety
    /// The callback's context must remain valid for this synchronous walk; text is
    /// borrowed only during each callback invocation.
    pub unsafe fn snapshot_with(&self, ctx: *mut u8, push: unsafe extern "C-unwind" fn(*mut u8, *const u8, usize, *const u8, usize)) {
        for (name, dir) in self.0.borrow().iter() { push(ctx, name.as_ptr(), name.len(), dir.as_ptr(), dir.len()); }
    }

    pub fn snapshot(&self) -> std::collections::HashMap<String, String> {
        unsafe extern "C-unwind" fn push(ctx: *mut u8, name: *const u8, name_len: usize, dir: *const u8, dir_len: usize) {
            let out = &mut *(ctx as *mut std::collections::HashMap<String, String>);
            let name = std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, name_len));
            let dir = std::str::from_utf8_unchecked(std::slice::from_raw_parts(dir, dir_len));
            out.insert(name.to_owned(), dir.to_owned());
        }
        let mut out = std::collections::HashMap::new();
        // SAFETY: out lives until the synchronous callback walk returns.
        unsafe { self.snapshot_with(&mut out as *mut _ as *mut u8, push); }
        out
    }

}

#[relax_api]
impl ScriptVmBase {
    #[relax_api]
    pub fn shared_crate_manifests(&self) -> ScriptCrateManifests { ScriptCrateManifests(self.code.crate_manifests.clone()) }
    #[relax_api]
    pub fn shared_mod_overrides(&self) -> ScriptModOverrides { ScriptModOverrides(self.code.script_mod_overrides.clone()) }
}

impl ScriptVm<'_> {
    /// The tokens of the script fn whose code holds `ip`, each with its document place
    /// (`ScriptCode::fn_tokens_placed`). None when there is none. Not an op: the owned String
    /// and Vec come across as words (`fn_tokens_placed_raw`) and are rebuilt by whichever side
    /// compiles this fn.
    #[inline(always)]
    pub fn fn_tokens_placed(&self, ip: ScriptIp) -> Option<ScriptFnTokens> {
        let mut raw = ScriptFnTokensRaw { file: [0; 3], tokens: [0; 3], closure: false };
        if !self.fn_tokens_placed_raw(ip, &mut raw) { return None; }
        // SAFETY: the words came from string_to_words / vec_to_words in the op just above.
        Some(unsafe {
            ScriptFnTokens {
                file: makepad_relax_api::string_from_words(raw.file),
                closure: raw.closure,
                tokens: makepad_relax_api::vec_from_words(raw.tokens),
            }
        })
    }
}
