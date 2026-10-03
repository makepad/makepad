//! Panic entry points. Rapid lowers `panic!` to `panic_fmt` (coord.md S2/S3); bounds and
//! unwrap failures call the specific helpers so their messages match real core exactly.
//! The message is formatted into a fixed stack buffer (no allocation), the hook (if std set
//! one) runs, then `intrinsics_rt::panic_impl` reports and unwinds (live) or aborts (AOT).

use crate::fmt;
use crate::panic::{Location, PanicInfo};

/// Set by std::panic::set_hook (std-os lane); None = default hook (the runtime's report).
static mut HOOK: Option<fn(&PanicInfo<'_>)> = None;
/// Nesting depth: a panic inside the hook or while formatting aborts.
static mut DEPTH: u32 = 0;

pub fn set_hook_fn(hook: Option<fn(&PanicInfo<'_>)>) {
    unsafe { HOOK = hook }
}

pub fn hook_fn() -> Option<fn(&PanicInfo<'_>)> {
    unsafe { HOOK }
}

struct StackBuf {
    buf: [u8; 4096],
    len: usize,
}

impl fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let b = s.as_bytes();
        let room = self.buf.len() - self.len;
        let mut n = if b.len() < room { b.len() } else { room };
        // never cut a UTF-8 sequence
        while n > 0 && n < b.len() && (b[n] & 0xc0) == 0x80 {
            n -= 1;
        }
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        Ok(())
    }
}

#[track_caller]
pub fn panic_fmt(args: fmt::Arguments<'_>) -> ! {
    panic_at(args, Location::caller())
}

pub fn panic_at(args: fmt::Arguments<'_>, loc: &Location<'_>) -> ! {
    unsafe {
        DEPTH += 1;
        if DEPTH > 2 {
            crate::intrinsics_rt::abort();
        }
        if DEPTH == 1 {
            if let Some(h) = HOOK {
                h(&PanicInfo::new(args, loc));
            }
        }
        let mut sb = StackBuf { buf: [0; 4096], len: 0 };
        let _ = fmt::write(&mut sb, args);
        DEPTH -= 1;
        let file = loc.file();
        crate::intrinsics_rt::panic_impl(
            sb.buf.as_ptr(),
            sb.len,
            file.as_ptr(),
            file.len(),
            loc.line(),
            loc.column(),
        )
    }
}

#[track_caller]
pub fn panic_str(msg: &'static str) -> ! {
    panic_fmt(format_args!("{}", msg))
}

#[track_caller]
pub fn panic_display<T: fmt::Display + ?Sized>(x: &T) -> ! {
    panic_fmt(format_args!("{}", x))
}

#[track_caller]
pub fn panic_bounds(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("index out of bounds: the len is {} but the index is {}", len, index))
}

#[track_caller]
pub fn slice_index_order_fail(index: usize, end: usize) -> ! {
    panic_fmt(format_args!("slice index starts at {} but ends at {}", index, end))
}

#[track_caller]
pub fn slice_start_index_len_fail(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("range start index {} out of range for slice of length {}", index, len))
}

#[track_caller]
pub fn slice_end_index_len_fail(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("range end index {} out of range for slice of length {}", index, len))
}

/// `assert_eq!` (kind 0) / `assert_ne!` (kind 1) failure.
#[track_caller]
pub fn assert_failed(kind: u8, left: &dyn fmt::Debug, right: &dyn fmt::Debug, args: Option<fmt::Arguments<'_>>) -> ! {
    let op = if kind == 0 { "==" } else { "!=" };
    match args {
        Some(a) => panic_fmt(format_args!(
            "assertion `left {} right` failed: {}\n  left: {:?}\n right: {:?}",
            op, a, left, right
        )),
        None => panic_fmt(format_args!("assertion `left {} right` failed\n  left: {:?}\n right: {:?}", op, left, right)),
    }
}
