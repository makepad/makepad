//! std::backtrace (library/std/src/backtrace.rs @1.100.0). `capture` (when RUST_LIB_BACKTRACE,
//! else RUST_BACKTRACE, is set and not "0"; read once) and `force_capture` take the thread's
//! frames from the runtime (inspect/backtrace.rs `relax_backtrace_capture`: JIT frames, and host
//! frames between them). Their names and source positions are resolved each time the backtrace is
//! printed (`relax_backtrace_resolve`: JIT fns by the unit, an optimised body's inlined fns each a
//! symbol of its own, innermost first, as backtrace-rs resolves them; host fns by their symbol
//! tables). Display and Debug print what real std prints: from the caller of the capture on,
//! paths under the current directory as `./...`; `{:#}` every frame with its address. Where
//! nothing walks the stack (a sandboxed module, the web) a capture is Unsupported, as real std's
//! on a target without unwind info.
//!
//! The default panic hook's RUST_BACKTRACE output (library/std/src/sys/backtrace.rs `_print_fmt`)
//! is `print_panic_backtrace`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::path::{Path, PathBuf, MAIN_SEPARATOR};
use crate::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BacktraceStatus {
    Unsupported,
    Disabled,
    Captured,
}

pub struct Backtrace {
    inner: Inner,
}

enum Inner {
    Unsupported,
    Disabled,
    /// return addresses, innermost first, from the capture's own frames on
    Captured(Vec<u64>),
}

#[cfg(not(relax_web))]
extern "C" {
    fn relax_backtrace_capture(out: *mut u64, cap: usize) -> usize;
    fn relax_backtrace_resolve(pcs: *const u64, n: usize, out: *mut u8, cap: usize) -> usize;
}

impl Backtrace {
    /// real std's `enabled`: the variables are read once
    fn enabled() -> bool {
        static ENABLED: AtomicU8 = AtomicU8::new(0);
        match ENABLED.load(Ordering::Relaxed) {
            0 => {}
            1 => return false,
            _ => return true,
        }
        let enabled = match crate::env::var("RUST_LIB_BACKTRACE") {
            Ok(s) => s != "0",
            Err(_) => match crate::env::var("RUST_BACKTRACE") {
                Ok(s) => s != "0",
                Err(_) => false,
            },
        };
        ENABLED.store(enabled as u8 + 1, Ordering::Relaxed);
        enabled
    }

    #[inline(never)]
    pub fn capture() -> Backtrace {
        if !Backtrace::enabled() {
            return Backtrace { inner: Inner::Disabled };
        }
        Backtrace::create()
    }

    #[inline(never)]
    pub fn force_capture() -> Backtrace {
        Backtrace::create()
    }

    pub const fn disabled() -> Backtrace {
        Backtrace { inner: Inner::Disabled }
    }

    #[inline(never)]
    fn create() -> Backtrace {
        let pcs = capture_pcs();
        if pcs.is_empty() {
            Backtrace { inner: Inner::Unsupported }
        } else {
            Backtrace { inner: Inner::Captured(pcs) }
        }
    }

    pub fn status(&self) -> BacktraceStatus {
        match self.inner {
            Inner::Unsupported => BacktraceStatus::Unsupported,
            Inner::Disabled => BacktraceStatus::Disabled,
            Inner::Captured(_) => BacktraceStatus::Captured,
        }
    }
}

/// The calling thread's return addresses (none where nothing walks the stack).
#[inline(never)]
fn capture_pcs() -> Vec<u64> {
    #[cfg(not(relax_web))]
    {
        let mut v: Vec<u64> = Vec::with_capacity(64);
        loop {
            let n = unsafe { relax_backtrace_capture(v.as_mut_ptr(), v.capacity()) };
            if n <= v.capacity() {
                unsafe { v.set_len(n) };
                return v;
            }
            v.reserve(n + 16);
        }
    }
    #[cfg(relax_web)]
    Vec::new()
}

/// One symbol of a frame: an inlined fn, or the frame's own.
struct Symbol {
    name: String,
    file: String,
    line: Option<u32>,
    col: Option<u32>,
}

