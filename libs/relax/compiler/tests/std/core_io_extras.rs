// io::repeat / Repeat, Empty as a writer and seeker, &Sink, IoSlice / IoSliceMut (new, deref,
// advance, advance_slices) and the default read_vectored / write_vectored (real std).
use std::io::{self, IoSlice, IoSliceMut, Read, Seek, SeekFrom, Write};

#[test]
fn a01_repeat_empty_sink() {
    let mut buf = [0u8; 5];
    let n = io::repeat(7).read(&mut buf).unwrap();
    println!("| repeat {} {:?}", n, buf);
    let mut taken = Vec::new();
    io::repeat(b'z').take(4).read_to_end(&mut taken).unwrap();
    println!("| take {:?}", String::from_utf8(taken).unwrap());
    let mut v = Vec::new();
    println!("| repeat read_to_end {:?}", io::repeat(1).read_to_end(&mut v).map_err(|e| e.kind()));
    let mut e = io::empty();
    println!("| empty write {:?} seek {:?} flush {:?}", e.write(b"abc").map_err(|e| e.kind()), e.seek(SeekFrom::End(5)).map_err(|e| e.kind()), e.flush().is_ok());
    let s = io::sink();
    let mut r = &s;
    println!("| &sink {:?} {:?}", r.write(b"xyz").map_err(|e| e.kind()), io::copy(&mut io::repeat(0).take(1000), &mut io::sink()).map_err(|e| e.kind()));
    println!("| debug {:?} {:?}", io::empty(), io::sink());
}

#[test]
fn a02_io_slices() {
    let a = [1u8, 2, 3];
    let b = [4u8, 5];
    let c = [6u8, 7, 8, 9];
    let mut slices = [IoSlice::new(&a), IoSlice::new(&b), IoSlice::new(&c)];
    println!("| deref {} {:?}", slices[0].len(), &*slices[1]);
    let mut bufs: &mut [IoSlice] = &mut slices;
    IoSlice::advance_slices(&mut bufs, 4);
    println!("| advanced {} first {:?}", bufs.len(), &*bufs[0]);
    let mut one = IoSlice::new(&c);
    one.advance(2);
    println!("| one {:?} dbg {:?}", &*one, one);
    let mut out = Vec::new();
    let n = out.write_vectored(&[IoSlice::new(&[]), IoSlice::new(&a), IoSlice::new(&b)]).unwrap();
    println!("| write_vectored {} {:?}", n, out);
    let mut x = [0u8; 2];
    let mut y = [0u8; 3];
    let mut src: &[u8] = &[9, 8, 7, 6, 5];
    let n = src.read_vectored(&mut [IoSliceMut::new(&mut x), IoSliceMut::new(&mut y)]).unwrap();
    println!("| read_vectored {} {:?} {:?}", n, x, y);
    let mut m1 = [1u8; 3];
    let mut m2 = [2u8; 3];
    let mut ms = [IoSliceMut::new(&mut m1), IoSliceMut::new(&mut m2)];
    let mut mb: &mut [IoSliceMut] = &mut ms;
    IoSliceMut::advance_slices(&mut mb, 5);
    mb[0][0] = 99;
    println!("| mut advanced {} {:?}", mb.len(), &*mb[0]);
}
