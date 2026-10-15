//! The terminal: raw mode (termios), its size, signals, reading input, and
//! decoding the bytes into keys. Plain libc calls declared here; no crates.
//!
//! Raw mode keeps output processing on (so `\n` still returns the carriage)
//! and turns off echo, line buffering, signals and flow control: Ctrl+C and
//! Ctrl+Z arrive as keys. The original settings come back on drop, on a
//! panic (the hook in `main`) and before exiting on SIGTERM / SIGHUP.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[allow(non_camel_case_types)]
mod ffi {
    use std::os::raw::{c_int, c_ulong, c_void};

    #[cfg(target_os = "linux")]
    pub type tcflag_t = u32;
    #[cfg(target_os = "macos")]
    pub type tcflag_t = c_ulong;
    #[cfg(target_os = "linux")]
    pub type speed_t = u32;
    #[cfg(target_os = "macos")]
    pub type speed_t = c_ulong;
    #[cfg(target_os = "linux")]
    pub const NCCS: usize = 32;
    #[cfg(target_os = "macos")]
    pub const NCCS: usize = 20;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct termios {
        pub c_iflag: tcflag_t,
        pub c_oflag: tcflag_t,
        pub c_cflag: tcflag_t,
        pub c_lflag: tcflag_t,
        #[cfg(target_os = "linux")]
        pub c_line: u8,
        pub c_cc: [u8; NCCS],
        pub c_ispeed: speed_t,
        pub c_ospeed: speed_t,
    }

    #[repr(C)]
    #[derive(Default)]
    pub struct winsize {
        pub ws_row: u16,
        pub ws_col: u16,
        pub ws_xpixel: u16,
        pub ws_ypixel: u16,
    }

    #[repr(C)]
    pub struct pollfd {
        pub fd: c_int,
        pub events: i16,
        pub revents: i16,
    }

    #[cfg(target_os = "linux")]
    pub type nfds_t = c_ulong;
    #[cfg(target_os = "macos")]
    pub type nfds_t = u32;

    pub const POLLIN: i16 = 1;
    pub const TCSANOW: c_int = 0;
    pub const SIGHUP: c_int = 1;
    pub const SIGINT: c_int = 2;
    pub const SIGTERM: c_int = 15;
    pub const SIGWINCH: c_int = 28;

    #[cfg(target_os = "linux")]
    pub mod flags {
        use super::tcflag_t;
        pub const ICRNL: tcflag_t = 0o400;
        pub const IXON: tcflag_t = 0o2000;
        pub const ISIG: tcflag_t = 0o1;
        pub const ICANON: tcflag_t = 0o2;
        pub const ECHO: tcflag_t = 0o10;
        pub const IEXTEN: tcflag_t = 0o100000;
        pub const VTIME: usize = 5;
        pub const VMIN: usize = 6;
        pub const TIOCGWINSZ: std::os::raw::c_ulong = 0x5413;
    }
    #[cfg(target_os = "macos")]
    pub mod flags {
        use super::tcflag_t;
        pub const ICRNL: tcflag_t = 0x100;
        pub const IXON: tcflag_t = 0x200;
        pub const ISIG: tcflag_t = 0x80;
        pub const ICANON: tcflag_t = 0x100;
        pub const ECHO: tcflag_t = 0x8;
        pub const IEXTEN: tcflag_t = 0x400;
        pub const VMIN: usize = 16;
        pub const VTIME: usize = 17;
        pub const TIOCGWINSZ: std::os::raw::c_ulong = 0x40087468;
    }

    extern "C" {
        pub fn tcgetattr(fd: c_int, t: *mut termios) -> c_int;
        pub fn tcsetattr(fd: c_int, action: c_int, t: *const termios) -> c_int;
        pub fn isatty(fd: c_int) -> c_int;
        pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
        pub fn poll(fds: *mut pollfd, n: nfds_t, timeout: c_int) -> c_int;
        pub fn read(fd: c_int, buf: *mut c_void, n: usize) -> isize;
        pub fn signal(sig: c_int, handler: usize) -> usize;
        pub fn dup(fd: c_int) -> c_int;
        pub fn dup2(fd: c_int, to: c_int) -> c_int;
    }
}

pub fn is_tty(fd: i32) -> bool {
    unsafe { ffi::isatty(fd) == 1 }
}

