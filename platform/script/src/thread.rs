use relax_api::relax_api;
use crate::heap::*;
use crate::json::*;
use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::pod::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

/// The operand stack limit ([`ScriptThread::stack_limit`]) was reached.
pub(crate) const SIGNAL_OPERAND_STACK: u8 = 1;
/// The call frame limit ([`ScriptThread::call_frame_limit`]) was reached.
pub(crate) const SIGNAL_CALL_FRAMES: u8 = 2;
/// A thread vector could not grow ([`ScriptThread::growth_refusal`]).
pub(crate) const SIGNAL_GROWTH: u8 = 4;

pub(crate) const GROWING_THE_OPERAND_STACK: &str = "out of memory while growing the operand stack";
pub(crate) const GROWING_THE_CALL_STACK: &str = "out of memory while growing the call stack";

/// Push `value` if `v` has or can get room: one compare while there is room (the
/// push itself checks nothing more), an out-of-line fallible grow when full.
/// False when the grow is refused; `value` is dropped.
#[inline(always)]
fn try_push<T>(v: &mut Vec<T>, value: T) -> bool {
    let len = v.len();
    if len == v.capacity() && !grow_one(v) {
        return false;
    }
    // SAFETY: `len < capacity` now holds, so slot `len` is allocated and unused;
    // it is written before the length covers it.
    unsafe {
        v.as_mut_ptr().add(len).write(value);
        v.set_len(len + 1);
    }
    true
}

/// The amortised (doubling) grow behind [`try_push`].
#[cold]
#[inline(never)]
fn grow_one<T>(v: &mut Vec<T>) -> bool {
    v.try_reserve(1).is_ok()
}

#[relax_api(plain, run)]
#[derive(Clone, Copy, Debug, Default)]
pub struct StackBases {
    pub loops: usize,
    pub tries: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
    pub slots: usize,
    pub native_conts: usize,
}

#[relax_api(plain, run)]
#[derive(Debug)]
pub struct LoopValues {
    pub value_id: LiveId,
    pub key_id: Option<LiveId>,
    pub index_id: Option<LiveId>,
    pub source: ScriptValue,
    pub index: f64,
    /// Iterations begun before this one (0 on the first): the position in the iterated
    /// sequence, where `index` is a range's value (`for i in 2..5` has index 2 at position 0).
    pub pos: u32,
    /// (end, step) captured at loop entry for range sources the script can
    /// no longer reach (un-REFFED range literals like `for i in 0..n`) —
    /// skips two proto-chain lookups per iteration. REFFED ranges keep the
    /// live per-iteration lookups since script code could mutate them.
    pub range_cache: Option<(f64, f64)>,
}

