use crate::heap::*;
use crate::json::*;
use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::pod::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct StackBases {
    pub loops: usize,
    pub tries: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
    pub slots: usize,
}

#[derive(Debug)]
pub struct LoopValues {
    pub value_id: LiveId,
    pub key_id: Option<LiveId>,
    pub index_id: Option<LiveId>,
    pub source: ScriptValue,
    pub index: f64,
    /// (end, step) captured at loop entry for range sources the script can
    /// no longer reach (un-REFFED range literals like `for i in 0..n`) —
    /// skips two proto-chain lookups per iteration. REFFED ranges keep the
    /// live per-iteration lookups since script code could mutate them.
    pub range_cache: Option<(f64, f64)>,
}

#[derive(Debug)]
pub struct TryFrame {
    pub push_nil: bool,
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

#[derive(Debug)]
pub struct LoopFrame {
    pub values: Option<LoopValues>,
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

pub struct CallFrame {
    pub bases: StackBases,
    pub args: OpcodeArgs,
    pub return_ip: Option<ScriptIp>,
    /// slot_base of the CALLER, restored when this frame is popped.
    /// The callee's own frame is established by SLOTS_FRAME (slot-compiled
    /// fns only); non-slotted fns never touch slot_base.
    pub prev_slot_base: usize,
}

#[derive(Debug)]
pub enum ScriptMe {
    Object(ScriptObject),
    Call {
        sself: Option<ScriptValue>,
        args: ScriptObject,
        /// When set, this is a dynamic dispatch via the type's call function.
        /// The method name is passed to the call handler at exec time.
        method: Option<LiveId>,
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

#[derive(Default, Debug, Clone, Copy)]
pub struct ScriptThreadId(pub(crate) u32);

impl ScriptThreadId {
    pub fn to_index(&self) -> usize {
        self.0 as usize
    }
}

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
    pub(crate) mes: Vec<ScriptMe>,
    /// Resource-limit signals are consumed by the VM loop so these failures
    /// cannot enter script-level `try` recovery.
    pub(crate) stack_limit_exceeded: bool,
    pub(crate) call_frame_limit_exceeded: bool,
    /// Frame-local variable slots for slot-compiled fn bodies. Lexically
    /// resolved locals live here as `slots[slot_base + i]` instead of in
    /// scope-object maps; GC marks the whole slab as roots.
    pub(crate) slots: Vec<ScriptValue>,
    /// Base of the CURRENT slot frame (see CallFrame::prev_slot_base).
    pub(crate) slot_base: usize,
    pub(crate) instruction_limit_remaining: Option<usize>,
    /// The innermost scope when the root frame returned, kept alive so
    /// the body can hand it to host->script calls.
    pub(crate) root_end_scope: Option<ScriptObjectRef>,
    pub trap: ScriptTrapInner,
    //pub(crate) last_err: ScriptValue,
    pub(crate) json_parser: JsonParserThread,
    pub(crate) thread_id: ScriptThreadId,
}

impl ScriptThread {
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
            mes: Vec::with_capacity(64),
            stack_limit_exceeded: false,
            call_frame_limit_exceeded: false,
            slots: Vec::with_capacity(256),
            slot_base: 0,
            instruction_limit_remaining: None,
            trap: ScriptTrapInner::default(),
            json_parser: Default::default(),
        }
    }

    pub fn new_bases(&self) -> StackBases {
        StackBases {
            tries: self.tries.len(),
            loops: self.loops.len(),
            stack: self.stack.len(),
            scope: self.scopes.len(),
            mes: self.mes.len(),
            slots: self.slots.len(),
        }
    }

    pub fn pause(&mut self) -> ScriptThreadId {
        self.trap.set_on(Some(ScriptTrapOn::Pause));
        self.is_paused = true;
        self.thread_id
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused
    }

    /// What this thread is rooting for the collector, as sizes: the value
    /// stack, the frame slots, the scope chain, the method contexts, the
    /// loop frames. A leak hunt reads these across events.
    pub fn root_footprint(&self) -> [usize; 5] {
        [self.stack.len(), self.slots.len(), self.scopes.len(), self.mes.len(), self.loops.len()]
    }

    pub fn thread_id(&self) -> ScriptThreadId {
        self.thread_id
    }

    pub fn truncate_bases(&mut self, bases: StackBases, heap: &mut ScriptHeap) {
        self.tries.truncate(bases.tries);
        self.loops.truncate(bases.loops);
        self.stack.truncate(bases.stack);
        self.free_unreffed_scopes(&bases, heap);
        self.mes.truncate(bases.mes);
        self.slots.truncate(bases.slots);
    }