/// (columns, rows) of the terminal on `fd`; 80x24 when it will not say.
pub fn size(fd: i32) -> (usize, usize) {
    let mut ws = ffi::winsize::default();
    let ok = unsafe { ffi::ioctl(fd, ffi::flags::TIOCGWINSZ, &mut ws as *mut ffi::winsize) } == 0;
    if ok && ws.ws_col > 0 {
        (ws.ws_col as usize, ws.ws_row.max(1) as usize)
    } else {
        (80, 24)
    }
}

/// The settings to put back, wherever the program leaves from.
static SAVED: Mutex<Option<(i32, ffi::termios)>> = Mutex::new(None);

/// Raw mode on `fd` for as long as this lives.
pub struct RawMode;

impl RawMode {
    pub fn enable(fd: i32) -> Option<RawMode> {
        let mut t: ffi::termios = unsafe { std::mem::zeroed() };
        if unsafe { ffi::tcgetattr(fd, &mut t) } != 0 {
            return None;
        }
        *SAVED.lock().unwrap_or_else(|e| e.into_inner()) = Some((fd, t));
        let mut raw = t;
        use ffi::flags::*;
        raw.c_iflag &= !(ICRNL | IXON);
        raw.c_lflag &= !(ECHO | ICANON | ISIG | IEXTEN);
        raw.c_cc[VMIN] = 1;
        raw.c_cc[VTIME] = 0;
        if unsafe { ffi::tcsetattr(fd, ffi::TCSANOW, &raw) } != 0 {
            return None;
        }
        Some(RawMode)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        restore();
    }
}

/// Put the saved settings back (idempotent; safe from a panic hook).
pub fn restore() {
    let saved = SAVED.lock().map(|s| *s).unwrap_or(None);
    if let Some((fd, t)) = saved {
        unsafe { ffi::tcsetattr(fd, ffi::TCSANOW, &t) };
    }
}

// ------------------------------------------------------------------ signals

static RESIZED: AtomicBool = AtomicBool::new(false);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(sig: i32) {
    match sig {
        ffi::SIGWINCH => RESIZED.store(true, Ordering::SeqCst),
        ffi::SIGINT => INTERRUPTED.store(true, Ordering::SeqCst),
        _ => TERMINATED.store(true, Ordering::SeqCst),
    }
}

/// SIGWINCH, SIGINT, SIGTERM and SIGHUP set flags the main loop reads.
pub fn install_signals() {
    for sig in [ffi::SIGWINCH, ffi::SIGINT, ffi::SIGTERM, ffi::SIGHUP] {
        unsafe { ffi::signal(sig, on_signal as extern "C" fn(i32) as usize) };
    }
}

pub fn take_resized() -> bool {
    RESIZED.swap(false, Ordering::SeqCst)
}

pub fn take_interrupted() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

pub fn terminated() -> bool {
    TERMINATED.load(Ordering::SeqCst)
}

// -------------------------------------------------------------------- input

/// Wait up to `timeout` for input on `fd`, then read what is there.
/// `None` at end of input.
pub fn read_input(fd: i32, timeout: Duration) -> Option<Vec<u8>> {
    let mut p = ffi::pollfd { fd, events: ffi::POLLIN, revents: 0 };
    let n = unsafe { ffi::poll(&mut p, 1, timeout.as_millis() as i32) };
    if n <= 0 {
        return Some(Vec::new());
    }
    let mut buf = [0u8; 4096];
    let got = unsafe { ffi::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
    if got == 0 {
        return None;
    }
    if got < 0 {
        // EINTR (a signal) and EAGAIN are not the end.
        return Some(Vec::new());
    }
    Some(buf[..got as usize].to_vec())
}

/// Send fd 2 to `path` (the hub's diagnostics would tear the prompt);
/// returns a duplicate of the old stderr for the panic message.
pub fn redirect_stderr(path: &std::path::Path) -> Option<i32> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path).ok()?;
    let old = unsafe { ffi::dup(2) };
    if unsafe { ffi::dup2(file.as_raw_fd(), 2) } < 0 {
        return None;
    }
    Some(old)
}

// --------------------------------------------------------------------- keys

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Alt (or Esc then) a character.
    Alt(char),
    Ctrl(char),
    Enter,
    /// Alt+Enter: a newline in the prompt.
    AltEnter,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    WordLeft,
    WordRight,
    Esc,
    /// A bracketed paste, whole.
    Paste(String),
}

/// Bytes in, keys out. An Esc alone is only known once no more bytes
/// follow it: `flush` (after a short quiet) settles it.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    paste: Option<Vec<u8>>,
}

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

enum Step {
    Key(Key, usize),
    /// Not one key yet (a sequence cut short): wait for more bytes.
    More,
    /// Bytes that mean nothing here: drop them.
    Skip(usize),
}

