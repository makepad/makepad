use crate::function::*;
use crate::heap::*;
use crate::makepad_error_log::*;
use crate::makepad_live_id::*;
use crate::mod_gc::*;
use crate::mod_html::*;
use crate::mod_math::*;
use crate::mod_pod::*;
use crate::mod_regex::*;
use crate::mod_shader::*;
use crate::mod_std::*;
use crate::native::*;
use crate::object::*;
use crate::opcode::*;
use crate::parser::*;
use crate::thread::*;
use crate::tokenizer::*;
use crate::trap::*;
use crate::value::*;
use crate::*;
use std::any::Any;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct ScriptModKey {
    pub file: String,
    pub line: usize,
    pub column: usize,
}

impl ScriptModKey {
    pub fn from_script_mod(script_mod: &ScriptMod) -> Self {
        Self {
            file: script_mod.file.clone(),
            line: script_mod.line,
            column: script_mod.column,
        }
    }
}

#[derive(Default, Debug)]
pub struct ScriptMod {
    pub cargo_manifest_path: String,
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub code: String,
    pub values: Vec<ScriptValue>,
}

pub enum ScriptSource {
    Mod(ScriptMod),
    Streaming { code: String },
}

pub struct ScriptBody {
    pub source: ScriptSource,
    pub effective_code: String,
    pub tokenizer: ScriptTokenizer,
    pub parser: ScriptParser,
    pub scope: ScriptObjectRef,
    pub me: ScriptObjectRef,
    /// The scope the body ended in, once it has run: `let`/`fn` that
    /// shadow a name open child scopes below `scope`.
    pub end_scope: Option<ScriptObjectRef>,
    pub checkpoint: Option<ParserCheckpoint>,
    pub source_len: usize,
}

#[derive(Default)]
pub struct ScriptBuiltins {
    pub range: ScriptObject,
    pub pod: ScriptPodBuiltins,
}

impl ScriptBuiltins {
    pub fn new(heap: &mut ScriptHeap, pod: ScriptPodBuiltins) -> Self {
        Self {
            range: heap
                .value_path(heap.modules, ids!(std.Range), NoTrap)
                .as_object()
                .unwrap(),
            pod,
        }
    }
}

/// The script bodies, behind the same `borrow()` / `borrow_mut()` surface a
/// `RefCell` has. Every mutable borrow advances `epoch`: a mutation is the only
/// thing that can move or free a body's opcode buffer (a native call can
/// re-enter `eval` and reload the very body that is running), and `run_core`
/// compares the epoch before it trusts its cached pointer into that buffer.
#[derive(Default)]
pub struct ScriptBodies {
    bodies: RefCell<Vec<ScriptBody>>,
    epoch: Cell<u64>,
}

impl ScriptBodies {
    #[inline(always)]
    pub fn borrow(&self) -> Ref<'_, Vec<ScriptBody>> {
        self.bodies.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, Vec<ScriptBody>> {
        self.epoch.set(self.epoch.get().wrapping_add(1));
        self.bodies.borrow_mut()
    }

    #[inline(always)]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.get()
    }
}

#[derive(Default)]
pub struct ScriptCode {
    pub builtins: ScriptBuiltins,
    pub native: RefCell<ScriptNative>,
    pub bodies: ScriptBodies,
    pub crate_manifests: Rc<RefCell<HashMap<String, String>>>,
    pub script_mod_overrides: Rc<RefCell<HashMap<ScriptModKey, String>>>,
}

pub struct ScriptLoc {
    pub file: String,
    pub col: u32,
    pub line: u32,
}

impl std::fmt::Debug for ScriptLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

impl std::fmt::Display for ScriptLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

impl ScriptCode {
    /// The source text of the fn whose body starts at `ip` — the `fn` token's
    /// line through the matching closing brace — plus where it lives. What
    /// the design tweaker shows under a material well: the pixel/vertex
    /// function as written, docs included, for a code-only rewrite.
    pub fn fn_source_text(&self, ip: ScriptIp) -> Option<(ScriptLoc, String)> {
        let loc = self.ip_to_loc(ip)?;
        let bodies = self.bodies.borrow();
        let body = bodies.get(ip.body as usize)?;
        let source_map = &body.parser.source_map;
        // Synthetic opcodes map to no token; take the nearest mapped one on
        // either side, as `ip_to_loc` does.
        let ip_index = (ip.index as usize).min(source_map.len().saturating_sub(1));
        let token_index = (0..=ip_index)
            .rev()
            .find_map(|i| source_map.get(i).and_then(|slot| *slot))
            .or_else(|| {
                ((ip_index + 1)..source_map.len()).find_map(|i| source_map.get(i).and_then(|slot| *slot))
            })?;
        let (row, _col) = body.tokenizer.token_index_to_row_col(token_index)?;
        let code = &body.effective_code;
        let lines: Vec<&str> = code.split_inclusive('\n').collect();
        // The ip maps to a token INSIDE the fn; walk back to the header line
        // (the nearest line above holding `fn`), then take from there to the
        // matching closing brace.
        let mut header = (row as usize).min(lines.len().saturating_sub(1));
        while header > 0 && !lines[header].contains("fn") {
            header -= 1;
        }
        let start: usize = lines[..header].iter().map(|l| l.len()).sum();
        let rest = &code[start.min(code.len())..];
        // From the first `{` after the fn header to its matching `}`.
        let open = rest.find('{')?;
        let mut depth = 0i32;
        let mut end = None;
        for (i, ch) in rest[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end?;
        // Include the header line (e.g. `pixel: fn() {`) from its own start.
        let line_start = rest[..open].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let text = rest[line_start..end].to_string();
        Some((loc, text))
    }

    pub fn ip_to_loc(&self, ip: ScriptIp) -> Option<ScriptLoc> {
        if let Some(body) = self.bodies.borrow().get(ip.body as usize) {
            let source_map = &body.parser.source_map;
            let ip_index = ip.index as usize;

            let direct_token = source_map.get(ip_index).and_then(|slot| *slot);
            // Some opcodes are synthetic and have `None` in source_map.
            // For error reporting, fall back to the nearest mapped token so we still
            // surface a real file/line instead of "unknown".
            let nearest_token = if direct_token.is_some() {
                direct_token
            } else {
                let left = ip_index.min(source_map.len().saturating_sub(1));
                let left_token = (0..=left)
                    .rev()
                    .find_map(|idx| source_map.get(idx).and_then(|slot| *slot));
                if left_token.is_some() {
                    left_token
                } else {
                    ((ip_index + 1)..source_map.len())
                        .find_map(|idx| source_map.get(idx).and_then(|slot| *slot))
                }
            };

            if let Some(token_index) = nearest_token {
                if let Some(rc) = body.tokenizer.token_index_to_row_col(token_index) {
                    if let ScriptSource::Mod(script_mod) = &body.source {
                        return Some(ScriptLoc {
                            file: script_mod.file.clone(),
                            line: rc.0 + script_mod.line as u32,
                            col: rc.1,
                        });
                    }
                    return Some(ScriptLoc {
                        file: "generated".into(),
                        line: rc.0,
                        col: rc.1,
                    });
                }
            }
        }
        return Some(ScriptLoc {
            file: "unknown".into(),
            line: ip.body as _,
            col: ip.index as _,
        });
    }
}

pub trait ScriptHost: Any {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn script_std(&mut self) -> &mut dyn Any;
    fn script_vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>>;
}

/// Small host container for standalone VMs that do not have an application
/// host type of their own. Hosts without a standard library use `()` for
/// `std`.
pub struct ScriptVmHost<H: Any = (), S: Any = ()> {
    pub host: H,
    pub std: S,
    pub script_vm: Option<Box<ScriptVmBase>>,
}

impl<H: Any, S: Any> ScriptVmHost<H, S> {
    pub fn new(host: H, std: S) -> Self {
        Self {
            host,
            std,
            script_vm: None,
        }
    }
}

impl<H: Any, S: Any> ScriptHost for ScriptVmHost<H, S> {
    fn as_any(&self) -> &dyn Any {
        &self.host
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        &mut self.host
    }

    fn script_std(&mut self) -> &mut dyn Any {
        &mut self.std
    }

    fn script_vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>> {
        &mut self.script_vm
    }
}

pub struct ScriptVm<'a> {
    pub host: &'a mut dyn ScriptHost,
    pub bx: Box<ScriptVmBase>,
}

#[derive(Clone, Copy, Debug)]
pub struct ScriptRunBudget {
    pub soft_deadline: f64,
    pub hard_deadline: f64,
    pub sample_interval_instructions: u32,
    pub instructions_until_sample: u32,
}

