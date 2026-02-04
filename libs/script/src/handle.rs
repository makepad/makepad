use crate::heap::*;
use crate::value::*;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;
use std::rc::Rc;

#[derive(Debug)]
pub struct ScriptHandleRef {
    pub(crate) roots: Rc<RefCell<HashMap<ScriptHandle, usize>>>,
    pub(crate) handle: ScriptHandle,
}

impl From<ScriptHandleRef> for ScriptValue {
    fn from(v: ScriptHandleRef) -> Self {
        ScriptValue::from_handle(v.as_handle())
    }
}

impl Clone for ScriptHandleRef {
    fn clone(&self) -> Self {
        let mut roots = self.roots.borrow_mut();
        match roots.entry(self.handle) {
            Entry::Occupied(mut occ) => {
                let value = occ.get_mut();
                *value += 1;
            }
            Entry::Vacant(_vac) => {
                eprintln!("ScriptHandleRef root is vacant!");
            }
        }
        Self {
            roots: self.roots.clone(),
            handle: self.handle.clone(),
        }
    }
}

impl ScriptHandleRef {
    pub fn as_handle(&self) -> ScriptHandle {
        self.handle
    }
}

impl Drop for ScriptHandleRef {
    fn drop(&mut self) {
        let mut roots = self.roots.borrow_mut();
        match roots.entry(self.handle) {
            Entry::Occupied(mut occ) => {
                let value = occ.get_mut();
                if *value >= 1 {
                    *value -= 1;
                } else {
                    eprintln!("ScriptHandleRef is 0!");
                }
                if *value == 0 {
                    occ.remove();
                }
            }
            Entry::Vacant(_vac) => {
                eprintln!("ScriptHandleRef root is vacant!");
            }
        }
    }
}

impl ScriptHeap {
    pub fn new_handle(
        &mut self,
        ty: ScriptHandleType,
        mut hgc: Box<dyn ScriptHandleGc>,
    ) -> ScriptHandle {
        if let Some(mut handle) = self.handles_free.pop() {
            // handle already has the correct generation from gc.rs sweep
            handle.ty = ty;
            hgc.set_handle(handle);
            self.handles[handle] = Some(ScriptHandleData {
                tag: Default::default(),
                handle: hgc,
            });
            handle
        } else {
            let index = self.handles.len();
            // New slot starts at generation 0
            let handle = ScriptHandle::new(ty, index as _, 0);
            hgc.set_handle(handle);
            self.handles.push(Some(ScriptHandleData {
                tag: Default::default(),
                handle: hgc,
            }));
            handle
        }
    }

    pub fn handle_ref<T: ScriptHandleGc + 'static>(&self, handle: ScriptHandle) -> Option<&T> {
        self.handles[handle]
            .as_ref()
            .and_then(|h| h.handle.downcast_ref::<T>())
    }

    pub fn handle_mut<T: ScriptHandleGc + 'static>(
        &mut self,
        handle: ScriptHandle,
    ) -> Option<&mut T> {
        self.handles[handle]
            .as_mut()
            .and_then(|h| h.handle.downcast_mut::<T>())
    }
}

#[derive(Default)]
pub struct HandleTag(u64);

impl HandleTag {
    const MARK: u64 = 0x1;
    const STATIC: u64 = 0x2;

    pub fn is_marked(&self) -> bool {
        self.0 & Self::MARK != 0
    }

    pub fn set_mark(&mut self) {
        self.0 |= Self::MARK
    }

    pub fn clear_mark(&mut self) {
        self.0 &= !Self::MARK
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }
}

pub struct ScriptHandleData {
    pub tag: HandleTag,
    pub handle: Box<dyn ScriptHandleGc>,
}

impl ScriptHandleData {
    pub fn gc(mut self) {
        self.handle.gc()
    }
}

pub trait ScriptHandleGc {
    fn gc(&mut self);
    fn set_handle(&mut self, _handle: ScriptHandle) {}
    fn ref_cast_type_id(&self) -> TypeId
    where
        Self: 'static,
    {
        TypeId::of::<Self>()
    }
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScriptHandleGc: No debug format")
    }
}

impl Debug for dyn ScriptHandleGc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.debug_fmt(f)
    }
}

impl dyn ScriptHandleGc {
    pub fn is<T: ScriptHandleGc + 'static>(&self) -> bool {
        let t = TypeId::of::<T>();
        let concrete = self.ref_cast_type_id();
        t == concrete
    }
    pub fn downcast_ref<T: ScriptHandleGc + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn ScriptHandleGc as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: ScriptHandleGc + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *const dyn ScriptHandleGc as *mut T) })
        } else {
            None
        }
    }
}
