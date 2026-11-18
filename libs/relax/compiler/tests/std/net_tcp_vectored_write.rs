//! A TcpStream's vectored write sends its buffers in one call, as real std's writev(2) does: the
//! bytes of every buffer in order, empty ones skipped, and at most the OS's limit of buffers in
//! one call (1024 on Linux and Apple targets), through the stream, through `&TcpStream` and from
//! the accepted side. Relax's std had the trait's default, which writes the first non-empty buffer
//! alone, so a caller that trusts one vectored write to take a whole small message lost the rest
//! (relax PR 233). A differential file of agree.sh's `named` list.
#![cfg(unix)]
use std::io::{ErrorKind, IoSlice, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};

#[test]
fn a_vectored_write_sends_every_buffer() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut tx = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut rx, _) = listener.accept().unwrap();
    let parts = [IoSlice::new(b"head:"), IoSlice::new(b""), IoSlice::new(b"body"), IoSlice::new(b";")];
    assert_eq!(tx.write_vectored(&parts).unwrap(), 10, "every buffer's bytes, the empty one skipped");
    assert_eq!((&tx).write_vectored(&parts[2..]).unwrap(), 5, "the same through &TcpStream");
    // no buffers: writev(2) with none, as real std calls it, which Linux answers with 0 and
    // Apple's kernel refuses (EINVAL: its man page's "iovcnt ... less than or equal to 0")
    let none = tx.write_vectored(&[]);
    if cfg!(target_vendor = "apple") {
        assert_eq!(none.unwrap_err().kind(), std::io::ErrorKind::InvalidInput, "no buffers on an Apple target");
    } else {
        assert_eq!(none.unwrap(), 0, "no buffers write nothing");
    }
    // one byte per buffer: a call takes the OS's limit of them, the rest is the next write's
    let bytes: Vec<u8> = (0..1100u32).map(|i| (i % 251) as u8).collect();
    let many: Vec<IoSlice> = bytes.chunks(1).map(IoSlice::new).collect();
    assert_eq!(tx.write_vectored(&many).unwrap(), 1024, "at most 1024 buffers in one call");
    let mut rest: Vec<IoSlice> = bytes[1024..].chunks(1).map(IoSlice::new).collect();
    let mut rest = &mut rest[..];
    while !rest.is_empty() {
        let n = tx.write_vectored(rest).unwrap();
        IoSlice::advance_slices(&mut rest, n);
    }
    tx.shutdown(Shutdown::Write).unwrap();
    let after = tx.write_vectored(&parts).unwrap_err();
    assert_eq!(after.kind(), ErrorKind::BrokenPipe, "a write after the write half's shutdown is EPIPE: {after}");
    // the accepted socket writes the same way (its read half is open on the other side)
    assert_eq!(rx.write_vectored(&parts).unwrap(), 10, "a vectored write from the accepted socket");
    let mut back = [0u8; 10];
    tx.read_exact(&mut back).unwrap();
    assert_eq!(&back, b"head:body;", "the connecting side reads the accepted side's buffers");
    let mut got = Vec::new();
    rx.read_to_end(&mut got).unwrap();
    let mut want = b"head:body;body;".to_vec();
    want.extend_from_slice(&bytes);
    assert_eq!(got, want, "the peer reads every byte, in order");
}
