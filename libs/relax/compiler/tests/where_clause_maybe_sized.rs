use std::{mem::ManuallyDrop, ops::{Deref, DerefMut}, ptr::NonNull};
struct AliasableBox<T>
where
    T: ?Sized,
{
    ptr: NonNull<T>,
}
impl<T> AliasableBox<T>
where
    T: ?Sized,
{
    fn from_box(b: Box<T>) -> Self { Self { ptr: unsafe { NonNull::new_unchecked(Box::into_raw(b)) } } }
}
impl<T> Deref for AliasableBox<T>
where
    T: ?Sized,
{ type Target = T; fn deref(&self) -> &T { unsafe { self.ptr.as_ref() } } }
impl<T> DerefMut for AliasableBox<T>
where
    T: ?Sized,
{ fn deref_mut(&mut self) -> &mut T { unsafe { self.ptr.as_mut() } } }
impl<T> Drop for AliasableBox<T>
where
    T: ?Sized,
{
    fn drop(&mut self) { let this = ManuallyDrop::new(self); drop(unsafe { Box::from_raw(this.ptr.as_ptr()) }); }
}
struct Stack { slots: AliasableBox<[usize]> }
#[test]
fn unsized_box_like() {
    let mut s = Stack { slots: AliasableBox::from_box(vec![1usize, 2, 3].into_boxed_slice()) };
    s.slots[1] = 7;
    let p = s.slots.as_mut_ptr();
    unsafe { *p.add(2) = 9 };
    assert_eq!(&*s.slots, &[1, 7, 9]);
    assert_eq!(s.slots.len(), 3);
    let st: AliasableBox<str> = AliasableBox::from_box(String::from("hi").into_boxed_str());
    assert_eq!(&*st, "hi");
}

fn f<T>(x: &T) -> usize
where
    T: ?Sized,
{
    std::mem::size_of_val(x)
}

#[test]
fn unsized_fn_parameter() {
    assert_eq!(f::<str>("hello"), 5);
}

trait Measure<T>
where
    T: ?Sized,
{
    fn measure(&self, x: &T) -> usize;
}

impl<T> Measure<T> for ()
where
    T: ?Sized,
{
    fn measure(&self, x: &T) -> usize { f(x) }
}

enum Borrowed<'a, T>
where
    T: ?Sized,
{
    Value(&'a T),
}

#[test]
fn unsized_trait_impl_and_enum() {
    let value: Borrowed<'_, str> = Borrowed::Value("hello");
    match value {
        Borrowed::Value(x) => assert_eq!(<() as Measure<str>>::measure(&(), x), 5),
    }
}