impl ScriptRunBudget {
    pub fn from_durations(soft: Duration, hard: Duration, sample_interval_instructions: u32) -> Self {
        let now = crate::clock::monotonic_now();
        let sample_interval_instructions = sample_interval_instructions.max(1);
        Self {
            soft_deadline: now + soft.as_secs_f64(),
            hard_deadline: now + hard.as_secs_f64(),
            sample_interval_instructions,
            instructions_until_sample: sample_interval_instructions,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScriptRunBudgetHit {
    Soft,
    Hard,
}

impl<'a> ScriptVm<'a> {
    /// Bail out of the interpreter with a script error.
    /// Use this when a stack (mes, scopes, loops, calls) is unexpectedly empty,
    /// indicating corrupted bytecode (e.g. from incomplete streaming input).
    /// Sets trap.on to Return(err) so run_core exits cleanly.
    pub(crate) fn bail(&mut self, msg: &str) {
        let err = script_err_unexpected!(self.bx.threads.cur_ref().trap, "{}", msg);
        self.bx
            .threads
            .cur()
            .trap
            .set_on(Some(ScriptTrapOn::Bail(err)));
    }

    pub fn with_instruction_limit<R>(
        &mut self,
        instruction_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_remaining = self.bx.threads.cur_ref().instruction_limit_remaining;
        let applied = previous_remaining
            .map(|remaining| remaining.min(instruction_limit))
            .unwrap_or(instruction_limit);
        self.bx.threads.cur().instruction_limit_remaining = Some(applied);
        // If f() runs no script at all, exit_remaining is never written —
        // seed it so consumed reads 0 in that case.
        self.bx.last_limit_exit_remaining = applied;
        let result = f(self);
        // Record what this call actually charged: hosts running several
        // calls against ONE cumulative budget (e.g. a game tick where
        // on_tick + timers + touch events share a pool) read it back via
        // last_limit_consumed and shrink the next call's limit accordingly.
        // A completed run wipes the thread's remaining to None (Return/Bail
        // in handle_trap_on), which stashes it in last_limit_exit_remaining.
        let remaining_now = self
            .bx
            .threads
            .cur_ref()
            .instruction_limit_remaining
            .unwrap_or(self.bx.last_limit_exit_remaining);
        self.bx.last_limit_consumed = applied.saturating_sub(remaining_now);
        if !self.bx.threads.cur_ref().is_paused() {
            self.bx.threads.cur().instruction_limit_remaining = previous_remaining;
        }
        result
    }

    /// Runs an operation with a cap on live VM operand values.
    ///
    /// Nested caps can only narrow the current limit. A paused continuation
    /// retains the cap that began its evaluation, matching instruction-limit
    /// behavior.
    pub fn with_stack_value_limit<R>(
        &mut self,
        stack_value_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_limit = self.bx.threads.cur_ref().stack_limit;
        self.bx.threads.cur().stack_limit = previous_limit.min(stack_value_limit);
        let result = f(self);
        if !self.bx.threads.cur_ref().is_paused() {
            self.bx.threads.cur().stack_limit = previous_limit;
        }
        result
    }

    /// Runs an operation with a cap on active VM call frames.
    ///
    /// The count includes a root evaluation frame. The raw Makepad VM has no
    /// call-frame cap, while nested bounded executions retain the narrower
    /// cap. A paused continuation retains its starting cap.
    pub fn with_call_frame_limit<R>(
        &mut self,
        call_frame_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous_limit = self.bx.threads.cur_ref().call_frame_limit;
        self.bx.threads.cur().call_frame_limit = Some(
            previous_limit
                .map(|previous| previous.min(call_frame_limit))
                .unwrap_or(call_frame_limit),
        );
        let result = f(self);
        if !self.bx.threads.cur_ref().is_paused() {
            self.bx.threads.cur().call_frame_limit = previous_limit;
        }
        result
    }

    /// Clears resource-limit signals that did not belong to an active VM
    /// instruction. Hosts normally do not need this: `run_core` consumes its
    /// own failures, and Octoscript clears stale signals before a fresh eval.
    pub fn clear_execution_limit_failures(&mut self) {
        self.bx.threads.cur().take_stack_limit_exceeded();
        self.bx.threads.cur().take_call_frame_limit_exceeded();
    }

    /// Run one untrusted evaluation/callback under a logical heap-allocation
    /// ceiling. The default VM has no ceiling; callers opt in explicitly, so
    /// trusted widget, shader and Studio script execution is unchanged.
    ///
    /// Container growth is charged before allocation. If a script exceeds
    /// the allowance, the current run bails with a captured `script limit`
    /// error instead of attempting the allocation. The report lets a host
    /// share one cumulative allowance across several callbacks in a tick.
    pub fn with_heap_allocation_limit<R>(
        &mut self,
        allocation_limit: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> (R, ScriptAllocationReport) {
        let previous = self.bx.heap.begin_allocation_budget(allocation_limit);
        let result = f(self);
        // Parser/native work can be the last action in `f`, with no following
        // opcode for run_core's poll. Surface that pending refusal here too.
        if let Some(message) = self.bx.heap.take_allocation_error() {
            let _ = script_err_limit!(self.bx.threads.cur_ref().trap, "{}", message);
            self.drain_errors();
        }
        let report = self.bx.heap.end_allocation_budget(previous);
        (result, report)
    }

    /// Instructions charged by the most recent `with_instruction_limit` call.
    pub fn last_limit_consumed(&self) -> usize {
        self.bx.last_limit_consumed
    }

    pub fn heap(&self) -> &ScriptHeap {
        &self.bx.heap
    }

    pub fn heap_mut(&mut self) -> &mut ScriptHeap {
        &mut self.bx.heap
    }

    /// Print a script value to stdout with debug formatting.
    pub fn println(&self, value: impl Into<ScriptValue>) {
        self.bx.heap.println(value.into());
    }

    /// Run garbage collection (mark and sweep). Deliberately does NOT return
    /// backing capacity to the allocator: routine GC (paint loop, isolate
    /// round-robin) refills the free lists at the same rate, and repeated
    /// shrink/regrow churn costs more than the high-water memory it saves.
    pub fn gc(&mut self) {
        self.bx.heap.mark(&self.bx.threads, &self.bx.code);
        self.bx.heap.sweep(false);
    }

    /// [`Self::gc`] plus returning over-allocated backing capacity (safe: no
    /// live slot is moved or removed). Call after a known allocation spike —
    /// a world teardown, an eval reset — not on the routine GC cadence.
    pub fn gc_and_compact(&mut self) {
        self.gc();
        self.bx.heap.shrink_to_fit();
    }

    /// Free a host-created transient value (a per-call args container, a
    /// per-tick input object) immediately instead of leaving it for GC. Safe
    /// by construction: if the script retained the value, the escape barrier
    /// (`ScriptHeap::escape_value`) tagged it REFFED and this is a no-op —
    /// normal GC handles it. Two caveats for hosts: (1) values bound as fn
    /// args of a call that PAUSED are still live in the paused scope without
    /// being REFFED — only release after the call completed unpaused (check
    /// `vm.thread().is_paused()`); (2) values stored via the `*_unchecked`
    /// heap fns bypass the barrier — releasing those is the host's own
    /// responsibility.
    pub fn release_transient(&mut self, v: ScriptValue) {
        self.bx.heap.free_value_if_unreffed(v);
    }

    /// Run garbage collection with status logging.
    pub fn gc_with_status(&mut self) {
        self.bx.heap.mark(&self.bx.threads, &self.bx.code);
        self.bx.heap.sweep(true);
    }

    pub fn thread(&self) -> &ScriptThread {
        self.bx.threads.cur_ref()
    }

    pub fn thread_mut(&mut self) -> &mut ScriptThread {
        self.bx.threads.cur()
    }

    pub fn trap(&'a self) -> ScriptTrap<'a> {
        self.bx.threads.cur_ref().trap.pass()
    }

    /// Format an enum variant error with descriptive information about the value.
    /// Used by generated code from derive macros for better error messages.
    pub fn format_enum_variant_error(&self, value: ScriptValue) -> String {
        crate::suggest::format_enum_variant_error(&self.bx.heap, value)
    }

    /// Format a ScriptObject for error messages with a brief debug representation.
    /// Shows the object's proto chain and key properties.
    pub fn format_object_for_error(&self, obj: ScriptObject) -> String {
        let mut out = String::new();
        let mut recur = Vec::new();
        // Use the heap's debug string but limit depth to keep it concise
        self.bx
            .heap
            .to_debug_string(obj.into(), &mut recur, &mut out, false, 0);
        // Truncate if too long
        if out.len() > 200 {
            out.truncate(197);
            out.push_str("...");
        }
        out
    }

    pub fn set_thread(&mut self, id: usize) {
        self.bx.threads.set_current(id);
    }

    pub fn with_vm<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        f(self)
    }

    pub fn is_reload(&self) -> bool {
        self.bx.is_reload
    }

    pub fn with_reload<R, F: FnOnce(&mut ScriptVm) -> R>(&mut self, f: F) -> R {
        let was_reload = std::mem::replace(&mut self.bx.is_reload, true);
        let out = f(self);
        self.bx.is_reload = was_reload;
        out
    }

    fn script_me_from_value(&mut self, me: ScriptValue) -> Option<ScriptMe> {
        if me.is_nil() {
            return None;
        }
        if let Some(obj) = me.as_object() {
            return Some(ScriptMe::Object(obj));
        }
        if let Some(arr) = me.as_array() {
            return Some(ScriptMe::Array(arr));
        }
        if let Some(pod) = me.as_pod() {
            return Some(ScriptMe::Pod {
                pod,
                offset: Default::default(),
            });
        }
        None
    }

    fn call_with_scope(&mut self, scope: ScriptObject, me: ScriptValue) -> ScriptValue {
        if let Some(fnptr) = self.bx.heap.parent_as_fn(scope) {
            match fnptr {
                ScriptFnPtr::Native(ni) => {
                    // Get the function pointer and drop the borrow before calling
                    let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                        let native = self.bx.code.native.borrow();
                        &*native.functions[ni.index as usize] as *const _
                    };
                    // Pause thread before native call so re-entrant calls get a different thread
                    self.bx.threads.cur().is_paused = true;
                    // SAFETY: The function pointer is valid as long as native functions aren't removed during execution
                    let result = unsafe { (*func_ptr)(self, scope) };
                    // Only unpause if native didn't explicitly pause (via pause() which sets trap.on to Pause)
                    if !matches!(
                        self.bx.threads.cur().trap.get_on(),
                        Some(ScriptTrapOn::Pause)
                    ) {
                        self.bx.threads.cur().is_paused = false;
                        // Eager-free the call scope (same contract as
                        // handle_call_exec): a native that stored or returned
                        // it left it REFFED / guarded.
                        if result.as_object() != Some(scope) {
                            self.bx.heap.free_object_if_unreffed(scope);
                        }
                    }
                    return result;
                }
                ScriptFnPtr::Script(sip) => {
                    let call = CallFrame {
                        bases: self.bx.threads.cur_ref().new_bases(),
                        args: OpcodeArgs::default(),
                        return_ip: None,
                        prev_slot_base: self.bx.threads.cur_ref().slot_base,
                    };
                    if !self.bx.threads.cur().push_call_frame(call) {
                        return self
                            .handle_execution_limit_failure()
                            .expect("a rejected call frame raises a limit failure");
                    }
                    self.bx.threads.cur().scopes.push(scope);
                    if let Some(me) = self.script_me_from_value(me) {
                        self.bx.threads.cur().mes.push(me);
                    }
                    self.bx.threads.cur().trap.ip = sip;
                    self.drain_stale_errors();
                    return self.run_core();
                }
            }
        } else {
            return script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                self.bx.heap.proto(scope).value_type()
            );
        }
    }

    pub fn call(&mut self, fnobj: ScriptValue, args: &[ScriptValue]) -> ScriptValue {
        self.call_with_me(fnobj, args, NIL)
    }

    pub fn call_with_self(
        &mut self,
        fnobj: ScriptValue,
        args: &[ScriptValue],
        sself: ScriptValue,
    ) -> ScriptValue {
        let scope = self.bx.heap.new_with_proto(fnobj);

        self.bx.heap.clear_object_deep(scope);
        if fnobj.is_err() {
            return fnobj;
        }

        let trap = self.bx.threads.cur().trap.pass();
        let err = self.bx.heap.push_all_fn_args(scope, args, trap);
        if err.is_err() {
            return err;
        }
        if !sself.is_nil() {
            self.bx
                .heap
                .force_value_in_map(scope, id!(self).into(), sself);
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);
        self.call_with_scope(scope, NIL)
    }

    pub fn call_with_me(
        &mut self,
        fnobj: ScriptValue,
        args: &[ScriptValue],
        me: ScriptValue,
    ) -> ScriptValue {
        let scope = self.bx.heap.new_with_proto(fnobj);

        self.bx.heap.clear_object_deep(scope);
        if fnobj.is_err() {
            return fnobj;
        }

        let trap = self.bx.threads.cur().trap.pass();
        let err = self.bx.heap.push_all_fn_args(scope, args, trap);
        if err.is_err() {
            return err;
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);
        self.call_with_scope(scope, me)
    }

    pub fn call_with_args_object(
        &mut self,
        fnobj: ScriptValue,
        args_obj: ScriptObject,
    ) -> ScriptValue {
        self.call_with_args_object_with_me(fnobj, args_obj, NIL)
    }

    pub fn call_with_args_object_with_me(
        &mut self,
        fnobj: ScriptValue,
        args_obj: ScriptObject,
        me: ScriptValue,
    ) -> ScriptValue {
        if fnobj.is_err() {
            return fnobj;
        }
        if fnobj.as_object().is_none() {
            return script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                fnobj.value_type()
            );
        }

        let scope = self.bx.heap.new_with_proto(fnobj);
        self.bx.heap.set_object_storage_vec2(scope);
        self.bx.heap.clear_object_deep(scope);

        let trap = self.bx.threads.cur().trap.pass();
        // Map positional (unnamed) vec args to named function parameters,
        // and merge named map args directly.
        let vec_len = self.bx.heap.vec_len(args_obj);
        for i in 0..vec_len {
            let kv = self.bx.heap.vec_key_value(args_obj, i, trap);
            if kv.key.is_nil() {
                // Unnamed positional arg — map to named parameter via unnamed_fn_arg
                self.bx.heap.unnamed_fn_arg(scope, kv.value, trap);
            } else {
                // Named arg — insert directly
                self.bx.heap.vec_push(scope, kv.key, kv.value, trap);
            }
        }
        // Copy map entries (like `self`, `ui`) from args_obj to scope
        let map_entries: Vec<_> = self
            .bx
            .heap
            .map_ref(args_obj)
            .iter()
            .map(|(k, v)| (*k, v.value))
            .collect();
        for (k, v) in map_entries {
            self.bx.heap.force_value_in_map(scope, k, v);
        }

        self.bx.heap.set_object_deep(scope);
        self.bx.heap.set_object_storage_auto(scope);
        self.call_with_scope(scope, me)
    }

    fn format_error(&self, err: &crate::trap::ScriptError) -> String {
        let loc = err
            .value
            .as_err()
            .and_then(|ptr| self.bx.code.ip_to_loc(ptr.ip));
        if let Some(loc) = loc {
            format!(
                "{}:{}:{}: {} ({}:{})",
                loc.file, loc.line, loc.col, err.message, err.origin_file, err.origin_line
            )
        } else {
            format!("{}: {}", err.origin_file, err.message)
        }
    }

    /// Drain pending errors into formatted strings instead of logging them.
    /// Note that errors raised DURING execution are drained by `run_core`
    /// itself (into the log, or into the captured-error sink when one is
    /// installed) — this only sees errors still queued afterwards. Hosts that
    /// need reliable capture install a sink: `vm.bx.captured_errors =
    /// Some(Vec::new())` before running, then take it after.
    pub fn take_errors(&mut self) -> Vec<String> {
        let mut out = std::mem::take(&mut self.bx.captured_errors).unwrap_or_default();
        loop {
            let err = self.bx.threads.cur().trap.err_pop_front();
            let Some(err) = err else {
                break;
            };
            out.push(self.format_error(&err));
        }
        out
    }

    /// Drain and log any pending errors in the error queue.
    /// Call this after operations that may produce errors outside of run_core
    /// (e.g., script_apply calls from Rust code).
    ///
    /// When a captured-error sink is installed (`bx.captured_errors`), errors
    /// go there instead of the log — even while `silence_errors` is set, so a
    /// host can collect diagnostics from streaming/incremental evals that
    /// would otherwise be dropped as meaningless-mid-stream.
    pub fn drain_errors(&mut self) {
        loop {
            let err = self.bx.threads.cur().trap.err_pop_front();
            if let Some(err) = err {
                if self.bx.captured_errors.is_some() {
                    let formatted = self.format_error(&err);
                    if let Some(sink) = self.bx.captured_errors.as_mut() {
                        sink.push(formatted);
                    }
                    continue;
                }
                if self.bx.silence_errors {
                    continue;
                }
                if let Some(ptr) = err.value.as_err() {
                    if let Some(loc2) = self.bx.code.ip_to_loc(ptr.ip) {
                        log_with_level(
                            &loc2.file,
                            loc2.line,
                            loc2.col,
                            loc2.line,
                            loc2.col,
                            format!("{} ({}:{})", err.message, err.origin_file, err.origin_line),
                            LogLevel::Error,
                        );
                    } else {
                        // No location info, still log the error
                        log_with_level(
                            &err.origin_file,
                            err.origin_line,
                            0,
                            err.origin_line,
                            0,
                            err.message.clone(),
                            LogLevel::Error,
                        );
                    }
                } else {
                    // Error without IP, still log
                    log_with_level(
                        &err.origin_file,
                        err.origin_line,
                        0,
                        err.origin_line,
                        0,
                        err.message.clone(),
                        LogLevel::Error,
                    );
                }
            } else {
                break;
            }
        }
    }

    #[inline(never)]
    #[cold]
    fn handle_errors(&mut self) {
        if self.bx.threads.cur_ref().call_stack_has_try() {
            // Discard younger script calls until the active frame owns a try
            // frame, restoring that frame's instruction body. Hard VM bails
            // (ScriptTrapOn::Bail) never enter this path.
            while !self.bx.threads.cur_ref().call_has_try() {
                let Some(call) = self.bx.threads.cur().calls.pop() else {
                    self.bail("calls empty while unwinding to a try frame");
                    return;
                };
                let return_ip = call.return_ip;
                self.bx.threads.cur().slot_base = call.prev_slot_base;
                self.bx
                    .threads
                    .cur()
                    .truncate_bases(call.bases, &mut self.bx.heap);
                let Some(return_ip) = return_ip else {
                    self.bail("root call reached while unwinding to a try frame");
                    return;
                };
                self.bx.threads.cur().trap.ip = return_ip;
            }
            // pop all errors
            self.bx.threads.cur().trap.err_clear();
            let try_frame = self.bx.threads.cur().tries.pop().unwrap();
            self.bx
                .threads
                .cur()
                .truncate_bases(try_frame.bases, &mut self.bx.heap);
            if try_frame.push_nil {
                self.bx.threads.cur().push_stack_unchecked(NIL)
            }
            self.bx
                .threads
                .cur()
                .trap
                .goto(try_frame.start_ip + try_frame.jump);
        } else {
            // An uncaught error is reported and the evaluation continues with
            // the error value in hand: one bad statement in a module must not
            // take the rest of the module with it, a live edit is broken half
            // the time, and scripts inspect returned error values (`?`).
            //
            // A host that runs script it did not write opts into
            // `bail_on_uncaught_error`: the error then terminates the
            // evaluation before another instruction (and therefore another
            // host effect) can execute, and rides the Bail back as its result.
            // Streaming evals (`silence_errors`) never bail: incomplete source
            // inevitably raises errors that mean nothing until the rest arrives.
            if !self.bx.bail_on_uncaught_error || self.bx.silence_errors {
                self.drain_errors();
                return;
            }
            let error = self
                .bx
                .threads
                .cur_ref()
                .trap
                .err_borrow()
                .front()
                .map(|e| e.value);
            self.drain_errors();
            if let Some(error) = error {
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(error)));
            }
        }
    }

    fn check_run_budget(&mut self) -> Option<ScriptRunBudgetHit> {
        let budget = self.bx.run_budget.as_mut()?;
        budget.instructions_until_sample = budget.instructions_until_sample.saturating_sub(1);
        if budget.instructions_until_sample > 0 {
            return None;
        }
        budget.instructions_until_sample = budget.sample_interval_instructions;

        let now = crate::clock::monotonic_now();
        if now >= budget.hard_deadline {
            return Some(ScriptRunBudgetHit::Hard);
        }
        if now >= budget.soft_deadline {
            return Some(ScriptRunBudgetHit::Soft);
        }
        None
    }

    #[inline(never)]
    #[cold]
    fn bail_resource_limit(&mut self, message: &'static str) -> ScriptValue {
        let err = script_err_limit!(self.bx.threads.cur_ref().trap, "{message}");
        // Resource-limit failures are uncatchable. Drain pre-existing script
        // errors before the Bail unwinds to a clean root state.
        self.drain_errors();
        self.bx
            .threads
            .cur()
            .trap
            .set_on(Some(ScriptTrapOn::Bail(err)));
        self.handle_trap_on()
            .expect("resource-limit Bail must return a VM value")
    }

    /// Errors raised while no script was running (a host apply, a host call of
    /// a value that is not a function) belong to no evaluation. They are
    /// reported here, at the Rust -> script boundary, so the run that starts
    /// now can neither be caught by them nor be ended by them.
    #[inline(always)]
    fn drain_stale_errors(&mut self) {
        if self.bx.threads.cur_ref().trap.has_err() {
            self.drain_errors();
        }
    }

    #[inline(never)]
    #[cold]
    fn handle_execution_limit_failure(&mut self) -> Option<ScriptValue> {
        let stack_limit_exceeded = self.bx.threads.cur().take_stack_limit_exceeded();
        let call_frame_limit_exceeded = self.bx.threads.cur().take_call_frame_limit_exceeded();
        if stack_limit_exceeded {
            return Some(self.bail_resource_limit("script operand stack limit exceeded"));
        }
        if call_frame_limit_exceeded {
            return Some(self.bail_resource_limit("script call frame limit exceeded"));
        }
        None
    }

    fn handle_trap_on(&mut self) -> Option<ScriptValue> {
        if self.bx.threads.cur().trap.on_is_none() {
            return None;
        }
        Some(match self.bx.threads.cur().trap.take_on().unwrap() {
            ScriptTrapOn::Pause | ScriptTrapOn::TimeBudgetYield => NIL,
            ScriptTrapOn::Return(value) => {
                // Preserve the remaining allowance for consumption accounting
                // (with_instruction_limit reads it after the None wipe).
                if let Some(rem) = self.bx.threads.cur().instruction_limit_remaining.take() {
                    self.bx.last_limit_exit_remaining = rem;
                }
                value
            }
            ScriptTrapOn::Bail(value) => {
                // Stack corruption or hard failure: unwind calls to find our root frame
                // and truncate all stacks back to clean state.
                loop {
                    if let Some(call) = self.bx.threads.cur().calls.pop() {
                        self.bx.threads.cur().slot_base = call.prev_slot_base;
                        self.bx
                            .threads
                            .cur()
                            .truncate_bases(call.bases, &mut self.bx.heap);
                        if call.return_ip.is_none() {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if let Some(rem) = self.bx.threads.cur().instruction_limit_remaining.take() {
                    self.bx.last_limit_exit_remaining = rem;
                }
                value
            }
        })
    }

    pub fn run_core(&mut self) -> ScriptValue {
        // Cached pointer into the active body's opcode buffer, so the loop
        // pays no RefCell borrow per instruction. It is only trusted while
        // `bodies.epoch()` is unchanged: see `ScriptBodies`.
        let mut cached_body_index: usize = usize::MAX;
        let mut cached_epoch: u64 = 0;
        let mut opcodes_ptr: *const ScriptValue = std::ptr::null();
        let mut opcodes_len: usize = 0;

        // A limit signal raised before this run started (a rejected frame, a
        // stale flag) fails it before the first instruction.
        if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
            if let Some(value) = self.handle_execution_limit_failure() {
                return value;
            }
        }

        loop {
            // Heap growth paths cannot own the interpreter trap (many are
            // shared with parsers/native bindings), so they record one hard
            // refusal on the heap. Turn it into the same uncatchable Bail as
            // the instruction ceiling before executing another opcode.
            if let Some(message) = self.bx.heap.take_allocation_error() {
                let err = script_err_limit!(
                    self.bx.threads.cur_ref().trap,
                    "{}",
                    message
                );
                self.drain_errors();
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(err)));
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }
            let instruction_limit_exceeded = if let Some(remaining) =
                self.bx.threads.cur().instruction_limit_remaining.as_mut()
            {
                if *remaining == 0 {
                    true
                } else {
                    *remaining -= 1;
                    false
                }
            } else {
                false
            };
            if instruction_limit_exceeded {
                let err = script_err_limit!(
                    self.bx.threads.cur_ref().trap,
                    "script instruction limit exceeded"
                );
                // drain_errors routes to the captured-error sink, the log, or
                // the void depending on host configuration.
                self.drain_errors();
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(err)));
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }

            if let Some(hit) = self.check_run_budget() {
                match hit {
                    ScriptRunBudgetHit::Soft => {
                        self.bx.threads.cur().is_paused = true;
                        self.bx
                            .threads
                            .cur()
                            .trap
                            .set_on(Some(ScriptTrapOn::TimeBudgetYield));
                    }
                    ScriptRunBudgetHit::Hard => {
                        let err = script_err_limit!(
                            self.bx.threads.cur().trap.pass(),
                            "script time budget exceeded"
                        );
                        // A hard budget hit is an uncatchable VM bail. Move its
                        // diagnostic out of the thread queue before unwinding so
                        // a later run cannot observe it as a script error.
                        self.drain_errors();
                        self.bx
                            .threads
                            .cur()
                            .trap
                            .set_on(Some(ScriptTrapOn::Bail(err)));
                    }
                }
                if let Some(value) = self.handle_trap_on() {
                    return value;
                }
            }

            let thread = self.bx.threads.cur();
            let body_index = thread.trap.ip.body as usize;
            let ip_index = thread.trap.ip.index as usize;

            // Re-derive the pointer when the body changes or when anything
            // mutated the bodies since it was taken: a native call may have
            // re-entered the VM and reloaded the very body that is running.
            let epoch = self.bx.code.bodies.epoch();
            if body_index != cached_body_index || epoch != cached_epoch {
                let bodies = self.bx.code.bodies.borrow();
                let opcodes = &bodies[body_index].parser.opcodes;
                opcodes_ptr = opcodes.as_ptr();
                opcodes_len = opcodes.len();
                cached_body_index = body_index;
                cached_epoch = epoch;
            }

            if ip_index >= opcodes_len {
                // If there's a value on the stack, return it (for expression-style scripts)
                let stack_len = self.bx.threads.cur().stack.len();
                if stack_len > 0 {
                    return self.bx.threads.cur().pop_stack_value();
                }
                return NIL;
            }

            // SAFETY: the buffer can only move or be freed through
            // `bodies.borrow_mut()`, which advances the epoch checked above.
            let opcode = unsafe { *opcodes_ptr.add(ip_index) };

            if self.bx.debug_trace {
                let stack_len = self.bx.threads.cur_ref().stack.len();
                if let Some((op, a)) = opcode.as_opcode() {
                    eprintln!("TRACE b{body_index} ip{ip_index} stack{stack_len} {op:?} {a:?}");
                } else {
                    eprintln!("TRACE b{body_index} ip{ip_index} stack{stack_len} PUSH {opcode:?}");
                }
            }

            if let Some((opcode, args)) = opcode.as_opcode() {
                self.opcode(opcode, args);
                if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
                    if let Some(value) = self.handle_execution_limit_failure() {
                        return value;
                    }
                }
                // single-load poll for both interrupt sources (errors + traps)
                let pending = self.bx.threads.cur().trap.pending();
                if pending != 0 {
                    if pending & crate::trap::TRAP_PENDING_ERR != 0 {
                        self.handle_errors();
                    }
                    if let Some(value) = self.handle_trap_on() {
                        return value;
                    }
                }
            } else {
                // its a direct value-to-stack
                self.bx.threads.cur().push_stack_value(opcode);
                self.bx.threads.cur().trap.goto_next();
                if self.bx.threads.cur_ref().has_execution_limit_exceeded() {
                    if let Some(value) = self.handle_execution_limit_failure() {
                        return value;
                    }
                }
            }
        }
    }

    pub fn run_root(&mut self, body_id: u16) -> ScriptValue {
        // Extract values from bodies before modifying thread state
        let (scope, me) = {
            let bodies = self.bx.code.bodies.borrow();
            (
                bodies[body_id as usize].scope.obj,
                bodies[body_id as usize].me.obj,
            )
        };

        let root_slots = self.bx.threads.cur_ref().slots.len();
        let root_slot_base = self.bx.threads.cur_ref().slot_base;
        if !self.bx.threads.cur().push_call_frame(CallFrame {
            bases: StackBases {
                tries: 0,
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
                // unlike the other zeroed bases, never truncate slots below
                // what an enclosing (paused/re-entrant) frame allocated
                slots: root_slots,
            },
            args: Default::default(),
            return_ip: None,
            prev_slot_base: root_slot_base,
        }) {
            return self
                .handle_execution_limit_failure()
                .expect("a rejected root frame raises a limit failure");
        }

        self.bx.threads.cur().scopes.push(scope);
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));

        self.bx.threads.cur().trap.ip.body = body_id;
        self.bx.threads.cur().trap.ip.index = 0;

        // the main interpreter loop
        self.drain_stale_errors();
        let value = self.run_core();
        if let Some(end) = self.bx.threads.cur().root_end_scope.take() {
            self.bx.code.bodies.borrow_mut()[body_id as usize].end_scope = Some(end);
        }
        value
    }

    /// Checks if the value has an apply transform and calls it, returning the transformed value.
    /// Returns None if no transform exists, Some(transformed) if a transform was applied.
    pub fn call_apply_transform(&mut self, value: ScriptValue) -> Option<ScriptValue> {
        if let Some(obj) = value.as_object() {
            if let Some(ni) = self.bx.heap.objects[obj].tag.as_apply_transform() {
                let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                    let native = self.bx.code.native.borrow();
                    &*native.functions[ni.index as usize] as *const _
                };
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { (*func_ptr)(self, obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.get_on(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        } else if let Some(arr) = value.as_array() {
            if let Some(ni) = self.bx.heap.arrays[arr].tag.as_apply_transform() {
                // For arrays, we need to create a temporary args object
                let args_obj = self.bx.heap.new_object();
                self.bx
                    .heap
                    .set_value_def(args_obj, id!(self).into(), value);
                let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject) -> ScriptValue = {
                    let native = self.bx.code.native.borrow();
                    &*native.functions[ni.index as usize] as *const _
                };
                // Pause thread before native call so re-entrant calls get a different thread
                self.bx.threads.cur().is_paused = true;
                let result = unsafe { (*func_ptr)(self, args_obj) };
                // Only unpause if native didn't explicitly pause
                if !matches!(
                    self.bx.threads.cur().trap.get_on(),
                    Some(ScriptTrapOn::Pause)
                ) {
                    self.bx.threads.cur().is_paused = false;
                }
                return Some(result);
            }
        }
        None
    }

    pub fn resume(&mut self) -> ScriptValue {
        self.bx.threads.cur().is_paused = false;
        self.run_core()
    }

    pub fn cast_to_f64(&self, v: ScriptValue) -> f64 {
        self.bx
            .heap
            .cast_to_f64(v, self.bx.threads.cur_ref().trap.ip)
    }

    pub fn handle_type(&self, id: LiveId) -> ScriptHandleType {
        *self.bx.code.native.borrow().handle_type.get(&id).unwrap()
    }

    pub fn new_handle_type(&mut self, id: LiveId) -> ScriptHandleType {
        self.bx
            .code
            .native
            .borrow_mut()
            .new_handle_type(&mut self.bx.heap, id)
    }

    pub fn downcast_handle_gc<T: ScriptHandleGc + 'static>(
        &self,
        handle: ScriptHandle,
    ) -> Option<&T> {
        self.bx.heap.handle_ref::<T>(handle)
    }

    pub fn add_handle_method<F>(
        &mut self,
        ht: ScriptHandleType,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx.code.native.borrow_mut().add_type_method(
            &mut self.bx.heap,
            ht.to_redux(),
            method,
            args,
            f,
        )
    }

    pub fn set_handle_setter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId, ScriptValue) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .set_type_setter(ht.to_redux(), f)
    }

    pub fn set_handle_getter<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .set_type_getter(ht.to_redux(), f)
    }

    /// Register a catch-all method dispatcher for a handle type.
    /// When a method call is made on a handle that has no specific method
    /// registered for that name, this call function is invoked with
    /// (vm, args_object, method). The args object has `self` set
    /// and all call arguments collected, just like a normal native method.
    pub fn set_handle_call<F>(&mut self, ht: ScriptHandleType, f: F)
    where
        F: Fn(&mut ScriptVm, ScriptObject, LiveId) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .set_type_call(ht.to_redux(), f)
    }

    pub fn new_module(&mut self, id: LiveId) -> ScriptObject {
        self.bx.heap.new_module(id)
    }

    pub fn module(&mut self, id: LiveId) -> ScriptObject {
        self.bx.heap.module(id)
    }

    pub fn map_mut_with<R, F: FnOnce(&mut Self, &mut ScriptObjectMap) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut map = ScriptObjectMap::default();
        std::mem::swap(&mut map, &mut self.bx.heap.objects[object].map);
        let r = f(self, &mut map);
        std::mem::swap(&mut map, &mut self.bx.heap.objects[object].map);
        r
    }

    /// Walk the prototype chain from root (oldest ancestor) to leaf (the object itself),
    /// calling the closure for each object's map. This is useful for collecting inherited
    /// properties where child properties should override parent properties.
    pub fn proto_map_iter_mut_with<F: FnMut(&mut Self, &mut ScriptObjectMap)>(
        &mut self,
        object: ScriptObject,
        f: &mut F,
    ) {
        // First recurse to the prototype (if any), so we process from root to leaf
        if let Some(proto) = self.bx.heap.objects[object].proto.as_object() {
            self.proto_map_iter_mut_with(proto, f);
        }
        // Then process this object's map
        let mut map = ScriptObjectMap::default();
        std::mem::swap(&mut map, &mut self.bx.heap.objects[object].map);
        f(self, &mut map);
        std::mem::swap(&mut map, &mut self.bx.heap.objects[object].map);
    }

    pub fn vec_with<R, F: FnOnce(&mut Self, &[ScriptVecValue]) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut vec = Vec::new();
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        let r = f(self, &vec);
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        r
    }

    pub fn vec_mut_with<R, F: FnOnce(&mut Self, &mut Vec<ScriptVecValue>) -> R>(
        &mut self,
        object: ScriptObject,
        f: F,
    ) -> R {
        let mut vec = Vec::new();
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        let r = f(self, &mut vec);
        std::mem::swap(&mut vec, &mut self.bx.heap.objects[object].vec);
        r
    }

    pub fn string_with<R, F: FnOnce(&mut Self, &str) -> R>(
        &mut self,
        value: ScriptValue,
        f: F,
    ) -> Option<R> {
        if let Some(s) = value.as_string() {
            if let Some(s) = &self.bx.heap.strings[s] {
                let s = s.string.clone();
                return Some(f(self, &s.0));
            }
            return None;
        }
        if let Some(r) = value.as_inline_string(|s| f(self, s)) {
            return Some(r);
        }
        None
    }

    pub fn new_string_with<F: FnOnce(&mut Self, &mut String)>(&mut self, f: F) -> ScriptValue {
        if self.bx.heap.has_allocation_budget() {
            let _ = self.bx.heap.charge_allocation(
                usize::MAX,
                "building a native string without an allocation preflight",
            );
            return NIL;
        }
        let mut out = if let Some(s) = self.bx.heap.strings_reuse.pop() {
            s
        } else {
            String::new()
        };
        f(self, &mut out);
        self.bx.heap.intern_or_store_string(out)
    }

    pub fn add_method<F>(
        &mut self,
        module: ScriptObject,
        method: LiveId,
        args: &[(LiveId, ScriptValue)],
        f: F,
    ) where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx
            .code
            .native
            .borrow_mut()
            .add_method(&mut self.bx.heap, module, method, args, f)
    }

    fn apply_injected_globals_to_scope(&mut self, scope_obj: ScriptObject) {
        if self.bx.injected_globals.is_empty() {
            return;
        }
        let globals: Vec<(LiveId, ScriptValue)> = self
            .bx
            .injected_globals
            .iter()
            .map(|(key, value)| (*key, *value))
            .collect();
        for (key, value) in globals {
            self.bx
                .heap
                .force_value_in_map(scope_obj, key.into(), value);
        }
    }

    fn apply_injected_globals_to_all_scopes(&mut self) {
        if self.bx.injected_globals.is_empty() {
            return;
        }
        let scope_objects: Vec<ScriptObject> = {
            let bodies = self.bx.code.bodies.borrow();
            bodies.iter().map(|body| body.scope.as_object()).collect()
        };
        for scope_obj in scope_objects {
            self.apply_injected_globals_to_scope(scope_obj);
        }
    }

    pub fn set_injected_global(&mut self, key: LiveId, value: ScriptValue) {
        self.bx.injected_globals.insert(key, value);
        self.apply_injected_globals_to_all_scopes();
    }

    /// Registers a native function to be used as an apply_transform and returns its NativeId.
    /// This is used for creating objects that transform to a computed value when applied.
    pub fn add_apply_transform_fn<F>(&mut self, f: F) -> NativeId
    where
        F: Fn(&mut ScriptVm, ScriptObject) -> ScriptValue + 'static,
    {
        self.bx.code.native.borrow_mut().add_apply_transform_fn(f)
    }

    pub fn add_script_mod(&mut self, new_mod: ScriptMod) -> u16 {
        // Register this crate's manifest path for crate path resolution
        let crate_name = new_mod.module_path.split("::").next().unwrap_or("");
        if !crate_name.is_empty() {
            self.bx.code.crate_manifests.borrow_mut().insert(
                crate_name.replace('-', "_"),
                new_mod.cargo_manifest_path.clone(),
            );
        }

        let scope_obj = self.bx.heap.new_with_proto(id!(scope).into());
        self.bx.heap.set_object_deep(scope_obj);
        self.bx
            .heap
            .set_value_def(scope_obj, id!(mod).into(), self.bx.heap.modules.into());
        self.apply_injected_globals_to_scope(scope_obj);
        let scope = self.bx.heap.new_object_ref(scope_obj);
        let me_obj = self.bx.heap.new_with_proto(id!(root_me).into());
        let me = self.bx.heap.new_object_ref(me_obj);
        let key = ScriptModKey::from_script_mod(&new_mod);
        let override_code = self
            .bx
            .code
            .script_mod_overrides
            .borrow()
            .get(&key)
            .cloned();
        let effective_code = override_code
            .clone()
            .unwrap_or_else(|| new_mod.code.clone());

        let new_body = ScriptBody {
            source: ScriptSource::Mod(new_mod),
            effective_code,
            tokenizer: ScriptTokenizer::default(),
            parser: ScriptParser::default(),
            scope,
            me,
            end_scope: None,
            checkpoint: None,
            source_len: 0,
        };
        let mut bodies = self.bx.code.bodies.borrow_mut();
        for (i, body) in bodies.iter_mut().enumerate() {
            if let ScriptSource::Mod(script_mod) = &body.source {
                if let ScriptSource::Mod(new_mod) = &new_body.source {
                    if script_mod.file == new_mod.file
                        && script_mod.line == new_mod.line
                        && script_mod.column == new_mod.column
                    {
                        let values_changed = script_mod.values != new_mod.values;
                        body.source = new_body.source;
                        body.scope = new_body.scope;
                        body.me = new_body.me;
                        if body.effective_code != new_body.effective_code || values_changed {
                            body.effective_code = new_body.effective_code;
                            body.tokenizer = ScriptTokenizer::default();
                            body.parser = ScriptParser::default();
                            body.checkpoint = None;
                            body.source_len = 0;
                        }
                        return i as u16;
                    }
                }
            }
        }
        let i = bodies.len();
        // A body id past the packing would alias another body's functions
        // (see `ScriptIp::BODY_BITS`): stop here rather than run the wrong code.
        assert!(i < ScriptIp::MAX_BODIES, "script body limit reached: {} bodies fit in ScriptIp", ScriptIp::MAX_BODIES);
        bodies.push(new_body);
        i as u16
    }

    pub fn eval(&mut self, script_mod: ScriptMod) -> ScriptValue {
        self.eval_with_source(script_mod, ScriptObject::ZERO)
    }

    pub fn eval_with_source(&mut self, script_mod: ScriptMod, source: ScriptObject) -> ScriptValue {
        let body_id = self.add_script_mod(script_mod);

        // Set __script_source__ on the scope if source is provided
        // If source has FROM_EVAL flag, use its prototype instead
        if source != ScriptObject::ZERO {
            let actual_source = if self.bx.heap.is_from_eval(source) {
                // Use the prototype of the FROM_EVAL object
                if let Some(proto) = self.bx.heap.proto(source).as_object() {
                    proto
                } else {
                    source
                }
            } else {
                source
            };
            let scope = self.bx.code.bodies.borrow()[body_id as usize].scope.obj;
            self.bx
                .heap
                .set_value_def(scope, id!(__script_source__).into(), actual_source.into());
        }

        let mut bodies = self.bx.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];

        if let ScriptSource::Mod(script_mod) = &body.source {
            if body.source_len == 0 {
                body.tokenizer.clear();
                body.parser = ScriptParser::default();
                body.tokenizer
                    .tokenize(&body.effective_code, &mut self.bx.heap);
                body.parser.parse(
                    &body.tokenizer,
                    &script_mod.file,
                    (script_mod.line, script_mod.column),
                    &script_mod.values,
                );
                body.source_len = body.effective_code.len();
            }
            // Parse errors never enter the trap queue (the parser recovers);
            // surface them to a captured-diagnostics sink here or a validating
            // host reports success for a script that failed to parse.
            let parse_errors = std::mem::take(&mut body.parser.parse_errors);
            drop(bodies);
            if let Some(sink) = self.bx.captured_errors.as_mut() {
                sink.extend(parse_errors);
            }
            // lets point our thread to it
            let result = self.run_root(body_id);
            // Mark the result object with FROM_EVAL flag
            if let Some(result_obj) = result.as_object() {
                self.bx.heap.set_from_eval(result_obj);
            }

            result
        } else {
            NIL
        }
    }

    /// Evaluate script incrementally by appending new source to an existing body.
    ///
    /// Pass the full growing source code string each time. On first call, creates
    /// the body and tokenizes/parses everything. On subsequent calls, computes the
    /// delta (new chars since last call), restores the parser checkpoint (removing
    /// auto-close opcodes), tokenizes only the new chars, continues parsing, then
    /// auto-closes again for execution. Always re-executes from opcode 0.
    pub fn eval_with_append_source(
        &mut self,
        script_mod: ScriptMod,
        code: &str,
        source: ScriptObject,
    ) -> ScriptValue {
        // Look for an existing body with matching file/line/column
        let existing_body_id = {
            let bodies = self.bx.code.bodies.borrow();
            let mut found = None;
            for (i, body) in bodies.iter().enumerate() {
                if let ScriptSource::Mod(existing_mod) = &body.source {
                    if existing_mod.file == script_mod.file
                        && existing_mod.line == script_mod.line
                        && existing_mod.column == script_mod.column
                    {
                        found = Some(i as u16);
                        break;
                    }
                }
            }
            found
        };

        let body_id = match existing_body_id {
            Some(id) => id,
            None => self.add_script_mod(script_mod),
        };

        // Set __script_source__ on the scope if source is provided
        if source != ScriptObject::ZERO {
            let actual_source = if self.bx.heap.is_from_eval(source) {
                if let Some(proto) = self.bx.heap.proto(source).as_object() {
                    proto
                } else {
                    source
                }
            } else {
                source
            };
            let scope = self.bx.code.bodies.borrow()[body_id as usize].scope.obj;
            self.bx
                .heap
                .set_value_def(scope, id!(__script_source__).into(), actual_source.into());
        }

        let mut bodies = self.bx.code.bodies.borrow_mut();
        let body = &mut bodies[body_id as usize];

        if let ScriptSource::Mod(existing_mod) = &body.source {
            // Restore checkpoint (removes auto-close opcodes from previous run)
            if let Some(cp) = body.checkpoint.take() {
                body.parser.restore_checkpoint(cp);
            }

            let prev_len = body.source_len;
            // Check if the content has diverged (not just appended to).
            // Compare the new code's prefix against what the tokenizer already has.
            let content_changed = prev_len > 0
                && (code.len() < prev_len
                    || code[..prev_len] != body.tokenizer.original[..prev_len]);

            if content_changed {
                // Content changed entirely — reset and re-tokenize from scratch
                body.tokenizer.clear();
                body.parser = ScriptParser::default();
                body.checkpoint = None;
                body.source_len = code.len();
                body.tokenizer.tokenize(code, &mut self.bx.heap);
            } else if code.len() >= prev_len {
                body.source_len = code.len();
                let new_chars = &code[prev_len..];
                if !new_chars.is_empty() {
                    body.tokenizer.tokenize(new_chars, &mut self.bx.heap);
                }
            }

            // If we stopped mid-string, intern the partial content so the parser
            // can emit the real string value into opcodes for incremental rendering.
            let unfinished = body.tokenizer.intern_unfinished_string(&mut self.bx.heap);

            // Incremental parse: continue from checkpoint, auto-close for execution
            let errors_before = body.parser.parse_errors.len();
            let cp = body.parser.parse_streaming(
                &body.tokenizer,
                &existing_mod.file,
                (existing_mod.line, existing_mod.column),
                &existing_mod.values,
                unfinished,
            );

            body.checkpoint = Some(cp);

            // A host that installed a captured-error sink is running a GAME
            // eval and needs structural parse errors to FAIL it — the
            // tolerant recovery otherwise runs something else entirely
            // (`let loop` recovered into an infinite empty loop and burned
            // the instruction budget). Live-typing paths install no sink and
            // keep the log-only tolerance.
            let new_parse_errors: Vec<String> =
                body.parser.parse_errors[errors_before.min(body.parser.parse_errors.len())..]
                    .to_vec();

            drop(bodies);
            if let Some(sink) = &mut self.bx.captured_errors {
                sink.extend(new_parse_errors);
            }
            // Silence runtime errors during incremental eval — incomplete code
            // will inevitably produce errors that are meaningless until the
            // source is fully received.
            self.bx.silence_errors = true;
            let result = self.run_root(body_id);
            self.bx.silence_errors = false;
            if let Some(result_obj) = result.as_object() {
                self.bx.heap.set_from_eval(result_obj);
            }
            result
        } else {
            NIL
        }
    }
}

