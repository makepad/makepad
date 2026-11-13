// io::Write for Cursor<Box<[u8]>> (stable since 1.5) and Cursor<[u8; N]> (1.61): a cursor over a
// buffer it owns and cannot grow, as tokio's `impl AsyncWrite for io::Cursor<Box<[u8]>>`
// (src/io/async_write.rs) and futures-util's (src/io/cursor.rs) use it. Both crates also forward
// `write_vectored`, which std's cursors answer with every buffer that fits, not the first one.
use std::io::{self, Cursor, IoSlice, Read, Seek, SeekFrom, Write};

// tokio's shape: the std impl called by path through a reborrow
fn poll_write<W>(w: &mut W, buf: &[u8]) -> io::Result<usize>
where
    W: Write,
{
    io::Write::write(&mut *w, buf)
}
fn poll_write_vectored<W: Write>(w: &mut W, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
    io::Write::write_vectored(&mut *w, bufs)
}

#[test]
fn boxed_slice_cursor_writes_until_full() {
    let mut c: Cursor<Box<[u8]>> = Cursor::new(vec![0u8; 8].into_boxed_slice());
    assert_eq!(poll_write(&mut c, b"abc").unwrap(), 3);
    assert_eq!(c.position(), 3);
    assert_eq!(c.write(b"defgh-overflow").unwrap(), 5);
    assert_eq!(c.position(), 8);
    // full: a write stores nothing, write_all reports WriteZero
    assert_eq!(c.write(b"x").unwrap(), 0);
    let e = c.write_all(b"x").unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::WriteZero);
    assert_eq!(e.to_string(), "failed to write whole buffer");
    c.flush().unwrap();
    assert_eq!(&**c.get_ref(), b"abcdefgh");

    // overwrite in the middle, then read the bytes back through the same cursor
    c.seek(SeekFrom::Start(2)).unwrap();
    write!(c, "{}{}", 4, "Z").unwrap();
    assert_eq!(c.position(), 4);
    c.set_position(0);
    let mut s = String::new();
    c.read_to_string(&mut s).unwrap();
    assert_eq!(s, "ab4Zefgh");

    // a position past the end writes nothing
    c.set_position(100);
    assert_eq!(c.write(b"q").unwrap(), 0);
    assert_eq!(c.position(), 100);
    let b: Box<[u8]> = c.into_inner();
    assert_eq!(&*b, b"ab4Zefgh");

    let mut empty: Cursor<Box<[u8]>> = Cursor::new(Box::new([]));
    assert_eq!(empty.write(b"abc").unwrap(), 0);
    assert_eq!(empty.write(b"").unwrap(), 0);
    assert!(empty.write_all(b"").is_ok());
}

#[test]
fn array_cursor_writes_until_full() {
    let mut c = Cursor::new([0u8; 5]);
    assert_eq!(poll_write(&mut c, b"12").unwrap(), 2);
    c.write_all(b"34").unwrap();
    assert_eq!(c.write(b"567").unwrap(), 1);
    assert_eq!(c.write(b"8").unwrap(), 0);
    assert_eq!(c.write_all(b"8").unwrap_err().kind(), io::ErrorKind::WriteZero);
    assert_eq!(c.position(), 5);
    assert_eq!(c.into_inner(), *b"12345");

    let mut z = Cursor::new([0u8; 0]);
    assert_eq!(z.write(b"a").unwrap(), 0);
    z.flush().unwrap();
}

#[test]
fn vectored_writes_take_every_buffer_that_fits() {
    let bufs = [IoSlice::new(b"ab"), IoSlice::new(b""), IoSlice::new(b"cde"), IoSlice::new(b"fgh")];

    let mut boxed: Cursor<Box<[u8]>> = Cursor::new(vec![b'.'; 6].into_boxed_slice());
    assert_eq!(poll_write_vectored(&mut boxed, &bufs).unwrap(), 6);
    assert_eq!(&**boxed.get_ref(), b"abcdef");
    assert_eq!(boxed.write_vectored(&bufs).unwrap(), 0);

    let mut array = Cursor::new([b'.'; 4]);
    assert_eq!(array.write_vectored(&bufs).unwrap(), 4);
    assert_eq!(array.get_ref(), b"abcd");

    let mut store = [b'.'; 7];
    let mut slice = Cursor::new(&mut store[..]);
    assert_eq!(slice.write_vectored(&bufs).unwrap(), 7);
    assert_eq!(slice.position(), 7);
    assert_eq!(&store, b"abcdefg");

    // the growing cursors take them all
    let mut vec = Cursor::new(vec![b'.'; 3]);
    vec.set_position(1);
    assert_eq!(vec.write_vectored(&bufs).unwrap(), 8);
    assert_eq!(vec.get_ref(), b".abcdefgh");
    let mut out = Vec::new();
    let mut by_ref = Cursor::new(&mut out);
    assert_eq!(by_ref.write_vectored(&bufs).unwrap(), 8);
    assert_eq!(by_ref.write_vectored(&[]).unwrap(), 0);
    assert_eq!(out, b"abcdefgh");
}