#[relax_api(plain, run)]
#[derive(Debug)]
pub struct TryFrame {
    pub push_nil: bool,
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

#[relax_api(plain, run)]
#[derive(Debug)]
pub struct LoopFrame {
    pub values: Option<LoopValues>,
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

#[relax_api(plain, run)]
pub struct CallFrame {
    pub bases: StackBases,
    pub args: OpcodeArgs,
    pub return_ip: Option<ScriptIp>,
    /// slot_base of the CALLER, restored when this frame is popped.
    /// The callee's own frame is established by SLOTS_FRAME (slot-compiled
    /// fns only); non-slotted fns never touch slot_base.
    pub prev_slot_base: usize,
    /// The root frame of a script body run (not a host->fn call): its
    /// return records the scope the body ended in (`ScriptBody::end_scope`).
    pub body_root: bool,
    /// A script callback whose result a suspended native consumes.
    pub native_cont: bool,
}

impl CallFrame {
    pub(crate) fn call_site(&self) -> Option<ScriptIp> {
        self.return_ip.map(|ip| if self.native_cont { ip } else {
            ScriptIp { index: ip.index.saturating_sub(1), ..ip }
        })
    }
}

pub(crate) struct NativeCont {
    pub keep: [ScriptValue; 4],
    pub ret: Option<ScriptValue>,
}

/// A suspended callback keeps its admitted target. A fresh call or a native's own
/// pause resolves the original method at the call opcode.
#[derive(Clone, Copy, Debug)]
pub enum CallTarget {
    Resolve(Option<LiveId>),
    Admitted(OpcodeNative),
}

#[derive(Clone, Copy, Debug)]
pub enum OpcodeNative {
    Function(NativeId),
    TypeCall { handler: usize, ty: ScriptTypeRedux, method: LiveId },
}

#[derive(Debug)]
pub enum ScriptMe {
    Object(ScriptObject),
    Call {
        sself: Option<ScriptValue>,
        args: ScriptObject,
        target: CallTarget,
    },
    Pod {
        pod: ScriptPod,
        offset: ScriptPodOffset,
    },
    Array(ScriptArray),
}

impl Into<ScriptValue> for ScriptMe {
    fn into(self) -> ScriptValue {
        match self {
            Self::Object(v) => v.into(),
            Self::Call { args, .. } => args.into(),
            Self::Pod { pod, .. } => pod.into(),
            Self::Array(v) => v.into(),
        }
    }
}

#[relax_api(plain, run)]
#[derive(Default, Debug, Clone, Copy)]
#[repr(transparent)]
pub struct ScriptThreadId(pub(crate) u32);

impl ScriptThreadId {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

#[relax_api(handle, run)]
#[allow(unused)]
pub struct ScriptThread {
    pub(crate) is_paused: bool,
    /// Maximum live operand values for this thread. The raw Makepad VM keeps
    /// its historical one-million-value ceiling; constrained hosts can lower
    /// it for one evaluation through `ScriptVm::with_stack_value_limit`.
    pub(crate) stack_limit: usize,
    /// Optional maximum number of active VM call frames, including a root
    /// evaluation frame. The raw VM leaves this unbounded; constrained hosts
    /// install a limit for their evaluation.
    pub(crate) call_frame_limit: Option<usize>,
    pub(crate) tries: Vec<TryFrame>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ScriptObject>,
    pub(crate) stack: Vec<ScriptValue>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) native_conts: Vec<NativeCont>,
    pub(crate) mes: Vec<ScriptMe>,
    /// Resource-limit signals ([`SIGNAL_OPERAND_STACK`], [`SIGNAL_CALL_FRAMES`],
    /// [`SIGNAL_GROWTH`]) are consumed by the VM loop so these failures cannot
    /// enter script-level `try` recovery. One byte, so the per-instruction poll
    /// is one compare.
    pub(crate) limit_signals: u8,
    /// Why a thread vector could not grow ([`SIGNAL_GROWTH`]); the VM loop hands it
    /// to the heap's memory refusal.
    pub(crate) growth_refusal: Option<&'static str>,
    /// Frame-local variable slots for slot-compiled fn bodies. Lexically
    /// resolved locals live here as `slots[slot_base + i]` instead of in
    /// scope-object maps; GC marks the whole slab as roots.
    pub(crate) slots: Vec<ScriptValue>,
    /// Base of the CURRENT slot frame (see CallFrame::prev_slot_base).
    pub(crate) slot_base: usize,
    pub(crate) instruction_limit_remaining: Option<u64>,
    /// Real limit scopes sharing this counter. At depth zero it only paces interrupt samples.
    pub(crate) instruction_limit_depth: usize,
    /// The counter and whether it enforces a limit, reapplied only while the run resumes.
    pub(crate) paused_instruction_limit: Option<(u64, bool)>,
    /// The innermost scope when the root frame returned, kept alive so
    /// the body can hand it to host->script calls.
    pub(crate) root_end_scope: Option<ScriptObjectRef>,
    pub trap: ScriptTrapInner,
    //pub(crate) last_err: ScriptValue,
    pub(crate) json_parser: JsonParserThread,
    pub(crate) thread_id: ScriptThreadId,
    /// Edit mode, while a program loads: the names read and where (see
    /// `crate::literal::LoadTrace::name_reads`). None otherwise.
    pub(crate) name_reads: Option<Box<crate::literal::NameReads>>,
    /// A sliced run ([`crate::vm::ScriptRunBudget::slice`]) stopped here at its soft deadline,
    /// between two instructions; `resume` continues it.
    pub(crate) yielded: bool,
    /// What the entries a yielded run returned through still owe its end, innermost first:
    /// done by the `resume` that completes the run.
    pub(crate) completions: Vec<RunCompletion>,
}

/// An entry's work after its run, owed by a sliced run that yielded through it
/// ([`ScriptThread::completions`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum RunCompletion {
    /// `run_root`: the body keeps the scope its root frame ended in.
    RootEnd { body_id: u16 },
    /// `eval_body`: the body counts as evaluated and its result as an eval's.
    EvalBody { body_id: u16 },
    /// `eval_with_append_source`: the run is silenced like a streaming one and its result is an eval's.
    Streaming,
}

impl ScriptThread {
    /// A host can install a policy while a thread is suspended. Its existing
    /// stack must fit too, even if its first resumed instruction only pops.
    pub(crate) fn policy_stack_failure(&self, policy: &crate::policy::ScriptVmPolicy) -> Option<crate::policy::Limit> {
        if self.limit_signals & SIGNAL_OPERAND_STACK != 0 || policy.stack_values.is_some_and(|cap| self.stack.len() > cap) {
            Some(crate::policy::Limit::OperandStack)
        } else if self.limit_signals & SIGNAL_CALL_FRAMES != 0 || policy.call_frames.is_some_and(|cap| self.calls.len() > cap) {
            Some(crate::policy::Limit::CallFrames)
        } else {
            None
        }
    }

