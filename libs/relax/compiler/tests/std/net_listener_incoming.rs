// std's listener iterators (relax issue 126), as real std has them: both `Incoming`s derive Debug
// (`Incoming { listener: .. }`), TCP's is fused, and `&UnixListener` iterates over its
// connections (`for s in &listener`), with an unbounded size_hint.
use std::net::TcpListener;

fn fused<I: std::iter::FusedIterator>(_: &I) {}

#[test]
fn tcp_incoming_is_debug_and_fused() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let it = l.incoming();
    fused(&it);
    assert_eq!(format!("{:?}", it), format!("Incoming {{ listener: {:?} }}", l));
}

#[cfg(unix)]
#[test]
fn unix_listener_iterates_by_reference() {
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = std::env::temp_dir().join(format!("relax-incoming-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let l = UnixListener::bind(&path).unwrap();
    assert_eq!(format!("{:?}", l.incoming()), format!("Incoming {{ listener: {:?} }}", l));
    // two waiting connections: each accept below returns at once
    let _a = UnixStream::connect(&path).unwrap();
    let _b = UnixStream::connect(&path).unwrap();
    let mut it = (&l).into_iter();
    assert_eq!(it.size_hint(), (usize::MAX, None));
    assert!(it.next().unwrap().is_ok(), "the first connection");
    for s in &l {
        assert!(s.is_ok(), "the second connection");
        break;
    }
    let _ = std::fs::remove_file(&path);
}
