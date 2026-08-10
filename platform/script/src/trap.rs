use crate::makepad_script_derive::*;
use crate::value::*;
use std::cell::RefCell;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct ScriptError {
    pub message: String,
    pub origin_file: String,
    pub origin_line: u32,
    pub value: ScriptValue,
}

#[derive(Debug, Clone, Copy)]
pub enum ScriptTrapOn {
    Pause,
    TimeBudgetYield,
    Return(ScriptValue),
    Bail(ScriptValue),
}
use std::cell::Cell;

/// Bit set in `pending` when the error queue is non-empty.
pub const TRAP_PENDING_ERR: u8 = 1;
/// Bit set in `pending` when `on` holds a trap (pause/return/bail/yield).
pub const TRAP_PENDING_ON: u8 = 2;

#[derive(Default, Debug)]
pub struct ScriptTrapInner {
    err: RefCell<VecDeque<ScriptError>>,
    on: Cell<Option<ScriptTrapOn>>,
    /// Mirror of "err non-empty" / "on set" as a bitfield so the interpreter
    /// loop polls both interrupt sources with a single Cell load per
    /// instruction. Only the accessors below may touch `err`/`on` — they
    /// keep this in sync.
    pending: Cell<u8>,
    pub ip: ScriptIp,
}

impl ScriptTrapInner {
    /// One-load poll for the interpreter loop: non-zero when there are
    /// pending errors (TRAP_PENDING_ERR) and/or a trap to handle
    /// (TRAP_PENDING_ON).
    #[inline(always)]
    pub fn pending(&self) -> u8 {
        self.pending.get()
    }

    #[inline(always)]
    pub fn has_err(&self) -> bool {
        self.pending.get() & TRAP_PENDING_ERR != 0
    }

    /// Take and clear the whole error queue.
    pub fn err_take(&self) -> VecDeque<ScriptError> {
        self.pending
            .set(self.pending.get() & !TRAP_PENDING_ERR);
        self.err.take()
    }

    /// Drop all pending errors.
    pub fn err_clear(&self) {
        self.pending
            .set(self.pending.get() & !TRAP_PENDING_ERR);
        self.err.borrow_mut().clear();
    }

    pub fn err_pop_front(&self) -> Option<ScriptError> {
        let mut err = self.err.borrow_mut();
        let r = err.pop_front();
        if err.is_empty() {
            self.pending
                .set(self.pending.get() & !TRAP_PENDING_ERR);
        }
        r
    }

    pub fn err_is_empty(&self) -> bool {
        !self.has_err()
    }

    /// Read-only view for diagnostics/GC marking.
    pub fn err_borrow(&self) -> std::cell::Ref<'_, VecDeque<ScriptError>> {
        self.err.borrow()
    }

    // `on` accessors — the trap slot shares the pending bitfield

    pub fn set_on(&self, on: Option<ScriptTrapOn>) {
        if on.is_some() {
            self.pending.set(self.pending.get() | TRAP_PENDING_ON);
        } else {
            self.pending.set(self.pending.get() & !TRAP_PENDING_ON);
        }
        self.on.set(on);
    }

    #[inline]
    pub fn get_on(&self) -> Option<ScriptTrapOn> {
        self.on.get()
    }

    pub fn take_on(&self) -> Option<ScriptTrapOn> {
        self.pending.set(self.pending.get() & !TRAP_PENDING_ON);
        self.on.take()
    }

    #[inline]
    pub fn on_is_none(&self) -> bool {
        self.pending.get() & TRAP_PENDING_ON == 0
    }
}

#[derive(Clone, Copy)]
pub enum ScriptTrap<'a> {
    NoTrap,
    Inner(&'a ScriptTrapInner),
}

pub use ScriptTrap::NoTrap;

impl<'a> ScriptTrap<'a> {
    pub fn pass(self) -> Self {
        self
    }
}

impl ScriptTrapInner {
    pub fn pass<'a>(&'a self) -> ScriptTrap<'a> {
        ScriptTrap::Inner(self)
    }
}

impl ScriptTrapInner {
    pub fn push_err(
        &self,
        value: ScriptValue,
        message: String,
        origin_file: String,
        origin_line: u32,
    ) -> ScriptValue {
        self.pending.set(self.pending.get() | TRAP_PENDING_ERR);
        self.err.borrow_mut().push_back(ScriptError {
            value,
            message,
            origin_file,
            origin_line,
        });
        value
    }
    pub fn ip(&self) -> u32 {
        self.ip.index
    }
    pub fn goto(&mut self, wh: u32) {
        self.ip.index = wh;
    }
    pub fn goto_rel(&mut self, wh: u32) {
        self.ip.index += wh;
    }
    #[inline]
    pub fn goto_next(&mut self) {
        self.ip.index += 1;
    }
}

// Consolidated error macros (19 total, down from 56)
script_err_gen!(script_err_not_found); // lookup failures
script_err_gen!(script_err_type_mismatch); // wrong type for operation
script_err_gen!(script_err_wrong_value); // expected different kind
script_err_gen!(script_err_out_of_bounds); // index/bounds errors
script_err_gen!(script_err_immutable); // cannot modify
script_err_gen!(script_err_stack); // stack errors
script_err_gen!(script_err_invalid_args); // argument errors
script_err_gen!(script_err_not_allowed); // operation not allowed
script_err_gen!(script_err_inconsistent); // types don't match across branches
script_err_gen!(script_err_not_impl); // not implemented
script_err_gen!(script_err_unexpected); // catch-all
script_err_gen!(script_err_assert_fail); // assertions
script_err_gen!(script_err_user); // user-generated
script_err_gen!(script_err_pod); // all pod errors
script_err_gen!(script_err_shader); // all shader errors
script_err_gen!(script_err_unknown_type); // type not registered
script_err_gen!(script_err_duplicate); // key already exists
script_err_gen!(script_err_io); // file system, child process
script_err_gen!(script_err_limit); // resource limits
