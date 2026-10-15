use makepad_relax_api::relax_api;
use crate::heap::*;
use crate::value::*;
use std::any::{Any, TypeId};
use std::fmt;
use std::fmt::Debug;

/// A Rust-held reference to a script handle: a GC root while it lives (roots.rs).
#[relax_api(plain)]
#[derive(Debug)]
#[repr(C)]
pub struct ScriptHandleRef {
    pub(crate) roots: *const crate::roots::ScriptRoots,
    pub(crate) handle: ScriptHandle,
}

impl From<ScriptHandleRef> for ScriptValue {
    fn from(v: ScriptHandleRef) -> Self {
        ScriptValue::from_handle(v.as_handle())
    }
}

impl Clone for ScriptHandleRef {
    fn clone(&self) -> Self {
        crate::roots::script_root_retain(self.roots, crate::roots::ROOT_HANDLE, self.handle.index);
        Self {
            roots: self.roots,
            handle: self.handle,
        }
    }
}

impl ScriptHandleRef {
    pub fn as_handle(&self) -> ScriptHandle {
        self.handle
    }
    /// The owning heap's identity (`ScriptHeap::heap_key`), the pair a resource lookup needs.
    /// A handle VALUE means nothing outside its heap: two isolates hand out the same numbers
    /// for different things, so anything Cx-owned that a handle names (a resource's bytes) is
    /// looked up by heap and handle.
    pub fn heap_key(&self) -> usize {
        self.roots as usize
    }
}

impl Drop for ScriptHandleRef {
    fn drop(&mut self) {
        crate::roots::script_root_release(self.roots, crate::roots::ROOT_HANDLE, self.handle.index);
    }
}

/// handle_edit's mode.
pub const HANDLE_EDIT_NEW: u32 = 0;
pub const HANDLE_EDIT_MUT: u32 = 1;

/// What handle_edit answers: the handle, and its box (null when the handle is not live).
#[relax_api(plain)]
#[repr(C)]
pub struct HandleEdit {
    pub handle: ScriptHandle,
    pub boxed: *mut ScriptHandleBox,
}

#[relax_api(marked)]
impl ScriptHeap {
    #[relax_api]
    pub fn handle_box_ref(&self, handle: ScriptHandle) -> Option<&ScriptHandleBox> {
        self.handles[handle].as_ref().map(|h| &h.handle)
    }
    #[inline]
    pub fn handle_box_mut(&mut self, handle: ScriptHandle) -> Option<&mut ScriptHandleBox> {
        // SAFETY: the box is the handle's, borrowed from `self`
        unsafe { self.handle_edit(HANDLE_EDIT_MUT, handle, ScriptHandleType::default(), std::ptr::null()).boxed.as_mut() }
    }

    /// Registers a handle object (the closure-free part of a handle type's state).
    #[inline(always)]
    pub fn new_handle<T: ScriptHandleGc>(&mut self, ty: ScriptHandleType, hgc: Box<T>) -> ScriptHandle {
        self.new_handle_boxed(ty, ScriptHandleBox::new(hgc))
    }

    #[inline(never)]
    pub fn new_handle_boxed(&mut self, ty: ScriptHandleType, hgc: ScriptHandleBox) -> ScriptHandle {
        let hgc = std::mem::ManuallyDrop::new(hgc);
        // SAFETY: `hgc` is moved into the op (ManuallyDrop here)
        unsafe { self.handle_edit(HANDLE_EDIT_NEW, ScriptHandle::default(), ty, &*hgc as *const ScriptHandleBox).handle }
    }

