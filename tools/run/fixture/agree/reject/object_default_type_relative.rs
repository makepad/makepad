// rejects: E0228
pub trait G: Sized { type Shim<T: ?Sized>; }
pub trait Tr {}
pub struct S<T: G + 'static>(pub &'static T::Shim<dyn Tr>);
