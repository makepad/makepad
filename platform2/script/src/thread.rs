use crate::heap::*;
use crate::json::*;
use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::pod::*;
use crate::trap::*;
use crate::value::*;
use crate::*;

#[derive(Debug, Default)]
pub struct StackBases {
    pub loops: usize,
    pub tries: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
}

#[derive(Debug)]
pub struct LoopValues {
    pub value_id: LiveId,
    pub key_id: Option<LiveId>,
    pub index_id: Option<LiveId>,
    pub source: ScriptValue,
    pub index: f64,
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
}

#[derive(Debug)]
pub enum ScriptMe {
    Object(ScriptObject),
    Call {
        sself: Option<ScriptValue>,
        args: ScriptObject,
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
    pub(crate) stack_limit: usize,
    pub(crate) tries: Vec<TryFrame>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ScriptObject>,
    pub(crate) stack: Vec<ScriptValue>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
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
            //last_err: NIL,
            scopes: vec![],
            tries: vec![],
            stack_limit: 1_000_000,
            loops: vec![],
            stack: vec![],
            calls: vec![],
            mes: vec![],
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
        }
    }

    pub fn pause(&mut self) -> ScriptThreadId {
        self.trap.on.set(Some(ScriptTrapOn::Pause));
        self.is_paused = true;
        self.thread_id
    }

    pub fn truncate_bases(&mut self, bases: StackBases, heap: &mut ScriptHeap) {
        self.tries.truncate(bases.tries);
        self.loops.truncate(bases.loops);
        self.stack.truncate(bases.stack);
        self.free_unreffed_scopes(&bases, heap);
        self.mes.truncate(bases.mes);
    }

    pub fn free_unreffed_scopes(&mut self, bases: &StackBases, heap: &mut ScriptHeap) {
        while self.scopes.len() > bases.scope {
            let scope = self.scopes.pop().unwrap();
            heap.free_object_if_unreffed(scope); // DISABLED: investigating RootObject already freed
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
        if self.stack.len() > self.stack_limit {
            script_err_stack!(self.trap, "stack exceeded limit {}", self.stack_limit);
        } else {
            self.stack.push(value);
        }
    }

    #[inline]
    pub fn push_stack_unchecked(&mut self, value: ScriptValue) {
        self.stack.push(value);
    }

    pub fn call_has_me(&self) -> bool {
        self.mes.len() > self.calls.last().unwrap().bases.mes
    }

    pub fn call_has_try(&self) -> bool {
        self.tries.len() > self.calls.last().unwrap().bases.tries
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
        if !self.threads.is_empty() {
            self.cur_ptr = unsafe { self.threads.as_mut_ptr().add(self.current) };
        } else {
            self.cur_ptr = std::ptr::null_mut();
        }
    }

    /// Get a mutable reference to the current thread using cached pointer
    /// SAFETY: The pointer is kept in sync with the current index and thread vector
    #[inline(always)]
    pub fn cur(&mut self) -> &mut ScriptThread {
        debug_assert!(
            !self.cur_ptr.is_null(),
            "cur() called on empty ScriptThreads"
        );
        unsafe { &mut *self.cur_ptr }
    }

    pub fn trap<'a>(&'a self) -> ScriptTrap<'a> {
        debug_assert!(
            !self.cur_ptr.is_null(),
            "trap() called on empty ScriptThreads"
        );
        unsafe { (*self.cur_ptr).trap.pass() }
    }

    /// Get an immutable reference to the current thread using cached pointer
    /// SAFETY: The pointer is kept in sync with the current index and thread vector
    #[inline(always)]
    pub fn cur_ref(&self) -> &ScriptThread {
        debug_assert!(
            !self.cur_ptr.is_null(),
            "cur_ref() called on empty ScriptThreads"
        );
        unsafe { &*self.cur_ptr }
    }

    /// Set which thread is current
    pub fn set_current(&mut self, id: usize) {
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
        self.current = thread_id.to_index();
        self.update_ptr();
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