    /// Changes handles by `mode`: HANDLE_EDIT_NEW registers the box at `hgc` (moved in) as a handle of type `ty`
    /// (`handle`); HANDLE_EDIT_MUT the box of `handle` (`boxed`, null when it is not live).
    ///
    /// # Safety
    /// For NEW, `hgc` is a readable box the caller relinquishes.
    #[relax_api]
    pub unsafe fn handle_edit(&mut self, mode: u32, handle: ScriptHandle, ty: ScriptHandleType, hgc: *const ScriptHandleBox) -> HandleEdit {
        if mode == HANDLE_EDIT_MUT {
            let boxed = self.handles[handle].as_mut().map_or(std::ptr::null_mut(), |h| &mut h.handle as *mut ScriptHandleBox);
            return HandleEdit { handle, boxed };
        }
        // SAFETY: NEW moves the caller's box in (the caller does not drop it)
        let mut hgc = unsafe { std::ptr::read(hgc) };
        let handle = if let Some(mut handle) = self.handles_free.pop() {
            // handle already has the correct generation from gc.rs sweep
            handle.ty = ty;
            hgc.set_handle(handle);
            self.handles[handle] = Some(ScriptHandleData {
                tag: Default::default(),
                handle: hgc,
            });
            handle
        } else {
            // a new slot starts at the heap's tag; the data is set once the handle is known
            let (index, generation) = self.handles.push(None);
            let handle = ScriptHandle::new(ty, index, generation);
            hgc.set_handle(handle);
            self.handles.set_at(index as usize, Some(ScriptHandleData {
                tag: Default::default(),
                handle: hgc,
            }));
            handle
        };
        HandleEdit { handle, boxed: std::ptr::null_mut() }
    }

    pub fn handle_ref<T: ScriptHandleGc + 'static>(&self, handle: ScriptHandle) -> Option<&T> {
        self.handle_box_ref(handle).and_then(|h| h.downcast_ref::<T>())
    }

    pub fn handle_mut<T: ScriptHandleGc + 'static>(
        &mut self,
        handle: ScriptHandle,
    ) -> Option<&mut T> {
        self.handle_box_mut(handle).and_then(|h| h.downcast_mut::<T>())
    }
}

#[relax_api(plain)]
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

#[relax_api(plain)]
pub struct ScriptHandleData {
    pub tag: HandleTag,
    pub handle: ScriptHandleBox,
}

impl ScriptHandleData {
    pub fn gc(mut self) {
        self.handle.gc()
    }
}

pub trait ScriptHandleGc: Any {
    fn gc(&mut self);
    fn set_handle(&mut self, _handle: ScriptHandle) {}
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ScriptHandleGc: No debug format")
    }
}

/// The C-ABI table of one handle type: the VM calls a handle object only through it (the
/// object may come from code another compiler built: local/plans/loader-core.md section 8).
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHandleVtable {
    gc: unsafe extern "C-unwind" fn(*mut u8),
    set_handle: unsafe extern "C-unwind" fn(*mut u8, ScriptHandle),
    /// formats the object's Debug text and passes it to `write(ctx, ptr, len)`
    debug: unsafe extern "C-unwind" fn(*const u8, unsafe extern "C-unwind" fn(*mut u8, *const u8, usize), *mut u8),
    drop: unsafe extern "C-unwind" fn(*mut u8),
}

/// The first 8 bytes of `TypeId::of::<T>()`: the downcast check. Compared only against keys
/// made by code of the same compiler (a handle object built by the other compiler simply never
/// downcasts), and only this u64 crosses the C boundary, never a TypeId.
#[inline(always)]
fn type_key<T: 'static>() -> u64 {
    const { assert!(std::mem::size_of::<TypeId>() >= 8) };
    let id = TypeId::of::<T>();
    unsafe { std::ptr::read_unaligned(&id as *const TypeId as *const u64) }
}

/// Opaque adapter whose formatting callback and formatter belong to the same compiler.
#[relax_api(plain)]
pub struct ScriptHandleDebug {
    data: *const u8,
    format: unsafe extern "C-unwind" fn(*const u8, *mut u8) -> bool,
}

impl ScriptHandleDebug {
    /// # Safety
    /// `data` must remain valid for `format` while this adapter is used.
    pub unsafe fn new(data: *const u8, format: unsafe extern "C-unwind" fn(*const u8, *mut u8) -> bool) -> Self {
        Self { data, format }
    }
    /// # Safety
    /// `formatter` must point to a formatter from the callback's compiler.
    pub unsafe fn format_into(&self, formatter: *mut u8) -> bool { (self.format)(self.data, formatter) }
}

impl Debug for ScriptHandleDebug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: this impl and the installed callback run on the same caller side.
        if unsafe { self.format_into(f as *mut _ as *mut u8) } { Ok(()) } else { Err(fmt::Error) }
    }
}

struct HandleVt<T>(std::marker::PhantomData<T>);