pub struct ScriptVmBase {
    pub void: usize,
    pub code: ScriptCode,
    pub heap: ScriptHeap,
    pub threads: ScriptThreads,
    pub injected_globals: std::collections::HashMap<LiveId, ScriptValue>,
    pub is_reload: bool,
    pub debug_trace: bool,
    pub silence_errors: bool,
    /// Whether script-directed debug output (the `~` LOG operator and
    /// `ScriptVm::log`) may reach the host log. Raw Makepad hosts keep it on;
    /// a standalone sandboxed runtime turns it off before any source runs,
    /// which makes `~` a catchable script error instead of an output path.
    pub allow_debug_output: bool,
    /// Whether an uncaught script error ends the evaluation (a Bail carrying
    /// the error) instead of being reported while the evaluation continues.
    /// Off for Makepad hosts: a module keeps evaluating past one bad statement
    /// and scripts may inspect returned error values. A host that runs script
    /// it did not write turns it on before any source runs.
    pub bail_on_uncaught_error: bool,
    /// When Some, drained errors are pushed here (formatted) instead of being
    /// logged or dropped — even under `silence_errors`. Install before an
    /// eval/call, take after, to feed diagnostics back to a host (e.g. an AI
    /// agent editing the script live).
    pub captured_errors: Option<Vec<String>>,
    pub run_budget: Option<ScriptRunBudget>,
    /// Instructions charged by the most recent with_instruction_limit call
    /// (see ScriptVm::last_limit_consumed).
    pub last_limit_consumed: usize,
    /// The thread's remaining allowance at Return/Bail, stashed because
    /// handle_trap_on wipes instruction_limit_remaining to None on exit.
    pub last_limit_exit_remaining: usize,
}