/// The symbols of each frame of `pcs` (empty: nothing known about it).
fn resolve(pcs: &[u64]) -> Vec<Vec<Symbol>> {
    let mut frames: Vec<Vec<Symbol>> = Vec::new();
    frames.resize_with(pcs.len(), Vec::new);
    #[cfg(not(relax_web))]
    {
        let mut buf: Vec<u8> = Vec::with_capacity(pcs.len() * 160);
        loop {
            let n = unsafe { relax_backtrace_resolve(pcs.as_ptr(), pcs.len(), buf.as_mut_ptr(), buf.capacity()) };
            if n <= buf.capacity() {
                unsafe { buf.set_len(n) };
                break;
            }
            buf.reserve(n);
        }
        let text = String::from_utf8_lossy(&buf);
        // `<frame>\t<name>\t<file>\t<line>\t<col>` per symbol
        for l in text.split('\n') {
            let mut w = l.split('\t');
            let (Some(k), Some(name), Some(file), Some(line), Some(col)) = (w.next(), w.next(), w.next(), w.next(), w.next()) else { continue };
            let Ok(k) = k.parse::<usize>() else { continue };
            if k < frames.len() {
                frames[k].push(Symbol { name: String::from(name), file: String::from(file), line: line.parse().ok(), col: col.parse().ok() });
            }
        }
    }
    frames
}

/// Where real std's short form starts: after the capture's own frames (its `actual_start`).
fn after_capture(syms: &[(usize, Option<&Symbol>)]) -> usize {
    syms.iter().position(|(_, s)| !s.is_some_and(|s| s.name.starts_with("std::backtrace::"))).unwrap_or(syms.len())
}

fn sym_name<'a>(s: &Option<&'a Symbol>) -> &'a str {
    match s {
        Some(s) => s.name.as_str(),
        None => "",
    }
}

/// Every symbol in order, a frame with none as one unknown symbol: (frame index, symbol).
fn flat(frames: &[Vec<Symbol>]) -> Vec<(usize, Option<&Symbol>)> {
    let mut v = Vec::new();
    for (k, f) in frames.iter().enumerate() {
        if f.is_empty() {
            v.push((k, None));
        }
        for s in f {
            v.push((k, Some(s)));
        }
    }
    v
}

/// backtrace-rs's output_filename: in the short form an absolute path under the current
/// directory as `./<rest>`.
fn print_path(f: &mut dyn fmt::Write, file: &str, full: bool, cwd: Option<&PathBuf>) -> fmt::Result {
    if !full {
        let p = Path::new(file);
        if let (true, Some(cwd)) = (p.is_absolute(), cwd) {
            if let Ok(rest) = p.strip_prefix(cwd) {
                if let Some(rest) = rest.to_str() {
                    return write!(f, ".{}{}", MAIN_SEPARATOR, rest);
                }
            }
        }
    }
    f.write_str(file)
}

/// backtrace-rs's BacktraceFrameFmt::print_raw_with_column: one symbol as its own numbered frame.
fn print_symbol(f: &mut dyn fmt::Write, idx: usize, ip: u64, s: Option<&Symbol>, full: bool, cwd: Option<&PathBuf>) -> fmt::Result {
    write!(f, "{:4}: ", idx)?;
    if full {
        // `{:18?}` of the frame's ip (a pointer: `{:#x}` padded to 2 + 16 digits)
        write!(f, "{:#18x} - ", ip)?;
    }
    match s {
        Some(s) if !s.name.is_empty() => f.write_str(&s.name)?,
        _ => f.write_str("<unknown>")?,
    }
    f.write_str("\n")?;
    if let Some(s) = s {
        if let (false, Some(line)) = (s.file.is_empty(), s.line) {
            if full {
                write!(f, "{:18}", "")?;
            }
            f.write_str("             at ")?;
            print_path(f, &s.file, full, cwd)?;
            write!(f, ":{}", line)?;
            if let Some(c) = s.col {
                write!(f, ":{}", c)?;
            }
            f.write_str("\n")?;
        }
    }
    Ok(())
}

