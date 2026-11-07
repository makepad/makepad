//! Panic entry points. Relax lowers `panic!` to `panic_fmt` (coord.md S2/S3); bounds and
//! unwrap failures call the specific helpers so their messages match real core exactly.
//! The message is formatted into a fixed stack buffer (no allocation), the hook (if std set
//! one) runs, then `intrinsics_rt::panic_impl` reports and unwinds (live) or aborts (AOT).

use crate::fmt;
use crate::panic::{Location, PanicInfo};

/// Set by std::panic::set_hook (std-os lane); None = default hook (the runtime's report).
static mut HOOK: Option<fn(&PanicInfo<'_>)> = None;
/// Nesting depth: a panic inside the hook or while formatting aborts. Process-wide until std
/// installs its per-thread counter (DEPTH_FN), so concurrent panics on different threads do not
/// see each other as nested.
static mut DEPTH: u32 = 0;
/// std's per-thread depth: called with +1 on entry (returns the new depth) and -1 on exit.
static mut DEPTH_FN: Option<fn(i32) -> u32> = None;

pub fn set_hook_fn(hook: Option<fn(&PanicInfo<'_>)>) {
    unsafe { HOOK = hook }
}

pub fn set_depth_fn(f: fn(i32) -> u32) {
    unsafe { DEPTH_FN = Some(f) }
}

fn depth_step(delta: i32) -> u32 {
    unsafe {
        match DEPTH_FN {
            Some(f) => f(delta),
            None => {
                DEPTH = (DEPTH as i32 + delta) as u32;
                DEPTH
            }
        }
    }
}

pub fn hook_fn() -> Option<fn(&PanicInfo<'_>)> {
    unsafe { HOOK }
}

/// The message as the runtime's report gets it, in a fixed stack buffer (no allocation). One
/// that does not fit keeps its head and its end, and names the bytes cut between them: the end
/// of a long message is often what it is about (a child process's failure after its log).
struct StackBuf {
    buf: [u8; 4096],
    /// bytes of `buf` in use: the message so far, or, once it is cut, the head, the room for
    /// the mark and the tail
    len: usize,
    /// whether the message is cut (the head is `buf[..head]`, the tail `buf[HEAD + MARK..len]`)
    cut: bool,
    head: usize,
    /// the bytes left out between the head and the tail
    dropped: usize,
}

/// A cut message's head, and the room after it for the mark that names the cut.
const HEAD: usize = 1024;
const MARK: usize = 48;

impl StackBuf {
    const fn new() -> Self {
        StackBuf { buf: [0; 4096], len: 0, cut: false, head: 0, dropped: 0 }
    }

    /// The message: as written, or its head, the mark and its tail.
    fn finish(&mut self) -> &[u8] {
        if !self.cut {
            return &self.buf[..self.len];
        }
        // the tail starts at a character
        let mut t = HEAD + MARK;
        while t < self.len && (self.buf[t] & 0xc0) == 0x80 {
            t += 1;
            self.dropped += 1;
        }
        let mut mark = [0u8; MARK];
        let mut n = 0;
        for &c in b"\n[... " {
            mark[n] = c;
            n += 1;
        }
        let (mut digits, mut d, mut v) = ([0u8; 20], 0, self.dropped);
        loop {
            digits[d] = b'0' + (v % 10) as u8;
            d += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while d > 0 {
            d -= 1;
            mark[n] = digits[d];
            n += 1;
        }
        for &c in b" bytes cut ...]\n" {
            mark[n] = c;
            n += 1;
        }
        let h = self.head;
        self.buf[h..h + n].copy_from_slice(&mark[..n]);
        self.buf.copy_within(t..self.len, h + n);
        self.len = h + n + (self.len - t);
        &self.buf[..self.len]
    }
}

impl fmt::Write for StackBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut b = s.as_bytes();
        let cap = self.buf.len();
        if !self.cut {
            let room = cap - self.len;
            if b.len() <= room {
                self.buf[self.len..self.len + b.len()].copy_from_slice(b);
                self.len += b.len();
                return Ok(());
            }
            // a full buffer: its head ends at a character, the bytes after the mark's room
            // start the tail
            self.buf[self.len..].copy_from_slice(&b[..room]);
            self.len = cap;
            b = &b[room..];
            let mut h = HEAD;
            while h > 0 && (self.buf[h] & 0xc0) == 0x80 {
                h -= 1;
            }
            self.dropped += HEAD + MARK - h;
            self.cut = true;
            self.head = h;
        }
        // the tail is the last bytes written
        let room = cap - HEAD - MARK;
        if b.len() > room {
            self.dropped += b.len() - room;
            b = &b[b.len() - room..];
        }
        let over = (self.len + b.len()).saturating_sub(cap);
        if over > 0 {
            self.buf.copy_within(HEAD + MARK + over..self.len, HEAD + MARK);
            self.len -= over;
            self.dropped += over;
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

#[inline(never)]
#[cold]
#[track_caller]
#[cfg_attr(not(relax_check), lang = "panic_fmt")]
#[rustc_do_not_const_check] // hooked by const-eval
pub const fn panic_fmt(args: fmt::Arguments<'_>) -> ! {
    panic_at(args, Location::caller())
}

#[inline(never)]
#[cold]
#[cfg_attr(relax_web, relax(keep_reachable))]
#[rustc_do_not_const_check] // panic_fmt's body (hooked by const-eval)
pub const fn panic_at(args: fmt::Arguments<'_>, loc: &Location<'_>) -> ! {
    unsafe {
        let depth = depth_step(1);
        #[cfg(relax_web)]
        if depth > 1 {
            crate::intrinsics_rt::abort();
        }
        if depth > 2 {
            crate::intrinsics_rt::abort();
        }
        if depth == 1 {
            if let Some(h) = HOOK {
                h(&PanicInfo::new(args, loc));
            }
        }
        // wasm panic=abort: the installed hook formats and logs in program code;
        // there is no unwind payload or second host report after the hook.
        #[cfg(relax_web)]
        crate::intrinsics_rt::abort();
        #[cfg(not(relax_web))]
        {
        let mut sb = StackBuf::new();
        let _ = fmt::write(&mut sb, args);
        depth_step(-1);
        let message = sb.finish();
        let file = loc.file();
        crate::intrinsics_rt::panic_impl(
            message.as_ptr(),
            message.len(),
            file.as_ptr(),
            file.len(),
            loc.line(),
            loc.column(),
        )
        }
    }
}

/// A constant message: like real core's `panic(&'static str)` its payload is the `&'static str`.
#[inline(never)]
#[cold]
#[track_caller]
pub const fn panic_str(msg: &'static str) -> ! {
    let pieces = [msg];
    panic_fmt(fmt::Arguments::new_const(&pieces))
}

#[track_caller]
#[cfg_attr(not(relax_check), lang = "panic_display")]
#[rustc_do_not_const_check] // hooked by const-eval
pub const fn panic_display<T: fmt::Display>(x: &T) -> ! {
    panic_fmt(format_args!("{}", x))
}

#[inline(never)]
#[cold]
#[track_caller]
pub fn panic_bounds(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("index out of bounds: the len is {} but the index is {}", len, index))
}

#[inline(never)]
#[cold]
#[track_caller]
pub fn slice_index_order_fail(index: usize, end: usize) -> ! {
    panic_fmt(format_args!("slice index starts at {} but ends at {}", index, end))
}

#[inline(never)]
#[cold]
#[track_caller]
pub fn slice_start_index_len_fail(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("range start index {} out of range for slice of length {}", index, len))
}

#[inline(never)]
#[cold]
#[track_caller]
pub fn slice_end_index_len_fail(index: usize, len: usize) -> ! {
    panic_fmt(format_args!("range end index {} out of range for slice of length {}", index, len))
}

/// `assert_eq!` (kind 0) / `assert_ne!` (kind 1) failure.
#[inline(never)]
#[cold]
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
