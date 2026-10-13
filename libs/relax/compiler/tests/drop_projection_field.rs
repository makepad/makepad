// A field declared through an associated type (`<StdAlloc as Alloc>::Mem`) has the normalized
// type for drop analysis (rustc FieldDef::ty + normalize_erasing_regions): the nested user Drop
// impl runs (brotli 8 CompressorWriter: its field is CompressorWriterCustomAlloc<W,
// <StandardAlloc as Allocator<u8>>::AllocatedMemory, ..>, whose inner Drop flushes the stream;
// Relax skipped it and the output stayed empty).
use std::cell::Cell;

thread_local! { static FLUSHED: Cell<u32> = Cell::new(0); }

pub trait Wrap { fn n(&self) -> usize; }
impl Wrap for Vec<u8> { fn n(&self) -> usize { self.len() } }
pub trait Alloc { type Mem: Wrap; fn mem(&self, n: usize) -> Self::Mem; }
pub struct StdAlloc;
impl Alloc for StdAlloc { type Mem = Vec<u8>; fn mem(&self, n: usize) -> Vec<u8> { vec![0; n] } }

pub struct Inner<B: Wrap> { buf: B, out: Option<u32> }
impl<B: Wrap> Drop for Inner<B> {
    fn drop(&mut self) {
        if let Some(k) = self.out {
            FLUSHED.with(|f| f.set(f.get() + k + self.buf.n() as u32));
        }
    }
}
pub struct Mid<B: Wrap>(Inner<B>);
pub struct Outer(Mid<<StdAlloc as Alloc>::Mem>);
pub struct Generic<A: Alloc>(Mid<A::Mem>);

fn flushed() -> u32 { FLUSHED.with(|f| f.replace(0)) }

#[test]
fn projection_field_drops_at_scope_end() {
    flushed();
    { let _o = Outer(Mid(Inner { buf: StdAlloc.mem(3), out: Some(10) })); }
    assert_eq!(flushed(), 13);
}

#[test]
fn projection_field_drops_by_drop_fn() {
    flushed();
    let o = Outer(Mid(Inner { buf: StdAlloc.mem(2), out: Some(1) }));
    drop(o);
    assert_eq!(flushed(), 3);
}

#[test]
fn generic_projection_field_drops() {
    flushed();
    { let _g: Generic<StdAlloc> = Generic(Mid(Inner { buf: StdAlloc.mem(4), out: Some(100) })); }
    assert_eq!(flushed(), 104);
}

#[test]
fn projection_field_in_vec_and_option() {
    flushed();
    {
        let _v = vec![Outer(Mid(Inner { buf: StdAlloc.mem(1), out: Some(1) })), Outer(Mid(Inner { buf: StdAlloc.mem(1), out: Some(1) }))];
        let _o = Some(Outer(Mid(Inner { buf: StdAlloc.mem(0), out: Some(5) })));
    }
    assert_eq!(flushed(), 9);
}
