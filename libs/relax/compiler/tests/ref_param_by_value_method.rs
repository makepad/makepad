// A by-value `self` method called on `&mut R` / `&R` where R is a generic param: rustc's probe
// picks the trait's method on the reference itself at the first step (`impl Read for &mut R`),
// never the param's where-clause method on `*reader` (a bitwise move out of the reference: the
// caller's reader did not advance, and a BufReader's buffer was freed twice -- libs/cef tar.rs).
use std::cell::Cell;
use std::io::{self, Read};

fn take_generic(reader: &mut impl Read) -> u8 {
    let mut b = [0u8; 2];
    reader.take(2).read_exact(&mut b).unwrap();
    b[1]
}
fn take_bound<R: Read>(reader: &mut R) -> u8 {
    let mut b = [0u8; 2];
    reader.take(2).read_exact(&mut b).unwrap();
    b[1]
}

#[test]
fn take_through_mut_ref_advances_the_reader() {
    let data: &[u8] = &[1, 2, 3, 4, 5];
    let mut c = io::Cursor::new(data);
    assert_eq!(take_generic(&mut c), 2);
    assert_eq!(c.position(), 2);
    assert_eq!(take_bound(&mut c), 4);
    assert_eq!(c.position(), 4);
}

#[test]
fn take_inside_closure_advances_the_reader() {
    fn run(mut reader: impl Read) -> u8 {
        (|| take_generic(&mut reader))();
        let mut b = [0u8; 1];
        reader.read_exact(&mut b).unwrap();
        b[0]
    }
    assert_eq!(run(io::Cursor::new(&[1u8, 2, 3][..])), 3);
}

// a reader that owns a heap buffer and counts its drops: the by-value pick on `*reader` dropped
// a bitwise copy of it (double free)
thread_local! { static DROPS: Cell<u32> = Cell::new(0); }
struct Owned { buf: Vec<u8>, pos: usize }
impl Read for Owned {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
impl Drop for Owned { fn drop(&mut self) { DROPS.with(|d| d.set(d.get() + 1)); } }

#[test]
fn take_through_mut_ref_drops_once() {
    DROPS.with(|d| d.set(0));
    {
        let mut r = Owned { buf: vec![7, 8, 9], pos: 0 };
        assert_eq!(take_bound(&mut r), 8);
        assert_eq!(r.pos, 2);
        assert_eq!(DROPS.with(|d| d.get()), 0);
    }
    assert_eq!(DROPS.with(|d| d.get()), 1);
}

// the same through a projection: `r: &mut S::Reader` with `type Reader: Read`
trait Source { type Reader: Read; }
struct CursorSource;
impl Source for CursorSource { type Reader = io::Cursor<&'static [u8]>; }
fn take_projection<S: Source>(r: &mut S::Reader) -> u8 {
    let mut b = [0u8; 2];
    r.take(2).read_exact(&mut b).unwrap();
    b[1]
}

#[test]
fn take_through_projection_ref() {
    let mut c = io::Cursor::new(&[1u8, 2, 3, 4][..]);
    assert_eq!(take_projection::<CursorSource>(&mut c), 2);
    assert_eq!(c.position(), 2);
}
