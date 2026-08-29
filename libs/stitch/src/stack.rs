use {
    crate::aliasable_box::AliasableBox,
    std::{
        cell::Cell,
        mem::ManuallyDrop,
        ops::{Deref, DerefMut},
        ptr,
    },
};

#[derive(Debug)]
pub struct Stack {
    slots: AliasableBox<[StackSlot]>,
    ptr: *mut StackSlot,
}

impl Stack {
    pub(crate) const SIZE: usize = 512 * 1024;

    pub fn lock() -> StackGuard {
        StackGuard {
            stack: ManuallyDrop::new(STACK.take().unwrap()),
        }
    }

    pub(crate) fn base_ptr(&mut self) -> *mut StackSlot {
        self.slots.as_mut_ptr() as *mut _
    }

    pub fn ptr(&mut self) -> *mut StackSlot {
        self.ptr
    }

    pub(crate) fn set_ptr(&mut self, ptr: *mut StackSlot) {
        self.ptr = ptr;
    }

    fn new() -> Self {
        let mut stack = Self {
            slots: AliasableBox::from_box(Box::from(vec![StackSlot::ZERO; Self::SIZE])),
            ptr: ptr::null_mut(),
        };
        stack.ptr = stack.slots.as_mut_ptr();
        stack
    }
}

#[derive(Debug)]
pub struct StackGuard {
    stack: ManuallyDrop<Stack>,
}

impl Deref for StackGuard {
    type Target = Stack;

    fn deref(&self) -> &Self::Target {
        &self.stack
    }
}

impl DerefMut for StackGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.stack
    }
}

impl Drop for StackGuard {
    fn drop(&mut self) {
        STACK.set(Some(unsafe { ManuallyDrop::take(&mut self.stack) }));
    }
}

/// A single stack slot.
///
/// Every Wasm value occupies exactly one stack slot, regardless of its type.
/// A slot is 16 bytes so that a `v128` value fits in one slot like every
/// other value type; this keeps the compiler's operand-index <-> stack-index
/// identity intact (no multi-slot "blocks" on the stack). Values smaller
/// than 16 bytes are stored at the start of their slot (little-endian).
/// The 16-byte alignment makes aligned `v128` slot accesses valid.
///
/// `Stack::SIZE` is halved relative to the old 8-byte slots so the stack
/// still occupies 8 MiB per thread.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(16))]
pub struct StackSlot([u64; 2]);

impl StackSlot {
    pub(crate) const ZERO: Self = Self([0; 2]);
}

thread_local! {
    static STACK: Cell<Option<Stack>> = Cell::new(Some(Stack::new()));
}