    pub fn new(thread_id: ScriptThreadId) -> Self {
        Self {
            thread_id,
            is_paused: false,
            root_end_scope: None,
            //last_err: NIL,
            // pre-reserve the hot stacks so steady-state execution never
            // pays Vec growth in the interpreter loop
            scopes: Vec::with_capacity(64),
            tries: vec![],
            stack_limit: 1_000_000,
            call_frame_limit: None,
            loops: Vec::with_capacity(16),
            stack: Vec::with_capacity(1024),
            calls: Vec::with_capacity(64),
            native_conts: Vec::new(),
            mes: Vec::with_capacity(64),
            limit_signals: 0,
            growth_refusal: None,
            slots: Vec::with_capacity(256),
            slot_base: 0,
            instruction_limit_remaining: None,
            instruction_limit_depth: 0,
            paused_instruction_limit: None,
            trap: ScriptTrapInner::default(),
            json_parser: Default::default(),
            name_reads: None,
            yielded: false,
            completions: Vec::new(),
        }
    }

    /// Roots held by a native between callback steps, including a result awaiting delivery.
    pub(crate) fn continuation_roots(&self) -> impl Iterator<Item = ScriptValue> + '_ {
        self.native_conts.iter().flat_map(|c| c.keep.into_iter().chain(c.ret))
    }

    pub fn new_bases(&self) -> StackBases {
        StackBases {
            tries: self.tries.len(),
            loops: self.loops.len(),
            stack: self.stack.len(),
            scope: self.scopes.len(),
            mes: self.mes.len(),
            slots: self.slots.len(),
            native_conts: self.native_conts.len(),
        }
    }

    pub(crate) fn real_instruction_remaining(&self) -> Option<u64> {
        self.instruction_limit_remaining.filter(|_| self.instruction_limit_depth != 0)
    }

    pub(crate) fn save_instruction_counter(&mut self) {
        self.paused_instruction_limit = self.instruction_limit_remaining
            .map(|remaining| (remaining, self.instruction_limit_depth != 0));
    }

    pub fn pause(&mut self) -> ScriptThreadId {
        self.save_instruction_counter();
        self.trap.set_on(Some(ScriptTrapOn::Pause));
        self.is_paused = true;
        self.thread_id
    }

    #[inline]
    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    /// The run on this thread returned before its end: it yielded at a sliced run's deadline,
    /// or a native paused it. The entries it returned through owe their ends to its `resume`.
    #[inline]
    pub(crate) fn run_unfinished(&self) -> bool {
        self.yielded || self.is_paused
    }