impl Decoder {
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Key> {
        self.buf.extend_from_slice(bytes);
        let mut keys = Vec::new();
        loop {
            if let Some(paste) = &mut self.paste {
                match find(&self.buf, PASTE_END) {
                    Some(at) => {
                        paste.extend_from_slice(&self.buf[..at]);
                        self.buf.drain(..at + PASTE_END.len());
                        let text = String::from_utf8_lossy(&self.paste.take().unwrap_or_default()).into_owned();
                        keys.push(Key::Paste(text.replace("\r\n", "\n").replace('\r', "\n")));
                        continue;
                    }
                    None => {
                        // Keep a tail that might be the start of the end marker.
                        let keep = (0..PASTE_END.len().min(self.buf.len() + 1))
                            .rev()
                            .find(|&n| n <= self.buf.len() && PASTE_END.starts_with(&self.buf[self.buf.len() - n..]))
                            .unwrap_or(0);
                        let cut = self.buf.len() - keep;
                        paste.extend_from_slice(&self.buf[..cut]);
                        self.buf.drain(..cut);
                        return keys;
                    }
                }
            }
            if self.buf.is_empty() {
                return keys;
            }
            if self.buf.starts_with(PASTE_START) {
                self.buf.drain(..PASTE_START.len());
                self.paste = Some(Vec::new());
                continue;
            }
            match decode(&self.buf) {
                Step::Key(k, n) => {
                    self.buf.drain(..n);
                    keys.push(k);
                }
                Step::Skip(n) => {
                    self.buf.drain(..n);
                }
                Step::More => return keys,
            }
        }
    }

    /// Nothing more came: a lone Esc is the Esc key; a cut sequence is dropped.
    pub fn flush(&mut self) -> Vec<Key> {
        if self.paste.is_some() || self.buf.is_empty() {
            return Vec::new();
        }
        let keys = if self.buf == b"\x1b" { vec![Key::Esc] } else { Vec::new() };
        self.buf.clear();
        keys
    }