impl ScriptVmBase {
    pub fn empty() -> Self {
        Self {
            void: 0,
            code: ScriptCode::default(),
            threads: ScriptThreads::empty(),
            heap: ScriptHeap::empty(),
            injected_globals: Default::default(),
            is_reload: false,
            debug_trace: false,
            silence_errors: false,
            allow_debug_output: true,
            bail_on_uncaught_error: false,
            captured_errors: None,
            run_budget: None,
            last_limit_consumed: 0,
            last_limit_exit_remaining: 0,
        }
    }

    pub fn new() -> Self {
        let mut heap = ScriptHeap::empty();
        let mut native = ScriptNative::new(&mut heap);
        define_math_module(&mut heap, &mut native);
        define_std_module(&mut heap, &mut native);
        define_regex_module(&mut heap, &mut native);
        define_html_module(&mut heap, &mut native);
        define_shader_module(&mut heap, &mut native);
        define_gc_module(&mut heap, &mut native);
        let pod_builtins = define_pod_module(&mut heap, &mut native);

        let builtins = ScriptBuiltins::new(&mut heap, pod_builtins);

        Self {
            void: 0,
            code: ScriptCode {
                builtins,
                native: RefCell::new(native),
                bodies: Default::default(),
                crate_manifests: Default::default(),
                script_mod_overrides: Default::default(),
            },
            threads: ScriptThreads::new(),
            heap: heap,
            injected_globals: Default::default(),
            is_reload: false,
            debug_trace: false,
            silence_errors: false,
            allow_debug_output: true,
            bail_on_uncaught_error: false,
            captured_errors: None,
            run_budget: None,
            last_limit_consumed: 0,
            last_limit_exit_remaining: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Script, ScriptHook, Default)]
    struct ApplyEvalParityTest {
        #[source]
        source: ScriptObjectRef,
        #[live]
        is_even: f32,
    }