    /// What this thread is rooting for the collector, as sizes: the value
    /// stack, the frame slots, the scope chain, the method contexts, the
    /// loop frames. A leak hunt reads these across events.
    #[inline]
    pub fn root_footprint(&self) -> [usize; 5] {
        [self.stack.len(), self.slots.len(), self.scopes.len(), self.mes.len(), self.loops.len()]
    }

    #[inline]
    pub fn thread_id(&self) -> ScriptThreadId {
        self.thread_id
    }

    #[inline]
    pub fn truncate_bases(&mut self, bases: StackBases, heap: &mut ScriptHeap) {
        self.tries.truncate(bases.tries);
        self.loops.truncate(bases.loops);
        self.stack.truncate(bases.stack);
        self.free_unreffed_scopes(&bases, heap);
        self.mes.truncate(bases.mes);
        self.native_conts.truncate(bases.native_conts);
        self.slots.truncate(bases.slots);
    }

    /// Loop back-edge cleanup: discard iteration-local try frames, operand
    /// values and call-builder (`mes`) state. The loop frame itself and the
    /// scopes are left to the loop's own iteration-scope reset so a plain
    /// `loop` keeps its iteration scope and `for` can reuse its scope object.
    #[inline]
    pub fn truncate_loop_iteration_bases(&mut self, bases: StackBases) {
        self.tries.truncate(bases.tries);
        self.stack.truncate(bases.stack);
        self.mes.truncate(bases.mes);
        self.native_conts.truncate(bases.native_conts);
    }

    /// Read a slot of the current frame. Bounds-checked: the parser only
    /// emits in-frame indices, but stay safe and trap instead of panicking.
    #[inline]
    pub fn slot(&mut self, index: u32) -> ScriptValue {
        if let Some(v) = self.slots.get(self.slot_base + index as usize) {
            return *v;
        }
        self.slot_out_of_frame(index)
    }

    #[inline]
    pub fn set_slot(&mut self, index: u32, value: ScriptValue) {
        let at = self.slot_base + index as usize;
        if let Some(v) = self.slots.get_mut(at) {
            *v = value;
        } else {
            self.slot_out_of_frame(index);
        }
    }

    // The error paths of the operand and slot accessors stay out of line,
    // so the accessors themselves inline into every opcode handler.
    #[cold]
    #[inline(never)]
    fn slot_out_of_frame(&mut self, index: u32) -> ScriptValue {
        script_err_stack!(self.trap, "slot {} out of frame", index)
    }

    #[cold]
    #[inline(never)]
    fn pop_empty_stack(&mut self, what: &'static str) -> ScriptValue {
        script_err_stack!(self.trap, "{} on empty stack", what)
    }

    #[inline]
    pub fn free_unreffed_scopes(&mut self, bases: &StackBases, heap: &mut ScriptHeap) {
        while self.scopes.len() > bases.scope {
            let scope = self.scopes.pop().unwrap();
            // Eager scope reclamation at call/loop exit. Safe: a scope
            // captured by a closure became that closure's proto (REFFED in
            // new_with_proto), a host-held scope is REFFED via
            // new_object_ref, and a scope stored as a value went through the
            // escape barrier — all three make this a no-op and defer to GC.
            heap.free_object_if_unreffed(scope);
        }
    }

    #[inline]
    pub fn pop_stack_resolved(&mut self, heap: &ScriptHeap) -> ScriptValue {
        if let Some(val) = self.stack.pop() {
            if let Some(id) = val.as_id() {
                if val.is_escaped_id() {
                    return val;
                }
                return self.scope_value(heap, id);
            }
            return val;
        } else {
            self.pop_empty_stack("pop_stack_resolved")
        }
    }

    #[inline]
    pub fn peek_stack_resolved(&mut self, heap: &ScriptHeap) -> ScriptValue {
        if let Some(val) = self.stack.last() {
            if let Some(id) = val.as_id() {
                if val.is_escaped_id() {
                    return *val;
                }
                return self.scope_value(heap, id);
            }
            return *val;
        } else {
            script_err_stack!(self.trap, "peek_stack_resolved on empty stack")
        }
    }

