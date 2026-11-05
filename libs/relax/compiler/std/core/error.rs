//! The Error trait.

use crate::fmt::{self, Debug, Display};
use crate::any::TypeId;
use crate::marker::PhantomData;

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
    fn provide<'a>(&'a self, _request: &mut Request<'a>) {}
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
    fn provide<'b>(&'b self, request: &mut Request<'b>) {
        Error::provide(&**self, request);
    }
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Error::source(&**self)
    }
}

/// Requests an owned value supplied by an error's metadata provider.
pub fn request_value<T: 'static>(error: &(impl Error + ?Sized)) -> Option<T> {
    request::<ValueTag<T>>(error)
}

/// Requests a borrowed value, including an unsized value such as `str`.
pub fn request_ref<'a, T: ?Sized + 'static>(error: &'a (impl Error + ?Sized)) -> Option<&'a T> {
    request::<RefTag<T>>(error)
}

fn request<'a, I: TypeTag<'a>>(error: &'a (impl Error + ?Sized)) -> Option<I::Reified> {
    let mut slot = Slot::<'a, I>(None);
    let erased: &mut dyn RequestSlot<'a> = &mut slot;
    // SAFETY: Request is transparent over the same trait object. The stack slot
    // remains alive for the entire provider call and cannot escape that borrow.
    error.provide(unsafe { &mut *(erased as *mut dyn RequestSlot<'a> as *mut Request<'a>) });
    slot.0
}

/// A typed metadata request. Only the first matching provider satisfies it.
#[repr(transparent)]
pub struct Request<'a>(dyn RequestSlot<'a> + 'a);

impl<'a> Request<'a> {
    pub fn provide_value<T: 'static>(&mut self, value: T) -> &mut Self {
        self.provide_value_with(|| value)
    }
    pub fn provide_value_with<T: 'static>(&mut self, fulfil: impl FnOnce() -> T) -> &mut Self {
        self.provide_with::<ValueTag<T>>(fulfil)
    }
    pub fn provide_ref<T: ?Sized + 'static>(&mut self, value: &'a T) -> &mut Self {
        self.provide_ref_with(|| value)
    }
    pub fn provide_ref_with<T: ?Sized + 'static>(&mut self, fulfil: impl FnOnce() -> &'a T) -> &mut Self {
        self.provide_with::<RefTag<T>>(fulfil)
    }
    pub fn would_be_satisfied_by_value_of<T: 'static>(&self) -> bool {
        self.would_accept::<ValueTag<T>>()
    }
    pub fn would_be_satisfied_by_ref_of<T: ?Sized + 'static>(&self) -> bool {
        self.would_accept::<RefTag<T>>()
    }
    fn would_accept<I: TypeTag<'a>>(&self) -> bool {
        self.0.tag_id() == TypeId::of::<I>() && !self.0.fulfilled()
    }
    fn provide_with<I: TypeTag<'a>>(&mut self, fulfil: impl FnOnce() -> I::Reified) -> &mut Self {
        if self.would_accept::<I>() {
            // SAFETY: RequestSlot is private and implemented only for Slot<I>.
            // Matching tag IDs identify the exact Option representation, including
            // the reference lifetime and metadata of unsized pointees.
            let slot = unsafe { &mut *(self.0.value_ptr() as *mut Option<I::Reified>) };
            *slot = Some(fulfil());
        }
        self
    }
}

impl Debug for Request<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request").finish_non_exhaustive()
    }
}

// Tags distinguish requesting T by value from requesting &'a T. They are
// static identities; the Reified value separately carries the provider lifetime.
trait TypeTag<'a>: 'static {
    type Reified: 'a;
}
struct ValueTag<T: 'static>(PhantomData<T>);
impl<'a, T: 'static> TypeTag<'a> for ValueTag<T> {
    type Reified = T;
}
struct RefTag<T: ?Sized + 'static>(PhantomData<T>);
impl<'a, T: ?Sized + 'static> TypeTag<'a> for RefTag<T> {
    type Reified = &'a T;
}
struct Slot<'a, I: TypeTag<'a>>(Option<I::Reified>);
trait RequestSlot<'a>: 'a {
    fn tag_id(&self) -> TypeId;
    fn fulfilled(&self) -> bool;
    fn value_ptr(&mut self) -> *mut ();
}
impl<'a, I: TypeTag<'a>> RequestSlot<'a> for Slot<'a, I> {
    fn tag_id(&self) -> TypeId { TypeId::of::<I>() }
    fn fulfilled(&self) -> bool { self.0.is_some() }
    fn value_ptr(&mut self) -> *mut () { &mut self.0 as *mut Option<I::Reified> as *mut () }
}
