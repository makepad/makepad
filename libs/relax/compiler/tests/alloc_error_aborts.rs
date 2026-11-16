// An allocation error ends the process with SIGABRT after std's message, as under rustc's std:
// a catch_unwind around it catches nothing. Relax issue 231: Relax's std panicked instead, so a
// program could catch its own out-of-memory and go on. Each case runs in a forked child, whose
// stderr the parent reads.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::alloc::{handle_alloc_error, Layout};
use std::panic::{catch_unwind, AssertUnwindSafe};

extern "C" {
    fn fork() -> i32;
    fn pipe(fds: *mut i32) -> i32;
    fn dup2(from: i32, to: i32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    fn _exit(code: i32) -> !;
}

/// Runs `f` under catch_unwind in a forked child, its stderr piped here: how the child ended, and
/// whether its stderr has `message`.
fn in_child(f: fn(), message: &str) -> String {
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
    let pid = unsafe { fork() };
    assert!(pid >= 0, "fork");
    if pid == 0 {
        unsafe {
            dup2(fds[1], 2);
            close(fds[0]);
            close(fds[1]);
        }
        let r = catch_unwind(AssertUnwindSafe(f));
        // reached only when the error was caught (3) or returned (4)
        unsafe { _exit(if r.is_err() { 3 } else { 4 }) }
    }
    unsafe { close(fds[1]) };
    let mut err = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = unsafe { read(fds[0], buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            break;
        }
        err.extend_from_slice(&buf[..n as usize]);
    }
    unsafe { close(fds[0]) };
    let mut status = 0;
    assert_eq!(unsafe { waitpid(pid, &mut status, 0) }, pid);
    let ended = match status & 0x7f {
        0 => format!("exit {}", (status >> 8) & 0xff),
        sig => format!("signal {sig}"),
    };
    format!("{ended}, message {}", String::from_utf8_lossy(&err).contains(message))
}

#[test]
fn a01_handle_alloc_error_aborts() {
    let r = in_child(|| handle_alloc_error(Layout::from_size_align(1 << 20, 8).unwrap()), "memory allocation of 1048576 bytes failed\n");
    println!("| handle_alloc_error: {r}");
}

#[test]
fn a02_a_refused_allocation_aborts() {
    // more than any system gives (a valid layout: at most isize::MAX bytes)
    let r = in_child(
        || {
            let v: Vec<u8> = Vec::with_capacity(isize::MAX as usize - 7);
            std::hint::black_box(&v);
        },
        "memory allocation of 9223372036854775800 bytes failed\n",
    );
    println!("| Vec::with_capacity: {r}");
}