    #[inline]
    pub fn peek_stack_value(&mut self) -> ScriptValue {
        if let Some(value) = self.stack.last() {
            return *value;
        } else {
            script_err_stack!(self.trap, "peek_stack_value on empty stack")
        }
    }

    #[inline]
    pub fn peek_stack_value_at(&mut self, offset: usize) -> ScriptValue {
        let len = self.stack.len();
        if offset < len {
            return self.stack[len - 1 - offset];
        } else {
            script_err_stack!(
                self.trap,
                "peek at offset {} exceeds stack len {}",
                offset,
                len
            )
        }
    }

    #[inline]
    pub fn pop_stack_value(&mut self) -> ScriptValue {
        if let Some(value) = self.stack.pop() {
            return value;
        } else {
            self.pop_empty_stack("pop_stack_value")
        }
    }

    #[inline]
    pub fn push_stack_value(&mut self, value: ScriptValue) {
        self.push_stack_unchecked(value);
    }

    #[inline]
    pub fn push_stack_unchecked(&mut self, value: ScriptValue) {
        // This is the common operand-push path used by opcode handlers. Its
        // historical name distinguishes it from identifier resolution, not
        // from resource accounting.
        if self.stack.len() >= self.stack_limit {
            self.limit_signals |= SIGNAL_OPERAND_STACK;
            return;
        }
        if !try_push(&mut self.stack, value) {
            self.refuse_growth(GROWING_THE_OPERAND_STACK);
        }
    }

    #[inline]
    pub(crate) fn push_call_frame(&mut self, call: CallFrame) -> bool {
        if self
            .call_frame_limit
            .is_some_and(|limit| self.calls.len() >= limit)
        {
            self.limit_signals |= SIGNAL_CALL_FRAMES;
            return false;
        }
        try_push(&mut self.calls, call) || self.refuse_growth(GROWING_THE_CALL_STACK)
    }

    /// The pushes of a frame's bookkeeping (scopes, receivers, loops, tries, slots).
    /// False when the vector could not grow: the push is dropped and the VM loop
    /// ends the run with the memory refusal before another instruction.
    #[inline]
    pub(crate) fn push_scope(&mut self, scope: ScriptObject) -> bool {
        try_push(&mut self.scopes, scope) || self.refuse_growth(GROWING_THE_CALL_STACK)
    }

    #[inline]
    pub(crate) fn push_me(&mut self, me: ScriptMe) -> bool {
        try_push(&mut self.mes, me) || self.refuse_growth(GROWING_THE_CALL_STACK)
    }

    #[inline]
    pub(crate) fn push_loop(&mut self, frame: LoopFrame) -> bool {
        try_push(&mut self.loops, frame) || self.refuse_growth(GROWING_THE_CALL_STACK)
    }

    #[inline]
    pub(crate) fn push_try(&mut self, frame: TryFrame) -> bool {
        try_push(&mut self.tries, frame) || self.refuse_growth(GROWING_THE_CALL_STACK)
    }

    /// Grow the slot slab to `len` (NIL-filled), or refuse like the pushes above.
    #[inline]
    pub(crate) fn grow_slots(&mut self, len: usize) -> bool {
        if len > self.slots.capacity() && self.slots.try_reserve(len - self.slots.len()).is_err() {
            return self.refuse_growth(GROWING_THE_CALL_STACK);
        }
        self.slots.resize(len, NIL);
        true
    }

    /// Record why a thread vector could not grow, for the VM loop. Always false.
    #[cold]
    #[inline(never)]
    pub(crate) fn refuse_growth(&mut self, reason: &'static str) -> bool {
        if self.growth_refusal.is_none() {
            self.growth_refusal = Some(reason);
        }
        self.limit_signals |= SIGNAL_GROWTH;
        false
    }

