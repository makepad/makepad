//! The Error trait.

use crate::fmt::{Debug, Display};

pub trait Error: Debug + Display {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
    fn description(&self) -> &str {
        "description() is deprecated; use Display"
    }
    fn cause(&self) -> Option<&dyn Error> {
        self.source()
    }
    #[doc(hidden)]
    fn type_id_internal(&self) -> crate::any::TypeId
    where
        Self: 'static,
    {
        crate::any::TypeId::of::<Self>()
    }
}

impl dyn Error {
    pub fn is<T: Error + 'static>(&self) -> bool {
        self.type_id_internal() == crate::any::TypeId::of::<T>()
    }
    pub fn downcast_ref<T: Error + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const dyn Error as *const T) })
        } else {
            None
        }
    }
    pub fn downcast_mut<T: Error + 'static>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            Some(unsafe { &mut *(self as *mut dyn Error as *mut T) })
        } else {
            None
        }
    }
}

impl dyn Error + Send + Sync {
    pub fn is<T: Error + 'static>(&self) -> bool {
        self.type_id_internal() == crate::any::TypeId::of::<T>()
    }
    pub fn downcast_ref<T: Error + 'static>(&self) -> Option<&T> {
        if self.is::<T>() {
            Some(unsafe { &*(self as *const (dyn Error + Send + Sync) as *const T) })
        } else {
            None
        }
    }
}

impl<'a, E: Error + ?Sized> Error for &'a E {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Error::source(&**self)
    }
}
