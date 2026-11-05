// std surface the async/crypto dependency stack reaches for (tokio, mio, socket2, ring, httparse,
// cc): PhantomPinned's derives, Pin::as_deref_mut, Box<Path> / Box<OsStr> Clone, Cursor over a
// boxed slice and an array, MaybeUninit's Debug, NonZero<T> -> NonZero<U> TryFrom, Wrapping's
// by-reference operators, the child pipes' IntoRawFd, TcpListener <-> OwnedFd, and a `link_name`
// given as `concat!(..)`.
use std::io::{Cursor, Write};
use std::marker::PhantomPinned;
use std::mem::MaybeUninit;
use std::num::{NonZero, Wrapping};
use std::pin::Pin;

#[test]
fn a01_phantom_pinned_derives() {
    let p = PhantomPinned;
    println!("| {:?} eq {} ord {:?} default {:?}", p, p == PhantomPinned, p.cmp(&PhantomPinned), PhantomPinned::default());
    let mut h = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&p, &mut h);
    println!("| hashed");
}

#[test]
fn a02_pin_as_deref_mut() {
    let mut boxed: Pin<Box<[u8; 3]>> = Box::pin([1, 2, 3]);
    let outer: Pin<&mut Pin<Box<[u8; 3]>>> = Pin::new(&mut boxed);
    let inner: Pin<&mut [u8; 3]> = outer.as_deref_mut();
    println!("| {:?} {}", &*inner, std::mem::size_of_val(&*inner));
}

#[test]
fn a03_boxed_path_and_os_str_clone() {
    let p: Box<std::path::Path> = Box::from(std::path::Path::new("a/b.txt"));
    let q = p.clone();
    let s: Box<std::ffi::OsStr> = Box::from(std::ffi::OsStr::new("name"));
    let t = s.clone();
    println!("| {:?} {:?} {}", q, t, q.extension().is_some());
    let v: Box<[Box<std::ffi::OsStr>]> = vec![s].into_boxed_slice();
    println!("| {}", v.clone().len());
}

#[test]
fn a04_cursor_boxed_slice_and_array() {
    let mut c = Cursor::new(vec![0u8; 6].into_boxed_slice());
    let n = c.write(b"abcdefgh").unwrap();
    println!("| boxed wrote {} pos {} {:?}", n, c.position(), c.get_ref());
    let mut a = Cursor::new([0u8; 4]);
    a.set_position(2);
    let n = a.write(b"xyz").unwrap();
    let flushed = a.flush().is_ok();
    println!("| array wrote {} {:?} flush {:?}", n, a.get_ref(), flushed);
}

#[test]
fn a05_maybe_uninit_debug() {
    let m = MaybeUninit::new(5u8);
    let s: &[MaybeUninit<u8>] = std::slice::from_ref(&m);
    println!("| {:?} {:?}", m, s);
}

#[test]
fn a06_nonzero_try_from_nonzero() {
    let a = NonZero::new(300usize).unwrap();
    let b: Result<NonZero<u32>, _> = a.try_into();
    let c: Result<NonZero<u8>, _> = a.try_into();
    let d: NonZero<u64> = NonZero::new(7u8).unwrap().into();
    println!("| {:?} {} {:?}", b, c.is_err(), d);
    let e: Result<NonZero<i8>, _> = NonZero::new(-7i64).unwrap().try_into();
    println!("| {:?}", e);
}

#[test]
fn a07_wrapping_by_reference() {
    let a = Wrapping(0xffff_fff0u32);
    let b = Wrapping(0x20u32);
    let w = [b, Wrapping(3)];
    let mut acc = a;
    for x in w.iter() {
        acc = acc + x;
    }
    println!("| {:?} {:?} {:?} {:?}", acc, &a - &b, &a * b, a ^ &b);
    println!("| {:?} {:?} {:?}", &a | &b, a & &b, &a % b);
}

#[cfg(unix)]
#[test]
fn a08_child_pipe_fds_and_listener_fd() {
    use std::os::fd::{IntoRawFd, OwnedFd};
    let mut child = std::process::Command::new("cat").stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
    let fd_in = child.stdin.take().unwrap().into_raw_fd();
    let fd_err = child.stderr.take().unwrap().into_raw_fd();
    println!("| fds {} {}", fd_in > 2, fd_err > 2);
    unsafe {
        drop(<OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd_in));
        drop(<OwnedFd as std::os::fd::FromRawFd>::from_raw_fd(fd_err));
    }
    let _ = child.wait();
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let fd: OwnedFd = l.into();
    let l2 = std::net::TcpListener::from(fd);
    println!("| listener {}", l2.local_addr().unwrap().port() == port);
}

extern "C" {
    #[link_name = concat!("ab", "s")]
    fn abs_through_concat(x: i32) -> i32;
}

#[test]
fn a09_link_name_from_concat() {
    println!("| {}", unsafe { abs_through_concat(-41) });
}

#[cfg(target_arch = "aarch64")]
#[test]
fn a10_neon_not_and_immediate_shift() {
    use std::arch::aarch64::*;
    unsafe {
        let v = vdupq_n_u8(0x0f);
        let n: [u8; 16] = std::mem::transmute(vmvnq_u8(v));
        let s: [u8; 16] = std::mem::transmute(vshrq_n_u8(v, 2));
        println!("| {:02x} {:02x}", n[0], s[0]);
    }
}
