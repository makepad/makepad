// Nested cursor projections must preserve the lifetime bound by the caller's
// for<'a> equality through impl selection and associated-type normalization.
trait Cursor<'a, P> {
    type RefMut;
    fn get_mut(&'a mut self) -> Self::RefMut;
}
trait CursorExt<P>: for<'a> Cursor<'a, P> {
    fn zip<C: CursorExt<P>>(self, other: C) -> Zip<Self, C> where Self: Sized {
        Zip(self, other)
    }
    fn map<Q, F: Fn(Q) -> P>(self, map: F) -> Map<Self, F> where Self: Sized {
        Map(self, map)
    }
}
impl<P, C: for<'a> Cursor<'a, P>> CursorExt<P> for C {}
struct Zip<A, B>(A, B);
impl<'a, P, A: Cursor<'a, P>, B: Cursor<'a, P>> Cursor<'a, P> for Zip<A, B> {
    type RefMut = (A::RefMut, B::RefMut);
    fn get_mut(&'a mut self) -> Self::RefMut { (self.0.get_mut(), self.1.get_mut()) }
}
struct Map<C, F>(C, F);
impl<'a, P, Q, C: Cursor<'a, P>, F: Fn(Q) -> P> Cursor<'a, Q> for Map<C, F> {
    type RefMut = C::RefMut;
    fn get_mut(&'a mut self) -> Self::RefMut { self.0.get_mut() }
}
struct Cell<'b, T>(&'b mut T);
impl<'a, T: 'a> Cursor<'a, (usize, usize)> for Cell<'_, T> {
    type RefMut = &'a mut T;
    fn get_mut(&'a mut self) -> Self::RefMut { self.0 }
}
fn update(mut cursor: impl for<'a> Cursor<'a, usize, RefMut = (&'a mut bool, (&'a mut f32, &'a mut f32))>) {
    let (flag, (x, y)) = cursor.get_mut();
    *flag = true;
    *x += 2.0;
    *y += 3.0;
}
#[test]
fn nested_projection_equality_runs() {
    let mut flag = false;
    let mut x = 4.0f32;
    let mut y = 5.0f32;
    let offset = 7;
    update(Cell(&mut flag).zip(Cell(&mut x).zip(Cell(&mut y))).map(move |n| (offset, n)));
    assert!(flag);
    assert_eq!(x, 6.0);
    assert_eq!(y, 8.0);
}

fn update_pair(mut cursor: impl for<'a> Cursor<'a, (usize, usize), RefMut = (&'a mut f32, &'a mut f32)>) {
    let (x, y) = cursor.get_mut();
    *x = 9.0;
    *y = 10.0;
}
#[test]
fn direct_projection_equality_runs() {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    update_pair(Zip(Cell(&mut x), Cell(&mut y)));
    assert_eq!(x, 9.0);
    assert_eq!(y, 10.0);
}

trait Array { type Item; }
impl<T, const N: usize> Array for [T; N] { type Item = T; }
struct Buffer<A>(A);
trait Append<T> { fn append(&mut self, value: T); }
impl<A: Array> Append<A::Item> for Buffer<A> {
    fn append(&mut self, _: A::Item) {}
}
#[test]
fn projection_in_impl_header() {
    let mut buffer = Buffer([0u32; 2]);
    buffer.append(7);
}

trait At<'a> { type Value; fn value(&'a self) -> Self::Value; }
impl<'a> At<'a> for u32 {
    type Value = u32;
    fn value(&'a self) -> u32 { *self }
}
fn body_projection<T: for<'a> At<'a>>(value: T) {
    let _: T::Value = value.value();
}
#[test]
fn body_projection_infers_bound_lifetime() { body_projection(7u32); }