    #[inline]
    pub(crate) fn take_stack_limit_exceeded(&mut self) -> bool {
        self.take_signal(SIGNAL_OPERAND_STACK)
    }

    #[inline]
    pub(crate) fn take_call_frame_limit_exceeded(&mut self) -> bool {
        self.take_signal(SIGNAL_CALL_FRAMES)
    }

    #[inline]
    pub(crate) fn take_growth_refusal(&mut self) -> Option<&'static str> {
        self.take_signal(SIGNAL_GROWTH);
        self.growth_refusal.take()
    }

    #[inline]
    fn take_signal(&mut self, signal: u8) -> bool {
        let raised = self.limit_signals & signal != 0;
        self.limit_signals &= !signal;
        raised
    }

    #[inline]
    pub(crate) fn has_execution_limit_exceeded(&self) -> bool {
        self.limit_signals != 0
    }

    #[inline]
    pub fn call_has_me(&self) -> bool {
        self.calls
            .last()
            .map(|call| self.mes.len() > call.bases.mes)
            .unwrap_or(false)
    }

    #[inline]
    pub fn call_has_try(&self) -> bool {
        self.calls
            .last()
            .map(|call| self.tries.len() > call.bases.tries)
            .unwrap_or(false)
    }

    /// Whether a call frame of this run (not just the innermost) owns a try
    /// frame, so an error in a nested script call can unwind to it.
    ///
    /// The scan stops at the nearest root frame (`return_ip == None`). The
    /// frames below it belong to an outer `run_core` that is still live on
    /// the Rust stack under a native that called back with `vm.call`;
    /// unwinding into them from here would pop frames that outer loop is
    /// still executing. It also stops at a continuation frame (`native_cont`,
    /// `array.retain(fn)` and friends): that callback shares the interpreter,
    /// but its errors stay its own, as a nested run's do.
    #[inline]
    pub(crate) fn call_stack_has_try(&self) -> bool {
        for call in self.calls.iter().rev() {
            if self.tries.len() > call.bases.tries {
                return true;
            }
            if call.return_ip.is_none() || call.native_cont {
                return false;
            }
        }
        false
    }

    // lets resolve an id to a ScriptValue
    #[inline]
    pub fn scope_value(&mut self, heap: &ScriptHeap, id: LiveId) -> ScriptValue {
        if let Some(reads) = &mut self.name_reads {
            reads.record(id, self.trap.ip.body, self.trap.ip.index);
        }
        heap.scope_value(*self.scopes.last().unwrap(), id.into(), self.trap.pass())
    }

    #[inline]
    pub fn set_scope_value(
        &mut self,
        heap: &mut ScriptHeap,
        id: LiveId,
        value: ScriptValue,
    ) -> ScriptValue {
        heap.set_scope_value(
            *self.scopes.last().unwrap(),
            id.into(),
            value,
            self.trap.pass(),
        )
    }

    #[inline]
    pub fn def_scope_value(&mut self, heap: &mut ScriptHeap, id: LiveId, value: ScriptValue) {
        // alright if we are shadowing a value, we need to make a new scope
        if let Some(new_scope) = heap.def_scope_value(*self.scopes.last().unwrap(), id, value) {
            self.push_scope(new_scope);
        }
    }

    /// [`Self::def_scope_value`] for each of `items` in order (a glob `use`).
    pub fn def_scope_values(&mut self, heap: &mut ScriptHeap, mut items: &[(LiveId, ScriptValue)]) {
        while !items.is_empty() {
            let (taken, shadow) = heap.def_scope_values(*self.scopes.last().unwrap(), items);
            if let Some(scope) = shadow {
                if !self.push_scope(scope) { return; }
            }
            items = &items[taken..];
        }
    }
}

/// Wrapper around Vec<ScriptThread> with a current thread index.
/// This allows accessing the current thread without borrowing the entire container,
/// enabling split borrows between threads and other fields like heap/code.
#[relax_api(handle, run)]
pub struct ScriptThreads {
    threads: Vec<ScriptThread>,
    current: usize,
    /// Cached raw pointer to current thread for fast access in hot paths.
    /// SAFETY: This must be updated whenever `current` changes or threads are reallocated.
    cur_ptr: *mut ScriptThread,
}