    /// Loop back-edge cleanup: discard iteration-local try frames, operand
    /// values and call-builder (`mes`) state. The loop frame itself and the
    /// scopes are left to the loop's own iteration-scope reset so a plain
    /// `loop` keeps its iteration scope and `for` can reuse its scope object.
    pub fn truncate_loop_iteration_bases(&mut self, bases: StackBases) {
        self.tries.truncate(bases.tries);
        self.stack.truncate(bases.stack);
        self.mes.truncate(bases.mes);
    }

    /// Read a slot of the current frame. Bounds-checked: the parser only
    /// emits in-frame indices, but stay safe and trap instead of panicking.
    #[inline]
    pub fn slot(&mut self, index: u32) -> ScriptValue {
        if let Some(v) = self.slots.get(self.slot_base + index as usize) {
            return *v;
        }
        script_err_stack!(self.trap, "slot {} out of frame", index)
    }

    #[inline]
    pub fn set_slot(&mut self, index: u32, value: ScriptValue) {
        let at = self.slot_base + index as usize;
        if let Some(v) = self.slots.get_mut(at) {
            *v = value;
        } else {
            script_err_stack!(self.trap, "slot {} out of frame", index);
        }
    }

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
            script_err_stack!(self.trap, "pop_stack_resolved on empty stack")
        }
    }

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

    pub fn peek_stack_value(&mut self) -> ScriptValue {
        if let Some(value) = self.stack.last() {
            return *value;
        } else {
            script_err_stack!(self.trap, "peek_stack_value on empty stack")
        }
    }

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

    pub fn pop_stack_value(&mut self) -> ScriptValue {
        if let Some(value) = self.stack.pop() {
            return value;
        } else {
            script_err_stack!(self.trap, "pop_stack_value on empty stack")
        }
    }

    pub fn push_stack_value(&mut self, value: ScriptValue) {
        self.push_stack_unchecked(value);
    }

    #[inline]
    pub fn push_stack_unchecked(&mut self, value: ScriptValue) {
        // This is the common operand-push path used by opcode handlers. Its
        // historical name distinguishes it from identifier resolution, not
        // from resource accounting.
        if self.stack.len() >= self.stack_limit {
            self.stack_limit_exceeded = true;
            return;
        }
        self.stack.push(value);
    }

    pub(crate) fn push_call_frame(&mut self, call: CallFrame) -> bool {
        if self
            .call_frame_limit
            .is_some_and(|limit| self.calls.len() >= limit)
        {
            self.call_frame_limit_exceeded = true;
            return false;
        }
        self.calls.push(call);
        true
    }

    pub(crate) fn take_stack_limit_exceeded(&mut self) -> bool {
        std::mem::take(&mut self.stack_limit_exceeded)
    }

    pub(crate) fn take_call_frame_limit_exceeded(&mut self) -> bool {
        std::mem::take(&mut self.call_frame_limit_exceeded)
    }

    pub(crate) fn has_execution_limit_exceeded(&self) -> bool {
        self.stack_limit_exceeded || self.call_frame_limit_exceeded
    }

    pub fn call_has_me(&self) -> bool {
        self.calls
            .last()
            .map(|call| self.mes.len() > call.bases.mes)
            .unwrap_or(false)
    }

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
    /// the Rust stack under a native call (`array.retain(fn)` calls back on
    /// this same thread); unwinding into them from here would pop frames that
    /// outer loop is still executing.
    pub(crate) fn call_stack_has_try(&self) -> bool {
        for call in self.calls.iter().rev() {
            if self.tries.len() > call.bases.tries {
                return true;
            }
            if call.return_ip.is_none() {
                return false;
            }
        }
        false
    }

    // lets resolve an id to a ScriptValue
    pub fn scope_value(&mut self, heap: &ScriptHeap, id: LiveId) -> ScriptValue {
        heap.scope_value(*self.scopes.last().unwrap(), id.into(), self.trap.pass())
    }

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

    pub fn def_scope_value(&mut self, heap: &mut ScriptHeap, id: LiveId, value: ScriptValue) {
        // alright if we are shadowing a value, we need to make a new scope
        if let Some(new_scope) = heap.def_scope_value(*self.scopes.last().unwrap(), id, value) {
            self.scopes.push(new_scope);
        }
    }
}

/// Wrapper around Vec<ScriptThread> with a current thread index.
/// This allows accessing the current thread without borrowing the entire container,
/// enabling split borrows between threads and other fields like heap/code.
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
    #[inline(always)]
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
    #[inline(always)]
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
