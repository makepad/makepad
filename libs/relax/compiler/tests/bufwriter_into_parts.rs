// BufWriter::into_parts (relax PR #93) takes the inner writer and the buffer without flushing. If
// the inner writer panicked, the buffer comes back as WriterPanicked (the bytes that were still
// buffered; how much of them the panicked write took is not known), as in rustc's std. Found
// running an editor under Relax. `rustc --test` and `makepad test` both pass it.
use std::io::{self, BufWriter, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};

struct Mem(Vec<u8>);

impl Write for Mem {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct PanickingWriter;

impl Write for PanickingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        panic!("inner write");
    }
    fn flush(&mut self) -> io::Result<()> {
        panic!("inner flush");
    }
}

#[test]
fn into_parts_returns_the_buffer_without_flushing() {
    let mut w = BufWriter::with_capacity(64, Mem(Vec::new()));
    w.write_all(b"hello").unwrap();
    let (inner, buf) = w.into_parts();
    assert!(inner.0.is_empty(), "into_parts must not flush");
    assert_eq!(buf.unwrap(), b"hello");
}

#[test]
fn into_parts_after_inner_panic_keeps_the_buffer() {
    let mut stream = BufWriter::new(PanickingWriter);
    write!(stream, "some data").unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| stream.flush().unwrap()));
    assert!(result.is_err());
    let (recovered_writer, buffered_data) = stream.into_parts();
    assert!(matches!(recovered_writer, PanickingWriter));
    let err = buffered_data.expect_err("inner writer panicked");
    assert_eq!(
        err.to_string(),
        "BufWriter inner writer panicked, what data remains unwritten is not known"
    );
    let shown = format!("{err:?}");
    assert!(
        shown.starts_with("WriterPanicked { buffer: 9/"),
        "debug shows the buffered len and capacity, got {shown}"
    );
    let _: &dyn std::error::Error = &err;
    assert_eq!(err.into_inner(), b"some data");
}
