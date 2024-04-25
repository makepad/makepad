use {
    crate::aliased_box::AliasableBox,
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
    pub(crate) const SIZE: usize = 1024 * 1024;

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
            slots: AliasableBox::from_box(Box::from(vec![0; Self::SIZE])),
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

pub(crate) type StackSlot = u64;

thread_local! {
    static STACK: Cell<Option<Stack>> = Cell::new(Some(Stack::new()));
}