    pub fn pending(&self) -> bool {
        !self.buf.is_empty() && self.paste.is_none()
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn decode(b: &[u8]) -> Step {
    let c = b[0];
    match c {
        0x1b => decode_esc(b),
        b'\r' | b'\n' => Step::Key(Key::Enter, 1),
        b'\t' => Step::Key(Key::Tab, 1),
        0x7f | 0x08 => Step::Key(Key::Backspace, 1),
        0x00 => Step::Skip(1),
        0x01..=0x1a => Step::Key(Key::Ctrl((b'a' + c - 1) as char), 1),
        0x1c..=0x1f => Step::Skip(1),
        _ => match utf8_char(b) {
            Some(Ok((ch, n))) => Step::Key(Key::Char(ch), n),
            Some(Err(n)) => Step::Skip(n),
            None => Step::More,
        },
    }
}

/// One UTF-8 character at the start of `b`: `Ok((char, len))`, `Err(len)`
/// for bad bytes, `None` when it is cut short.
fn utf8_char(b: &[u8]) -> Option<Result<(char, usize), usize>> {
    let n = match b[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return Some(Err(1)),
    };
    if b.len() < n {
        return None;
    }
    match std::str::from_utf8(&b[..n]) {
        Ok(s) => Some(Ok((s.chars().next().unwrap_or('\u{fffd}'), n))),
        Err(_) => Some(Err(1)),
    }
}

fn decode_esc(b: &[u8]) -> Step {
    if b.len() == 1 {
        return Step::More;
    }
    match b[1] {
        b'[' => decode_csi(b),
        b'O' => {
            if b.len() < 3 {
                return Step::More;
            }
            let k = match b[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                b'H' => Key::Home,
                b'F' => Key::End,
                _ => return Step::Skip(3),
            };
            Step::Key(k, 3)
        }
        b'\r' | b'\n' => Step::Key(Key::AltEnter, 2),
        0x7f => Step::Key(Key::Ctrl('w'), 2),
        0x1b => Step::Key(Key::Esc, 1),
        _ => match utf8_char(&b[1..]) {
            Some(Ok((ch, n))) => Step::Key(
                match ch {
                    'b' => Key::WordLeft,
                    'f' => Key::WordRight,
                    ch => Key::Alt(ch),
                },
                1 + n,
            ),
            Some(Err(n)) => Step::Skip(1 + n),
            None => Step::More,
        },
    }
}

/// `ESC [ params final`.
fn decode_csi(b: &[u8]) -> Step {
    let mut i = 2;
    while i < b.len() && (0x30..=0x3f).contains(&b[i]) {
        i += 1;
    }
    while i < b.len() && (0x20..=0x2f).contains(&b[i]) {
        i += 1;
    }
    if i >= b.len() {
        return Step::More;
    }
    let fin = b[i];
    let params = std::str::from_utf8(&b[2..i]).unwrap_or("");
    let mut nums = params.split(';').map(|p| p.parse::<u32>().unwrap_or(0));
    let first = nums.next().unwrap_or(0);
    let modifier = nums.next().unwrap_or(1);
    let ctrl = modifier >= 2 && (modifier - 1) & 4 != 0;
    let alt = modifier >= 2 && (modifier - 1) & 2 != 0;
    let k = match fin {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' if ctrl || alt => Key::WordRight,
        b'D' if ctrl || alt => Key::WordLeft,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'Z' => Key::BackTab,
        b'~' => match first {
            1 | 7 => Key::Home,
            4 | 8 => Key::End,
            3 => Key::Delete,
            _ => return Step::Skip(i + 1),
        },
        // `CSI 13;2u` / `CSI 13;3u`: a terminal that reports modified Enter.
        b'u' if first == 13 && modifier >= 2 => Key::AltEnter,
        _ => return Step::Skip(i + 1),
    };
    Step::Key(k, i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(bytes: &[u8]) -> Vec<Key> {
        let mut d = Decoder::default();
        let mut k = d.feed(bytes);
        k.extend(d.flush());
        k
    }

    #[test]
    fn plain_and_control() {
        assert_eq!(keys(b"a\x01\x05\r\x7f"), [Key::Char('a'), Key::Ctrl('a'), Key::Ctrl('e'), Key::Enter, Key::Backspace]);
        assert_eq!(keys("é日".as_bytes()), [Key::Char('é'), Key::Char('日')]);
        assert_eq!(keys(b"\x03\x04\t"), [Key::Ctrl('c'), Key::Ctrl('d'), Key::Tab]);
    }

    #[test]
    fn arrows_and_editing_keys() {
        assert_eq!(keys(b"\x1b[A\x1b[B\x1b[C\x1b[D"), [Key::Up, Key::Down, Key::Right, Key::Left]);
        assert_eq!(keys(b"\x1bOH\x1b[F\x1b[3~\x1b[1~\x1b[4~"), [Key::Home, Key::End, Key::Delete, Key::Home, Key::End]);
        assert_eq!(keys(b"\x1b[1;5C\x1b[1;5D"), [Key::WordRight, Key::WordLeft]);
        assert_eq!(keys(b"\x1b[Z"), [Key::BackTab]);
    }

    #[test]
    fn esc_alone_vs_alt() {
        // A lone Esc waits for the quiet that settles it.
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b").is_empty());
        assert!(d.pending());
        assert_eq!(d.flush(), [Key::Esc]);
        // Esc followed by a key in the same read is Alt.
        assert_eq!(keys(b"\x1bx"), [Key::Alt('x')]);
        assert_eq!(keys(b"\x1b\r"), [Key::AltEnter]);
        assert_eq!(keys(b"\x1bb\x1bf"), [Key::WordLeft, Key::WordRight]);
        // Two Escs: the first is a key of its own.
        assert_eq!(keys(b"\x1b\x1b"), [Key::Esc, Key::Esc]);
    }

    #[test]
    fn sequences_split_across_reads() {
        let mut d = Decoder::default();
        assert!(d.feed(b"\x1b[").is_empty());
        assert_eq!(d.feed(b"A"), [Key::Up]);
        assert!(d.feed(&"日".as_bytes()[..1]).is_empty());
        assert_eq!(d.feed(&"日".as_bytes()[1..]), [Key::Char('日')]);
    }

    #[test]
    fn bracketed_paste_is_one_key() {
        assert_eq!(keys(b"x\x1b[200~line one\r\nline\ttwo\x1b[201~y"), [Key::Char('x'), Key::Paste("line one\nline\ttwo".into()), Key::Char('y')]);
        // Split anywhere, the end marker too.
        let mut d = Decoder::default();
        let mut got = d.feed(b"\x1b[200~ab\x1b[2");
        assert!(got.is_empty());
        got.extend(d.feed(b"01~"));
        assert_eq!(got, [Key::Paste("ab".into())]);
        // An Esc inside a paste is text, not a key.
        assert_eq!(keys(b"\x1b[200~a\x1bb\x1b[201~"), [Key::Paste("a\x1bb".into())]);
    }
}
