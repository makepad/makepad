// `impl Trait` nested inside a parameter type is an anonymous generic parameter too (rustc:
// synthetic params, in order of appearance): libs/sdfer esdt::glyph_to_sdf takes
// `&mut Image2d<Unorm8, impl AsMut<[Unorm8]> + AsRef<[Unorm8]>>`. Before: "unsupported type
// (dyn/impl/macro)" in the signature -> makepad_draw text SDF on Linux (node 165) stopped.
pub struct Image2d<T, S = Vec<T>> { w: usize, s: S, _p: std::marker::PhantomData<T> }
fn total(img: &mut Image2d<u8, impl AsMut<[u8]> + AsRef<[u8]>>) -> usize { img.w + img.s.as_ref().len() }
fn first_some(v: &[Option<impl Fn(u32) -> u32>], x: u32) -> u32 { v.iter().flatten().next().map_or(0, |f| f(x)) }
fn pair(p: (impl Into<u64>, impl Into<u64>)) -> u64 { p.0.into() + p.1.into() }
#[test] fn nested_in_generic_args() { let mut i = Image2d::<u8, Vec<u8>> { w: 1, s: vec![1, 2], _p: std::marker::PhantomData }; assert_eq!(total(&mut i), 3); }
#[test] fn nested_in_slice_option() { let fs = [None, Some(|x: u32| x * 3)]; assert_eq!(first_some(&fs, 5), 15); }
#[test] fn nested_in_tuple() { assert_eq!(pair((3u8, 4u32)), 7); }

// A closure type met once with its signature still open and once resolved is the same type
// (unify compares the closure's signature and captures). Before: "expected `Option<closure#N>`,
// found `Option<closure#N>`" for `[None, Some(|x| ..)]` passed as `&[Option<F>]`.
fn first_some_g<F: Fn(u32) -> u32>(v: &[Option<F>], x: u32) -> u32 { v.iter().flatten().next().map_or(0, |f| f(x)) }
#[test] fn same_closure_open_and_resolved() { let fs = vec![None, Some(|x: u32| x * 3)]; assert_eq!(first_some_g(&fs, 5), 15); }

// An impl's `F: Fn(P2) -> P` bound fixes P2 from the closure's signature when the impl is
// selected (sdfer NDCursorMapPos; before, P2 stayed open and `reset(0)` made it i32).
pub trait Cursor<'a, P> { fn reset(&'a mut self, p: P); fn get(&'a mut self) -> usize; }
pub trait CursorExt<P>: for<'a> Cursor<'a, P> {
    fn map_abs<P2, FA: Fn(P2) -> P>(self, fa: FA) -> MapPos<Self, FA> where Self: Sized { MapPos(self, fa) }
}
impl<P, C: for<'a> Cursor<'a, P>> CursorExt<P> for C {}
pub struct MapPos<C, FA>(C, FA);
impl<'a, C: Cursor<'a, P>, P, P2, FA: Fn(P2) -> P> Cursor<'a, P2> for MapPos<C, FA> {
    fn reset(&'a mut self, p: P2) { self.0.reset((self.1)(p)); }
    fn get(&'a mut self) -> usize { self.0.get() }
}
pub struct Cur { pos: (usize, usize) }
impl<'a> Cursor<'a, (usize, usize)> for Cur {
    fn reset(&'a mut self, p: (usize, usize)) { self.pos = p; }
    fn get(&'a mut self) -> usize { self.pos.0 * 100 + self.pos.1 }
}
#[test] fn impl_fn_bound_fixes_param() { let x = 7usize; let mut c = Cur { pos: (0, 0) }.map_abs(move |y| (x, y)); c.reset(3); assert_eq!(c.get(), 703); }