    #[test]
    fn script_apply_eval_refreshes_interpolated_values_on_reused_callsite() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: &mut host,
            bx: Box::new(ScriptVmBase::new()),
        };

        let mut item = ApplyEvalParityTest::default();
        let obj = vm.heap_mut().new_object();
        item.source = vm.heap_mut().new_object_ref(obj);

        for idx in 0..6 {
            let is_even_f = if idx % 2 == 0 { 1.0f32 } else { 0.0f32 };
            script_apply_eval!(vm, item, {
                is_even: #(is_even_f)
            });
            assert_eq!(
                item.is_even, is_even_f,
                "reused script_apply_eval callsite kept stale value at iteration {}",
                idx
            );
        }
    }

    fn parse_reports_error(code: &str) -> bool {
        let mut bx = ScriptVmBase::new();
        let mut tokenizer = ScriptTokenizer::default();
        // Production eval appends "\n;" so the tail statement finalizes
        // (the streaming parser only closes a statement on the next token);
        // mirror that here or emit-time checks never run for the last line.
        let code = format!("{}\n;", code);
        tokenizer.tokenize(&code, &mut bx.heap);
        let mut parser = ScriptParser::default();
        parser.parse(&tokenizer, "reserved_binding_test", (0, 0), &[]);
        parser.had_error
    }

    #[test]
    fn reserved_words_cannot_be_bound() {
        // let / var
        assert!(parse_reports_error("let me = 1"));
        assert!(parse_reports_error("let self = 1"));
        assert!(parse_reports_error("let scope = 1"));
        assert!(parse_reports_error("let nil = 1"));
        assert!(parse_reports_error("let true = 1"));
        assert!(parse_reports_error("let for = 1"));
        assert!(parse_reports_error("var me = 1"));
        // fn / closure argument names
        assert!(parse_reports_error("let f = |me| me"));
        assert!(parse_reports_error("let f = |x, self| x"));
        assert!(parse_reports_error("fn f(me) { }"));
        // for-loop variables
        assert!(parse_reports_error("for me in [1] { }"));
        assert!(parse_reports_error("for k, self in [1] { }"));
        // destructuring patterns
        assert!(parse_reports_error("let [me] = [1]"), "array simple");
        assert!(parse_reports_error("let {a, me} = {x: 1}"), "object multi");
        assert!(parse_reports_error("let {me} = {x: 1}"), "object single");
        assert!(parse_reports_error("let [nil] = [1]"));
        assert!(parse_reports_error("let [a, [me]] = [1, [2]]"));
    }

    #[test]
    fn reserved_words_still_read_and_normal_names_bind() {
        // Reading the implicits stays legal — only BINDING them errors.
        assert!(!parse_reports_error("let hero = 1"));
        assert!(!parse_reports_error("let x = me"));
        assert!(!parse_reports_error("let f = |dt, input| dt"));
        assert!(!parse_reports_error("for k, v in [1] { }"));
        assert!(!parse_reports_error("let {x} = {x: 1}"));
        // `me:` as an OBJECT KEY is data, not a binding.
        assert!(!parse_reports_error("let o = {me: 1}"));
    }

    #[test]
    fn reentrant_reload_of_the_active_body_does_not_keep_an_opcode_pointer() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm {
            host: &mut host,
            bx: Box::new(ScriptVmBase::new()),
        };

        let reentrant = vm.bx.heap.new_module(id!(reentrant));
        vm.add_method(reentrant, id!(reload), &[], |vm, _| {
            vm.eval(ScriptMod {
                file: "reentrant-reload.octoscript".to_owned(),
                code: "41\n;".to_owned(),
                ..Default::default()
            })
        });

        let _ = vm.eval(ScriptMod {
            file: "reentrant-reload.octoscript".to_owned(),
            code: "use mod.reentrant\nreentrant.reload()\n;".to_owned(),
            ..Default::default()
        });

        // The replacement body intentionally makes the outer VM result
        // unspecified. Under Miri or ASan this path used to dereference the
        // old parser's freed opcode allocation before it could return.
    }

    fn plain_vm(host: &mut ScriptVmHost<(), ()>) -> ScriptVm<'_> {
        ScriptVm {
            host,
            bx: Box::new(ScriptVmBase::new()),
        }
    }

    #[test]
    fn streaming_try_catch_restores_the_contextual_separator_state() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let script_mod = || ScriptMod {
            file: "streaming-try-catch.octoscript".to_owned(),
            ..Default::default()
        };
        // The trailing space makes the tokenizer emit the separator in the
        // first pass, so the checkpoint must retain that it was consumed and
        // the appended identifier `catch` must parse as the fallback.
        let prefix = "use mod.std.assert\n\
                      let catch = 41\n\
                      try {\n\
                          assert(false)\n\
                      } catch ";

        let _ = vm.eval_with_append_source(script_mod(), prefix, ScriptObject::ZERO);
        let result = vm.eval_with_append_source(
            script_mod(),
            &format!("{prefix}catch + 1\n;"),
            ScriptObject::ZERO,
        );

        assert_eq!(result.as_f64(), Some(42.0));
    }

    #[test]
    fn streaming_logical_precedence_repatches_pending_short_circuits() {
        for (prefix, suffix, expected) in [
            ("true || false ", "&& false", true),
            ("false && true ", "|| true", true),
            ("true == 2 ", "> 1", true),
        ] {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = plain_vm(&mut host);
            let module = || ScriptMod {
                file: "streaming-precedence.octoscript".into(),
                ..Default::default()
            };
            let partial = vm.with_instruction_limit(1000, |vm| {
                vm.eval_with_append_source(module(), prefix, ScriptObject::ZERO)
            });
            assert!(
                !partial.is_err(),
                "unfinished logical expression must terminate: {prefix}"
            );
            let result = vm.with_instruction_limit(1000, |vm| {
                vm.eval_with_append_source(
                    module(),
                    &format!("{prefix}{suffix}\n;"),
                    ScriptObject::ZERO,
                )
            });
            assert_eq!(result.as_bool(), Some(expected), "{prefix}{suffix}");
        }
    }

    #[test]
    fn streaming_legacy_try_ok_repatches_the_success_jump() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let script_mod = || ScriptMod {
            file: "streaming-try-ok.octoscript".to_owned(),
            ..Default::default()
        };
        let prefix = "let marker = 0\ntry { 7 } { marker = 1 }";

        let _ = vm.eval_with_append_source(script_mod(), prefix, ScriptObject::ZERO);
        let result = vm.eval_with_append_source(
            script_mod(),
            &format!("{prefix} ok {{ marker = 2 }}\nreturn marker\n;"),
            ScriptObject::ZERO,
        );

        assert_eq!(result.as_u40(), Some(2));
    }

    #[test]
    fn hard_time_budget_drains_its_uncatchable_error() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.captured_errors = Some(Vec::new());
        vm.bx.run_budget = Some(ScriptRunBudget::from_durations(
            Duration::ZERO,
            Duration::ZERO,
            1,
        ));

        let result = vm.eval(ScriptMod {
            file: "hard-time-budget.octoscript".to_owned(),
            code: "try { loop {} } catch { 42 }\n;".to_owned(),
            ..Default::default()
        });

        assert!(result.is_err());
        assert!(vm.bx.threads.cur_ref().trap.err_is_empty());
        assert!(vm
            .take_errors()
            .iter()
            .any(|diagnostic| diagnostic.contains("script time budget exceeded")));
    }

    #[test]
    fn return_does_not_pop_to_me_after_an_operand_stack_limit() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let receiver = vm.heap_mut().new_object();
        let pop_to_me = OpcodeArgs(OpcodeArgs::POP_TO_ME_FLAG);

        {
            let thread = vm.thread_mut();
            thread.stack.push(7.into());
            thread.mes.push(ScriptMe::Object(receiver));
            assert!(thread.push_call_frame(CallFrame {
                bases: StackBases::default(),
                args: OpcodeArgs::NONE,
                return_ip: None,
                prev_slot_base: 0,
            }));
            assert!(thread.push_call_frame(CallFrame {
                bases: StackBases {
                    stack: 1,
                    mes: 1,
                    ..Default::default()
                },
                args: pop_to_me,
                return_ip: Some(ScriptIp::default()),
                prev_slot_base: 0,
            }));
        }

        vm.with_stack_value_limit(1, |vm| {
            vm.opcode(
                Opcode::RETURN,
                OpcodeArgs(OpcodeArgs::NIL.0 | OpcodeArgs::POP_TO_ME_FLAG),
            );
        });

        assert!(vm.thread().has_execution_limit_exceeded());
        assert_eq!(vm.thread().stack.len(), 1);
        assert_eq!(vm.thread().stack[0].as_f64(), Some(7.0));
        assert_eq!(vm.bx.heap.vec_len(receiver), 0);
    }

    #[test]
    fn uncaught_error_terminates_an_eval_only_when_the_host_opts_in() {
        // `f()` on a number raises an uncaught script error; `42` follows it.
        let source = "let f = 1\nf()\n42";
        let plain_eval = |vm: &mut ScriptVm| {
            vm.bx.captured_errors = Some(Vec::new());
            vm.eval(ScriptMod {
                file: "uncaught-plain.octoscript".to_owned(),
                code: format!("{source}\n;"),
                ..Default::default()
            })
        };

        // Default: the error is reported and the module keeps evaluating.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        let plain = plain_eval(&mut vm);
        assert_eq!(plain.as_number(), Some(42.0), "{plain:?}");
        assert!(!vm.take_errors().is_empty());

        // Opted in: the error ends the evaluation and is its result.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.bail_on_uncaught_error = true;
        let bailed = plain_eval(&mut vm);
        assert!(bailed.is_err(), "{bailed:?}");
        assert!(!vm.take_errors().is_empty());

        // Streaming evals never bail, opted in or not.
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);
        vm.bx.bail_on_uncaught_error = true;
        let streaming = vm.eval_with_append_source(
            ScriptMod {
                file: "uncaught-streaming.octoscript".to_owned(),
                ..Default::default()
            },
            &format!("{source}\n;"),
            ScriptObject::ZERO,
        );
        assert_eq!(streaming.as_number(), Some(42.0), "{streaming:?}");
    }

    #[test]
    fn malformed_ok_end_bails_instead_of_ignoring_the_missing_try_frame() {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = plain_vm(&mut host);

        vm.handle_ok_end();

        assert!(matches!(
            vm.bx.threads.cur_ref().trap.get_on(),
            Some(ScriptTrapOn::Bail(_))
        ));
    }
}
