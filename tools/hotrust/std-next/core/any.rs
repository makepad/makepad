//! Any and TypeId.

use crate::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeId {
    t: u64,
}

impl TypeId {
    pub fn of<T: ?Sized + 'static>() -> TypeId {
        TypeId { t: unsafe { crate::intrinsics_mem::type_id::<T>() } }
    }
}

impl fmt::Debug for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TypeId({:#034x})", self.t as u128)
    }
}

pub fn type_name<T: ?Sized>() -> &'static str {
    unsafe { crate::intrinsics_mem::type_name::<T>() }
}

pub fn type_name_of_val<T: ?Sized>(_val: &T) -> &'static str {
    type_name::<T>()
}

/// Dynamic typing. HotRust implements Any for every 'static type itself (the one compiler-
/// provided blanket impl), so `&dyn Any` gets a vtable with `type_id`.
pub trait Any: 'static {
    fn type_id(&self) -> TypeId;
}

impl<T: 'static + ?Sized> Any for T {
    fn type_id(&self) -> TypeId {
        TypeId::of::<T>()
    }
}

impl dyn Any {
    pub fn is<T: Any>(&self) -> bool {
        self.type_id() == TypeId::of::<T>()
    }
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn Any as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *mut dyn Any as *mut T) })
        } else {
            None
        }
    }
}

impl dyn Any + Send {
    pub fn is<T: Any>(&self) -> bool {
        self.type_id() == TypeId::of::<T>()
    }
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const (dyn Any + Send) as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *mut (dyn Any + Send) as *mut T) })
        } else {
            None
        }
    }
}

impl dyn Any + Send + Sync {
    pub fn is<T: Any>(&self) -> bool {
        self.type_id() == TypeId::of::<T>()
    }
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const (dyn Any + Send + Sync) as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: Any>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *mut (dyn Any + Send + Sync) as *mut T) })
        } else {
            None
        }
    }
}

impl fmt::Debug for dyn Any {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Any { .. }")
    }
}
impl fmt::Debug for dyn Any + Send {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Any { .. }")
    }
}
impl fmt::Debug for dyn Any + Send + Sync {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Any { .. }")
    }
}