/// A Formatter as a `dyn fmt::Write`.
struct Out<'a, 'b>(&'a mut fmt::Formatter<'b>);
impl fmt::Write for Out<'_, '_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.write_str(s)
    }
}

impl fmt::Display for Backtrace {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pcs = match &self.inner {
            Inner::Unsupported => return fmt.write_str("unsupported backtrace"),
            Inner::Disabled => return fmt.write_str("disabled backtrace"),
            Inner::Captured(pcs) => pcs,
        };
        let full = fmt.alternate();
        let frames = resolve(pcs);
        let syms = flat(&frames);
        let start = if full { 0 } else { after_capture(&syms) };
        let cwd = crate::env::current_dir().ok();
        let mut out = Out(fmt);
        for (idx, (k, s)) in syms[start..].iter().enumerate() {
            print_symbol(&mut out, idx, pcs[*k], *s, full, cwd.as_ref())?;
        }
        Ok(())
    }
}

/// real std's BacktraceSymbol Debug
struct SymbolDebug<'a>(&'a Symbol, Option<&'a PathBuf>);
impl fmt::Debug for SymbolDebug<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        fmt.write_str("{ ")?;
        if s.name.is_empty() {
            fmt.write_str("fn: <unknown>")?;
        } else {
            write!(fmt, "fn: \"{}\"", s.name)?;
        }
        if !s.file.is_empty() {
            fmt.write_str(", file: \"")?;
            print_path(&mut Out(fmt), &s.file, false, self.1)?;
            fmt.write_str("\"")?;
        }
        if let Some(line) = s.line {
            write!(fmt, ", line: {:?}", line)?;
        }
        fmt.write_str(" }")
    }
}

impl fmt::Debug for Backtrace {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pcs = match &self.inner {
            Inner::Unsupported => return fmt.write_str("<unsupported>"),
            Inner::Disabled => return fmt.write_str("<disabled>"),
            Inner::Captured(pcs) => pcs,
        };
        let frames = resolve(pcs);
        let syms = flat(&frames);
        let start = after_capture(&syms);
        let cwd = crate::env::current_dir().ok();
        fmt.write_str("Backtrace ")?;
        let mut dbg = fmt.debug_list();
        // (a frame with no symbol has no entry)
        for (_, s) in &syms[start..] {
            if let Some(s) = s {
                dbg.entry(&SymbolDebug(s, cwd.as_ref()));
            }
        }
        dbg.finish()
    }
}

/// The panic hook's backtrace (library/std/src/sys/backtrace.rs `_print_fmt` and the hook's
/// note): "stack backtrace:" and every frame from the panic machinery on (short: from the first
/// core::panicking / std::panicking fn, the frames real std prints after
/// `__rust_end_short_backtrace`, up to a spawned thread's start, its
/// `__rust_begin_short_backtrace`), each symbol numbered.
pub(crate) fn print_panic_backtrace(w: &mut dyn crate::io::Write, full: bool) {
    let pcs = capture_pcs();
    if pcs.is_empty() {
        return;
    }
    let frames = resolve(&pcs);
    let syms = flat(&frames);
    let (start, end) = if full {
        (0, syms.len())
    } else {
        let start = syms.iter().position(|(_, s)| sym_name(s).starts_with("core::panicking::") || sym_name(s).starts_with("std::panicking::")).unwrap_or_else(|| {
            syms.iter().position(|(_, s)| !["std::backtrace::", "std::panic::", "std::sys::rt::"].iter().any(|p| sym_name(s).starts_with(p))).unwrap_or(syms.len())
        });
        let end = syms[start..].iter().position(|(_, s)| sym_name(s).starts_with("std::thread::thread_start")).map_or(syms.len(), |e| start + e);
        (start, end)
    };
    let cwd = if full { None } else { crate::env::current_dir().ok() };
    let mut text = String::from("stack backtrace:\n");
    for (idx, (k, s)) in syms[start..end].iter().enumerate() {
        let _ = print_symbol(&mut text, idx, pcs[*k], *s, full, cwd.as_ref());
    }
    if !full {
        text.push_str("note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.\n");
    }
    let _ = w.write_all(text.as_bytes());
}