impl<T: ScriptHandleGc> HandleVt<T> {
    const VT: ScriptHandleVtable = ScriptHandleVtable {
        gc: Self::gc,
        set_handle: Self::set_handle,
        debug: Self::debug,
        drop: Self::drop,
    };

    unsafe extern "C-unwind" fn gc(data: *mut u8) {
        (*(data as *mut T)).gc()
    }
    unsafe extern "C-unwind" fn set_handle(data: *mut u8, handle: ScriptHandle) {
        (*(data as *mut T)).set_handle(handle)
    }
    unsafe extern "C-unwind" fn debug(data: *const u8, write: unsafe extern "C-unwind" fn(*mut u8, *const u8, usize), ctx: *mut u8) {
        unsafe extern "C-unwind" fn format<T: ScriptHandleGc>(data: *const u8, formatter: *mut u8) -> bool {
            (&*(data as *const T)).debug_fmt(&mut *(formatter as *mut fmt::Formatter<'_>)).is_ok()
        }
        let text = format!("{:?}", ScriptHandleDebug::new(data, format::<T>));
        write(ctx, text.as_ptr(), text.len());
    }
    unsafe extern "C-unwind" fn drop(data: *mut u8) {
        std::mem::drop(Box::from_raw(data as *mut T));
    }
}

/// A handle object owned by the heap: `{data, vtable}` with a C-ABI vtable.
#[relax_api(plain)]
#[repr(C)]
pub struct ScriptHandleBox {
    data: *mut u8,
    vt: &'static ScriptHandleVtable,
    /// the concrete type, for downcasts (`type_key::<T>()` of the code that built the object)
    type_key: u64,
}

impl ScriptHandleBox {
    pub fn type_key(&self) -> u64 { self.type_key }
    pub fn data_ptr(&self) -> *mut u8 { self.data }

    #[inline(always)]
    pub fn new<T: ScriptHandleGc>(b: Box<T>) -> Self {
        Self {
            data: Box::into_raw(b) as *mut u8,
            vt: &HandleVt::<T>::VT,
            type_key: type_key::<T>(),
        }
    }

    #[inline]
    pub fn gc(&mut self) {
        unsafe { ((self.vt).gc)(self.data) }
    }

    #[inline]
    pub fn set_handle(&mut self, handle: ScriptHandle) {
        unsafe { ((self.vt).set_handle)(self.data, handle) }
    }

    #[inline]
    pub fn is<T: ScriptHandleGc + 'static>(&self) -> bool {
        self.type_key() == type_key::<T>()
    }

    pub fn downcast_ref<T: ScriptHandleGc + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self.data_ptr() as *const T) })
        } else {
            None
        }
    }

    pub fn downcast_mut<T: ScriptHandleGc + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self.data_ptr() as *mut T) })
        } else {
            None
        }
    }
}

impl Drop for ScriptHandleBox {
    fn drop(&mut self) {
        unsafe { ((self.vt).drop)(self.data) }
    }
}

impl Debug for ScriptHandleBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe extern "C-unwind" fn write(ctx: *mut u8, ptr: *const u8, len: usize) {
            let out = &mut *(ctx as *mut String);
            out.push_str(std::str::from_utf8_unchecked(std::slice::from_raw_parts(ptr, len)));
        }
        let mut text = String::new();
        unsafe { ((self.vt).debug)(self.data, write, &mut text as *mut String as *mut u8) };
        f.write_str(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FirstHandle;
    struct SecondHandle;

    impl ScriptHandleGc for FirstHandle {
        fn gc(&mut self) {}
    }

    impl ScriptHandleGc for SecondHandle {
        fn gc(&mut self) {}
    }

    #[test]
    fn handle_downcasts_use_the_concrete_any_type() {
        let mut handle = ScriptHandleBox::new(Box::new(FirstHandle));

        assert!(handle.is::<FirstHandle>());
        assert!(!handle.is::<SecondHandle>());
        assert!(handle.downcast_ref::<FirstHandle>().is_some());
        assert!(handle.downcast_ref::<SecondHandle>().is_none());
        assert!(handle.downcast_mut::<FirstHandle>().is_some());
        assert!(handle.downcast_mut::<SecondHandle>().is_none());
    }
}
