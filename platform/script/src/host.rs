//! Host capabilities belong to the compiler that owns the host. Only the
//! context and its C callback table cross the VM boundary.
use makepad_relax_api::relax_api;
use crate::{LiveId, ScriptVmBase};
use std::marker::PhantomData;

/// A stable capability name, not a compiler's TypeId.
///
/// # Safety
/// IDs must uniquely identify the pointee contract. A Rust-layout capability
/// may only be dereferenced by code compiled with its owner; foreign code must
/// treat it as opaque and use that owner's operations.
pub unsafe trait ScriptHostCapability {
    const ID: LiveId;
}

unsafe impl ScriptHostCapability for () {
    const ID: LiveId = LiveId::from_str("makepad.script.host.unit.v1");
}
unsafe impl ScriptHostCapability for i32 {
    const ID: LiveId = LiveId::from_str("makepad.script.host.i32.v1");
}
unsafe impl ScriptHostCapability for usize {
    const ID: LiveId = LiveId::from_str("makepad.script.host.usize.v1");
}

pub const SCRIPT_HOST_VM_BASE: LiveId = LiveId::from_str("makepad.script.host.vm_base.v1");

#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHostOps {
    query: unsafe extern "C-unwind" fn(*mut u8, LiveId, bool) -> *mut u8,
    take: unsafe extern "C-unwind" fn(*mut u8) -> *mut ScriptVmBase,
    restore: unsafe extern "C-unwind" fn(*mut u8, *mut ScriptVmBase) -> *mut ScriptVmBase,
}

/// An exclusive, non-owning borrow of the host. Reborrow it for nested calls;
/// never copy the descriptor or keep its capabilities past the borrow.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHostContext<'a> {
    owner: *mut u8,
    ops: &'static ScriptHostOps,
    borrow: PhantomData<&'a mut u8>,
}

/// Owner-side adapter. No trait object or Any vtable crosses the boundary.
///
/// # Safety
/// query returns null or a pointer to the named capability in this owner.
/// `standard` selects std state rather than the application host, including
/// when both roles use the same type. IDs must not alias VM_BASE.
/// Shared queries must not create exclusive references to existing borrows.
/// SCRIPT_HOST_VM_BASE is reserved. The VM slot and all capabilities outlive the context.
pub unsafe trait ScriptHostOwner: Sized {
    unsafe fn query(owner: *mut Self, id: LiveId, standard: bool) -> *mut u8;
    fn vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>>;

    const HOST_OPS: ScriptHostOps = ScriptHostOps {
        query: host_query::<Self>,
        take: host_take::<Self>,
        restore: host_restore::<Self>,
    };

    fn host_context(&mut self) -> ScriptHostContext<'_> {
        ScriptHostContext {
            owner: self as *mut Self as *mut u8,
            ops: &Self::HOST_OPS,
            borrow: PhantomData,
        }
    }
}

unsafe extern "C-unwind" fn host_query<H: ScriptHostOwner>(
    owner: *mut u8,
    id: LiveId,
    standard: bool,
) -> *mut u8 {
    H::query(owner.cast(), id, standard)
}
unsafe extern "C-unwind" fn host_take<H: ScriptHostOwner>(owner: *mut u8) -> *mut ScriptVmBase {
    (&mut *owner.cast::<H>())
        .vm_slot()
        .take()
        .map_or(std::ptr::null_mut(), Box::into_raw)
}
unsafe extern "C-unwind" fn host_restore<H: ScriptHostOwner>(
    owner: *mut u8,
    base: *mut ScriptVmBase,
) -> *mut ScriptVmBase {
    let base = if base.is_null() {
        None
    } else {
        Some(Box::from_raw(base))
    };
    std::mem::replace((&mut *owner.cast::<H>()).vm_slot(), base)
        .map_or(std::ptr::null_mut(), Box::into_raw)
}

impl ScriptHostContext<'_> {
    #[inline]
    pub fn reborrow(&mut self) -> ScriptHostContext<'_> {
        ScriptHostContext {
            owner: self.owner,
            ops: self.ops,
            borrow: PhantomData,
        }
    }

    // Generic typed access stays with the caller. Neither Any nor TypeId is
    // passed into compiled VM/std code.
    #[inline]
    pub fn get<T: ScriptHostCapability>(&self) -> Option<&T> {
        unsafe { ((self.ops.query)(self.owner, T::ID, false) as *const T).as_ref() }
    }
    #[inline]
    pub fn get_mut<T: ScriptHostCapability>(&mut self) -> Option<&mut T> {
        unsafe { ((self.ops.query)(self.owner, T::ID, false) as *mut T).as_mut() }
    }
    #[inline]
    pub fn std_ref<T: ScriptHostCapability>(&self) -> Option<&T> {
        unsafe { ((self.ops.query)(self.owner, T::ID, true) as *const T).as_ref() }
    }
    #[inline]
    pub fn std_mut<T: ScriptHostCapability>(&mut self) -> Option<&mut T> {
        unsafe { ((self.ops.query)(self.owner, T::ID, true) as *mut T).as_mut() }
    }
    #[inline]
    pub fn vm_base(&self) -> Option<&ScriptVmBase> {
        unsafe {
            ((self.ops.query)(self.owner, SCRIPT_HOST_VM_BASE, false) as *const ScriptVmBase)
                .as_ref()
        }
    }
    #[inline]
    pub fn take_vm(&mut self) -> Option<Box<ScriptVmBase>> {
        let ptr = unsafe { (self.ops.take)(self.owner) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { Box::from_raw(ptr) })
        }
    }
    /// Install a base, returning the previous slot to its caller. On unwind,
    /// inspect vm_base first so an already parked real VM is never replaced.
    #[inline]
    pub fn restore_vm(&mut self, base: Box<ScriptVmBase>) -> Option<Box<ScriptVmBase>> {
        let old = unsafe { (self.ops.restore)(self.owner, Box::into_raw(base)) };
        if old.is_null() {
            None
        } else {
            Some(unsafe { Box::from_raw(old) })
        }
    }
}

/// Small owner for embeddings without an application context.
pub struct ScriptVmHost<H = (), S = ()> {
    pub host: H,
    pub std: S,
    pub script_vm: Option<Box<ScriptVmBase>>,
}
impl<H, S> ScriptVmHost<H, S> {
    #[inline]
    pub fn new(host: H, std: S) -> Self {
        Self {
            host,
            std,
            script_vm: None,
        }
    }
}
unsafe impl<H: ScriptHostCapability, S: ScriptHostCapability> ScriptHostOwner
    for ScriptVmHost<H, S>
{
    unsafe fn query(owner: *mut Self, id: LiveId, standard: bool) -> *mut u8 {
        if !standard && id == H::ID {
            std::ptr::addr_of_mut!((*owner).host).cast()
        } else if standard && id == S::ID {
            std::ptr::addr_of_mut!((*owner).std).cast()
        } else if !standard && id == SCRIPT_HOST_VM_BASE {
            (*owner)
                .script_vm
                .as_deref()
                .map_or(std::ptr::null_mut(), |base| {
                    base as *const ScriptVmBase as *mut u8
                })
        } else {
            std::ptr::null_mut()
        }
    }
    fn vm_slot(&mut self) -> &mut Option<Box<ScriptVmBase>> {
        &mut self.script_vm
    }
}

impl<H: ScriptHostCapability, S: ScriptHostCapability> ScriptVmHost<H, S> {
    #[inline]
    pub fn host_context(&mut self) -> ScriptHostContext<'_> {
        ScriptHostOwner::host_context(self)
    }
}