impl ScriptThreads {
    pub fn new() -> Self {
        let mut threads = vec![ScriptThread::new(ScriptThreadId(0))];
        let cur_ptr = threads.as_mut_ptr();
        Self {
            threads,
            current: 0,
            cur_ptr,
        }
    }

    pub fn empty() -> Self {
        Self {
            threads: vec![],
            current: 0,
            cur_ptr: std::ptr::null_mut(),
        }
    }

    /// Update the cached pointer after any operation that might invalidate it
    #[inline(always)]
    fn update_ptr(&mut self) {
        // Bounds-checked: an invalid `current` yields a null pointer that the
        // accessors reject, never an out-of-bounds pointer.
        self.cur_ptr = self
            .threads
            .get_mut(self.current)
            .map_or(std::ptr::null_mut(), |thread| thread as *mut ScriptThread);
    }

    /// Get a mutable reference to the current thread using cached pointer
    /// SAFETY: The pointer is kept in sync with the current index and thread vector
    #[inline]
    pub fn cur(&mut self) -> &mut ScriptThread {
        assert!(
            !self.cur_ptr.is_null(),
            "current ScriptThread index is invalid"
        );
        unsafe { &mut *self.cur_ptr }
    }

    pub fn trap<'a>(&'a self) -> ScriptTrap<'a> {
        assert!(
            !self.cur_ptr.is_null(),
            "current ScriptThread index is invalid"
        );
        unsafe { (*self.cur_ptr).trap.pass() }
    }

    /// Get an immutable reference to the current thread using cached pointer
    /// SAFETY: The pointer is kept in sync with the current index and thread vector
    #[inline]
    pub fn cur_ref(&self) -> &ScriptThread {
        assert!(
            !self.cur_ptr.is_null(),
            "current ScriptThread index is invalid"
        );
        unsafe { &*self.cur_ptr }
    }

    /// Set which thread is current
    pub fn set_current(&mut self, id: usize) {
        assert!(
            id < self.threads.len(),
            "current ScriptThread index {id} is out of bounds for {} threads",
            self.threads.len()
        );
        self.current = id;
        self.update_ptr();
    }

    /// Get the current thread index
    pub fn current(&self) -> usize {
        self.current
    }

    /// Find the first unpaused thread (creating one if needed) and set it as current
    pub fn set_current_to_first_unpaused_thread(&mut self) {
        for (id, thread) in self.threads.iter().enumerate() {
            if !thread.is_paused {
                self.current = id;
                self.update_ptr();
                return;
            }
        }
        // No unpaused thread found, create a new one
        let id = self.threads.len();
        self.threads
            .push(ScriptThread::new(ScriptThreadId(id as u32)));
        self.current = id;
        self.update_ptr();
    }

    /// Set the current thread by ScriptThreadId
    pub fn set_current_thread_id(&mut self, thread_id: ScriptThreadId) {
        self.set_current(thread_id.to_index());
    }

    /// Get the number of threads
    pub fn len(&self) -> usize {
        self.threads.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    /// Push a new thread
    pub fn push(&mut self, thread: ScriptThread) {
        self.threads.push(thread);
        // push may reallocate, so update pointer
        self.update_ptr();
    }

    /// Get thread by index
    pub fn get(&self, index: usize) -> Option<&ScriptThread> {
        self.threads.get(index)
    }

    /// Get thread by index mutably
    pub fn get_mut(&mut self, index: usize) -> Option<&mut ScriptThread> {
        self.threads.get_mut(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "out of bounds")]
    fn selecting_an_unknown_current_thread_panics_before_pointer_update() {
        let mut threads = ScriptThreads::new();
        threads.set_current(1);
    }

    #[test]
    #[should_panic(expected = "index is invalid")]
    fn empty_threads_reject_current_access_in_release_builds() {
        let threads = ScriptThreads::empty();
        let _ = threads.cur_ref();
    }
}
