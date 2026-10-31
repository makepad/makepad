//! The boot progress line of `makepad run` on a terminal: one line redrawn in place while the
//! program boots (phase, counts, time), replaced at first present by one summary line.
//!
//! How it stays out of the way: fd 2 writes directly to a private session log. One thread forwards
//! everything written to it (errors, warnings, the program's own stderr, [I] log lines) to the real
//! terminal, erasing the bar before each batch of lines and drawing it again after, so output
//! prints above the bar and nothing is garbled. The thread redraws at most ~20 times a second from
//! plain atomics (phase, crate counter, compiled functions); the compile path takes no lock and
//! does no extra work. At first present only the bar ends; the transcript continues until exit.
//! Fatal fault reports bypass the forwarding pipe through the saved stderr descriptor.
//! Only a confirmed clean exit deletes the transcript. Non-terminal runs keep plain output.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

pub static LOAD_DONE: AtomicU32 = AtomicU32::new(0);
pub static LOAD_TOTAL: AtomicU32 = AtomicU32::new(0);
static PHASE: AtomicU8 = AtomicU8::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Item tracking (the bar, or --verbose's slowest lists) is on.
static TRACK: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
static ABNORMAL: AtomicBool = AtomicBool::new(false);

pub(crate) fn exit_code(code: i32) { EXIT_CODE.store(code, Ordering::Release); }
pub(crate) fn abnormal() { ABNORMAL.store(true, Ordering::Release); }

// ---- what each compiling thread is on: one slot per thread, written without locks.
// A slot is a sequence number (odd while written), the start (ms since process start, +1) and
// the first 96 bytes of the name; the ticker reads it and drops a torn read.
const SLOTS: usize = 32;
const WORDS: usize = 12;
struct Slot {
    seq: AtomicU32,
    start: AtomicU64,
    len: AtomicU32,
    name: [AtomicU64; WORDS],
}
#[allow(clippy::declare_interior_mutable_const)]
const SLOT0: Slot = Slot { seq: AtomicU32::new(0), start: AtomicU64::new(0), len: AtomicU32::new(0), name: [const { AtomicU64::new(0) }; WORDS] };
static ITEMS: [Slot; SLOTS] = [SLOT0; SLOTS];
static NEXT_SLOT: AtomicU32 = AtomicU32::new(0);
thread_local! { static MY_SLOT: usize = NEXT_SLOT.fetch_add(1, Ordering::Relaxed) as usize % SLOTS; }
/// Items that took long enough to list: (what, name, ms).
static SLOW: relax_session::sync::Mutex<Vec<(&'static str, String, f64)>> = relax_session::sync::Mutex::new(Vec::new());

/// The crate being loaded and when it started.
static LOADING: relax_session::sync::Mutex<(String, f64)> = relax_session::sync::Mutex::new((String::new(), 0.0));

/// --verbose: keep the slowest-item lists too.
pub fn track_for_verbose() {
    TRACK.store(true, Ordering::Relaxed);
    VERBOSE_LIST.store(true, Ordering::Relaxed);
}

/// The native run window uses the existing atomically published item slots.
pub fn track_for_host() { TRACK.store(true, Ordering::Relaxed); }

pub fn current_item() -> Option<String> {
    let loading = LOADING.lock().unwrap_or_else(|e| e.into_inner()).0.clone();
    if !loading.is_empty() { Some(loading) } else { longest_item().map(|(name, _)| name) }
}

/// A thread starts compiling `name` (jit.rs compile_watch). One relaxed load when nothing tracks.
pub fn item_begin(name: &str) {
    if !TRACK.load(Ordering::Relaxed) {
        return;
    }
    let s = &ITEMS[MY_SLOT.try_with(|s| *s).unwrap_or(0)];
    let q = s.seq.load(Ordering::Relaxed);
    s.seq.store(q | 1, Ordering::Release);
    let b = name.as_bytes();
    let n = b.len().min(WORDS * 8);
    for (i, w) in s.name.iter().enumerate() {
        let mut v = [0u8; 8];
        let lo = i * 8;
        if lo < n {
            let hi = (lo + 8).min(n);
            v[..hi - lo].copy_from_slice(&b[lo..hi]);
        }
        w.store(u64::from_le_bytes(v), Ordering::Relaxed);
    }
    s.len.store(n as u32, Ordering::Relaxed);
    s.start.store(crate::clock::ms_since_process_start() as u64 + 1, Ordering::Relaxed);
    s.seq.store((q | 1).wrapping_add(1), Ordering::Release);
    let _ = name;
}

/// The thread is done with its item.
pub fn item_end() {
    if !TRACK.load(Ordering::Relaxed) {
        return;
    }
    let s = &ITEMS[MY_SLOT.try_with(|s| *s).unwrap_or(0)];
    let start = s.start.swap(0, Ordering::Relaxed);
    if start == 0 || !VERBOSE_LIST.load(Ordering::Relaxed) {
        return;
    }
    let ms = crate::clock::ms_since_process_start() - (start - 1) as f64;
    if ms >= 20.0 {
        if let Some(name) = read_slot(s) {
            SLOW.lock().unwrap().push(("fn", name, ms));
        }
    }
}
static VERBOSE_LIST: AtomicBool = AtomicBool::new(false);

fn read_slot(s: &Slot) -> Option<String> {
    let q = s.seq.load(Ordering::Acquire);
    if q & 1 == 1 {
        return None;
    }
    let n = (s.len.load(Ordering::Relaxed) as usize).min(WORDS * 8);
    let mut bytes = Vec::with_capacity(WORDS * 8);
    for w in &s.name {
        bytes.extend_from_slice(&w.load(Ordering::Relaxed).to_le_bytes());
    }
    if s.seq.load(Ordering::Acquire) != q {
        return None;
    }
    bytes.truncate(n);
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The longest-running item (name, ms so far) of any thread.
fn longest_item() -> Option<(String, f64)> {
    let now = crate::clock::ms_since_process_start();
    let mut best: Option<(String, f64)> = None;
    for s in ITEMS.iter() {
        let start = s.start.load(Ordering::Relaxed);
        if start == 0 {
            continue;
        }
        let ms = now - (start - 1) as f64;
        if best.as_ref().map_or(true, |b| ms > b.1) {
            if let Some(n) = read_slot(s) {
                best = Some((n, ms));
            }
        }
    }
    best
}

/// A crate starts loading (run.rs); the one before it is done.
pub fn crate_begin(name: &str) {
    if !TRACK.load(Ordering::Relaxed) {
        return;
    }
    crate_end();
    // process state (the status line reads it in any phase)
    let name = crate::zone_alloc::kept_with(|| name.to_string());
    *LOADING.lock().unwrap() = (name, crate::clock::ms_since_process_start());
}

pub fn crate_end() {
    if !TRACK.load(Ordering::Relaxed) {
        return;
    }
    let mut g = LOADING.lock().unwrap();
    if !g.0.is_empty() {
        let ms = crate::clock::ms_since_process_start() - g.1;
        if VERBOSE_LIST.load(Ordering::Relaxed) {
            SLOW.lock().unwrap().push(("crate", std::mem::take(&mut g.0), ms));
        }
        // (the buffer goes: no capacity of an old generation stays behind)
        g.0 = String::new();
    }
}

/// --verbose, at the end: the five slowest crates (load) and functions (compile).
pub fn print_slowest() {
    if !VERBOSE_LIST.load(Ordering::Relaxed) {
        return;
    }
    crate_end();
    let mut v = SLOW.lock().unwrap();
    for (kind, title) in [("crate", "slowest crates (load)"), ("fn", "slowest functions (compile)")] {
        let mut l: Vec<_> = v.iter().filter(|x| x.0 == kind).collect();
        l.sort_by(|a, b| b.2.total_cmp(&a.2));
        if l.is_empty() {
            continue;
        }
        eprintln!("makepad-run: {title}:");
        for (_, n, ms) in l.into_iter().take(5) {
            eprintln!("makepad-run:   {ms:>9.1} ms  {n}");
        }
    }
    v.clear();
}

#[cfg(unix)]
const LABELS: [&str; 10] = [
    "booting", "unit graph", "build scripts", "loading", "resolving", "signatures", "linking", "image", "starting", "compiling",
];

/// A boot phase finished (run.rs `phase`): the one that follows is shown. A no-op without a bar.
pub fn phase_done(name: &str) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let next = match name.split(" (").next().unwrap_or(name) {
        "provided sources" => 1,
        "unit graph" => 2,
        "build scripts" => 3,
        "load+parse" => 4,
        "resolve" => 5,
        "signatures" => 6,
        "link libraries" => 7,
        "image" => 8,
        _ => return,
    };
    PHASE.store(next, Ordering::Relaxed);
}

/// `main` is about to be called: functions compile until the first frame.
pub fn compiling() {
    PHASE.store(9, Ordering::Relaxed);
}

/// The session log of this run (`makepad run: session log ...`), once `start` made one.
pub fn session_log_path() -> Option<std::path::PathBuf> {
    SESSION_LOG.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
static SESSION_LOG: relax_session::sync::Mutex<Option<std::path::PathBuf>> = relax_session::sync::Mutex::new(None);

#[cfg(not(unix))]
pub fn start(_progress: bool) {}
#[cfg(not(unix))]
pub fn finish(_summary: bool) {}
#[cfg(not(unix))]
pub(crate) fn flush() {}

#[cfg(unix)]
pub use unix::{finish, start};
#[cfg(unix)]
pub(crate) use unix::{flush, fault_fd};

#[cfg(unix)]
mod unix {
    use super::*;
    use std::io::IsTerminal;
    use std::os::unix::fs::OpenOptionsExt;
    use relax_session::sync::Mutex;

    extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(a: i32, b: i32) -> i32;
        fn close(fd: i32) -> i32;
        fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
        fn write(fd: i32, buf: *const u8, n: usize) -> isize;
        fn atexit(f: extern "C" fn()) -> i32;
        fn pipe(fds: *mut i32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn poll(fds: *mut PollFd, nfds: NfdsT, timeout: i32) -> i32;
    }
    #[repr(C)]
    struct PollFd { fd: i32, events: i16, revents: i16 }
    #[cfg(target_os = "linux")]
    type NfdsT = u64;
    #[cfg(not(target_os = "linux"))]
    type NfdsT = u32;
    const POLLIN: i16 = 1;
    const F_SETFD: i32 = 2;
    const FD_CLOEXEC: i32 = 1;
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    #[cfg(target_os = "linux")]
    const O_NONBLOCK: i32 = 0o4000;
    #[cfg(not(target_os = "linux"))]
    const O_NONBLOCK: i32 = 4;

    // The program's fd 2 is the write end of a pipe, not the transcript file: a full disk
    // (ENOSPC on a tmpfs TMPDIR, 235 on 10-23) must never fail the program's own stderr writes,
    // which panic in std's eprintln!. The tee thread copies the pipe into the transcript
    // (write errors drop that data) and into TEE for the display. The pipe only blocks a
    // writer while the tee thread is behind, as any piped stderr does.
    // Lock order: STATE, then READER, then TEE.
    struct Reader {
        /// read end of the pipe, O_NONBLOCK
        fd: i32,
        /// the transcript; None once the run is over
        file: Option<std::fs::File>,
    }
    static READER: Mutex<Option<Reader>> = Mutex::new(None);
    /// bytes read from the pipe that the display has not taken yet
    static TEE: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    /// The real stderr, kept for process lifetime so a signal never races its close.
    static OUT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

    /// Fault reports bypass the tee: a fatal signal kills its forwarding threads.
    pub(crate) fn fault_fd() -> i32 {
        let fd = OUT.load(Ordering::Acquire);
        if fd < 0 { 2 } else { fd }
    }

    struct State {
        /// the real stderr (a dup of it)
        out: i32,
        /// the pipe's write end as created (fd 2 is a dup of it)
        write_fd: i32,
        path: std::path::PathBuf,
        /// bytes of a line that has no newline yet
        partial: Vec<u8>,
        drawn: bool,
        last_draw_ms: f64,
    }
    static STATE: Mutex<Option<State>> = Mutex::new(None);
    static READY_LINE: AtomicBool = AtomicBool::new(false);

    /// Read everything the pipe holds now into the transcript and TEE. True at end of
    /// stream (every write end closed) or on a read error.
    fn drain(r: &mut Reader) -> bool {
        use std::io::Write;
        let mut buf = [0u8; 16384];
        loop {
            let n = unsafe { read(r.fd, buf.as_mut_ptr(), buf.len()) };
            if n > 0 {
                let chunk = &buf[..n as usize];
                if let Some(f) = r.file.as_mut() {
                    // a failed transcript write drops this chunk from the file only
                    let _ = f.write_all(chunk);
                }
                crate::zone_alloc::kept_with(|| TEE.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(chunk));
                continue;
            }
            if n == 0 {
                return true;
            }
            match std::io::Error::last_os_error().kind() {
                std::io::ErrorKind::Interrupted => continue,
                std::io::ErrorKind::WouldBlock => return false,
                _ => return true,
            }
        }
    }

    /// Drain under READER; closes the read end at end of stream.
    fn drain_now() {
        let mut g = READER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.as_mut() {
            if drain(r) {
                unsafe { close(r.fd) };
                *g = None;
            }
        }
    }

    fn tee() {
        loop {
            let fd = match READER.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                Some(r) => r.fd,
                None => return,
            };
            let mut p = PollFd { fd, events: POLLIN, revents: 0 };
            // poll outside the lock: drains elsewhere never wait for new output
            if unsafe { poll(&mut p, 1, -1) } < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return;
            }
            drain_now();
            kick_forward();
        }
    }

    /// The display worker (`forward`), woken by new output and the ready line: with no progress bar
    /// it sleeps until there is something to show (an idle run's log display does not poll).
    static FORWARD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();

    fn kick_forward() {
        if let Some(t) = FORWARD.get() {
            t.unpark();
        }
    }

    fn out_all(fd: i32, mut b: &[u8]) {
        while !b.is_empty() {
            let n = unsafe { write(fd, b.as_ptr(), b.len()) };
            if n <= 0 {
                return;
            }
            b = &b[n as usize..];
        }
    }

    fn style(code: &str, s: &str) -> String {
        format!("\x1b[{code}m{s}\x1b[0m")
    }

    fn secs(ms: f64) -> String {
        format!("{:.1} s", ms / 1000.0)
    }

    /// `s` shortened to `n` chars with an ellipsis in the middle.
    fn cut(s: &str, n: usize) -> String {
        let c: Vec<char> = s.chars().collect();
        if c.len() <= n {
            return s.to_string();
        }
        let head = n / 2;
        let tail = n - head - 1;
        format!("{}\u{2026}{}", c[..head].iter().collect::<String>(), c[c.len() - tail..].iter().collect::<String>())
    }

    fn bar() -> String {
        let ms = crate::clock::ms_since_process_start() as f64;
        const SPIN: [&str; 10] = ["\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}"];
        let spin = style("1;36", SPIN[(ms / 80.0) as usize % SPIN.len()]);
        let p = PHASE.load(Ordering::Relaxed) as usize;
        let mut label = LABELS[p.min(LABELS.len() - 1)];
        // after main: "compiling" only while something compiles (a queue, a startup compile, an
        // item in flight); a program that never presents a frame (a terminal program, a window
        // drawn by other means) otherwise read as compiling forever over an idle compiler (T1STUCK)
        let (queued, active, item) = if p == 9 {
            let (q, a) = crate::jit::compile_progress();
            (q, a, longest_item())
        } else {
            (None, 0, None)
        };
        if p == 9 && queued == Some(0) && active == 0 && item.is_none() {
            label = "running";
        }
        let mut text = style("1", &format!("{label:<9}"));
        match p {
            3 => {
                let (d, t) = (LOAD_DONE.load(Ordering::Relaxed), LOAD_TOTAL.load(Ordering::Relaxed));
                if t > 0 {
                    text.push_str(&format!(" {d}/{t} crates"));
                }
                let g = LOADING.lock().unwrap();
                if !g.0.is_empty() {
                    text.push_str(&format!(" {} {}", style("2", "\u{b7}"), cut(&g.0, 40)));
                }
            }
            9 => {
                let n = crate::jit::COMPILED_FNS.load(Ordering::Relaxed);
                if label == "running" {
                    text.push_str(&format!(" {n} fns compiled"));
                } else {
                    let queued = queued.map_or_else(|| "busy".to_string(), |n| n.to_string());
                    text.push_str(&format!(" {n} fns · {queued} queued · {active} active"));
                }
                if let Some((name, ms)) = item.filter(|x| x.1 > 500.0) {
                    text.push_str(&format!(" {} {} {}", style("2", "\u{b7}"), cut(&name, 40), style("2", &secs(ms))));
                }
            }
            _ => {}
        }
        let mut line = format!("{spin} {text} {} {}", style("2", "\u{b7}"), style("2", &secs(ms)));
        // never wider than a narrow terminal's line: the escape codes are cut with the text
        if line.len() > 400 {
            line.truncate(400);
        }
        line
    }

    fn erase(st: &mut State) {
        if st.drawn {
            out_all(st.out, b"\r\x1b[2K");
            st.drawn = false;
        }
    }

    fn draw(st: &mut State) {
        let b = bar();
        out_all(st.out, format!("\r\x1b[2K{b}").as_bytes());
        st.drawn = true;
        st.last_draw_ms = crate::clock::ms_since_process_start() as f64;
    }

    /// Drain a snapshot of the log. New writes never extend this pass indefinitely.
    /// A busy producer can leave bytes for the next tick; none are dropped or capped.
    fn pump(st: &mut State, flush_partial: bool) {
        {
            let mut tee = TEE.lock().unwrap_or_else(|e| e.into_inner());
            crate::zone_alloc::kept_with(|| st.partial.extend_from_slice(&tee));
            tee.clear();
        }
        let cut = if flush_partial { st.partial.len() } else { st.partial.iter().rposition(|&c| c == b'\n').map_or(0, |i| i + 1) };
        if cut > 0 {
            let mut chunk: Vec<u8> = st.partial.drain(..cut).collect();
            if flush_partial && chunk.last() != Some(&b'\n') { chunk.push(b'\n'); }
            erase(st);
            out_all(st.out, &chunk);
        }
    }

    extern "C" fn at_exit() {
        // run's exit worker handles the terminal inside its cleanup budget. Do
        // not take STATE again on the exiting thread after that budget expired.
        if !crate::preempt::is_stopped() { finish(false); }
    }

    /// Start a durable transcript, and optionally the terminal progress bar.
    pub fn start(progress: bool) {
        let _k = crate::zone_alloc::kept();
        unsafe { std::env::remove_var("MAKEPAD_RUN_PROGRESS") };
        if std::env::var_os("MAKEPAD_RUN_SCRIPT_DEPTH").is_some() || STATE.lock().unwrap().is_some() { return; }
        let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let progress = progress && std::io::stderr().is_terminal()
            && env("NO_COLOR").is_none() && env("MAKEPAD_RUN_REPORT_TO").is_none();
        let path = std::env::temp_dir().join(format!("makepad-run-session-{}-{}.log", std::process::id(), crate::clock::wall_ns()));
        let writer = match std::fs::OpenOptions::new().create_new(true).append(true).mode(0o600).open(&path) {
            Ok(f) => f,
            Err(e) => { eprintln!("makepad-run: cannot create session log: {e}"); return; }
        };
        let mut fds = [-1i32; 2];
        if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
            eprintln!("makepad-run: cannot create the session log pipe: {}", std::io::Error::last_os_error());
            return;
        }
        let [read_fd, write_fd] = fds;
        unsafe {
            // only fd 2 (a dup, without CLOEXEC) is inherited by child processes
            fcntl(read_fd, F_SETFD, FD_CLOEXEC);
            fcntl(write_fd, F_SETFD, FD_CLOEXEC);
            let fl = fcntl(read_fd, F_GETFL);
            fcntl(read_fd, F_SETFL, fl | O_NONBLOCK);
            // a deeper pipe (Linux: up to 1 MiB unprivileged) so a burst never waits on the tee
            #[cfg(target_os = "linux")]
            fcntl(write_fd, 1031 /* F_SETPIPE_SZ */, 1 << 20);
        }
        let mut out = OUT.load(Ordering::Acquire);
        if out < 0 {
            out = unsafe { dup(2) };
            if out >= 0 {
                unsafe { fcntl(out, F_SETFD, FD_CLOEXEC) };
                OUT.store(out, Ordering::Release);
            }
        }
        if out < 0 {
            unsafe { close(read_fd); close(write_fd); }
            return;
        }
        if unsafe { dup2(write_fd, 2) } < 0 {
            unsafe { close(write_fd); close(read_fd); }
            return;
        }
        *READER.lock().unwrap_or_else(|e| e.into_inner()) = Some(Reader { fd: read_fd, file: Some(writer) });
        *super::SESSION_LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(crate::zone_alloc::kept_with(|| path.clone()));
        // the path is named at the start with --verbose; a run that ends badly names it when it keeps the log
        if crate::run::VERBOSE.load(Ordering::Relaxed) {
            let note = format!("makepad-run: session log {}\n", path.display());
            out_all(write_fd, note.as_bytes());
        }
        let mut st = State { out, write_fd, path, partial: Vec::new(), drawn: false, last_draw_ms: 0.0 };
        PHASE.store(1, Ordering::Relaxed);
        if progress { TRACK.store(true, Ordering::Relaxed); }
        ACTIVE.store(progress, Ordering::Relaxed);
        pump(&mut st, false);
        if progress { draw(&mut st); }
        *STATE.lock().unwrap() = Some(st);
        if progress { unsafe { std::env::set_var("MAKEPAD_RUN_PROGRESS", "1") }; }
        let previous = std::panic::take_hook();
        // the hook lives as long as the process: the program's heap
        // the message reaches the transcript and the terminal before an abort can follow the hook
        { let _k = crate::zone_alloc::kept(); std::panic::set_hook(Box::new(move |info| { abnormal(); previous(info); panic_drain(); })); }
        unsafe { atexit(at_exit) };
        { let _k = crate::zone_alloc::kept();
        if let Err(e) = std::thread::Builder::new().name("run-log-tee".into()).spawn(tee) {
            unsafe { dup2(out, 2) };
            out_all(out, format!("makepad-run: cannot start the session log tee: {e}\n").as_bytes());
            abnormal();
            finish(false);
            return;
        }
        if let Err(e) = std::thread::Builder::new().name("run-progress".into()).spawn(forward) {
            eprintln!("makepad-run: cannot forward session log: {e}");
            abnormal();
            finish(false);
        }
        }
    }

    fn forward() {
        let _ = FORWARD.set(std::thread::current());
        let mut first = true;
        loop {
            // the bar redraws every 50 ms; without it, output wakes this thread (tee, finish).
            // (The first round looks without a wake: output from before FORWARD was set.)
            if !first && !ACTIVE.load(Ordering::Relaxed) {
                std::thread::park();
            }
            first = false;
            // (a burst of output is shown together, as before)
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut g = STATE.lock().unwrap();
            let Some(st) = g.as_mut() else { return };
            pump(st, false);
            if READY_LINE.swap(false, Ordering::AcqRel) {
                let active = ACTIVE.swap(false, Ordering::Relaxed);
                erase(st);
                if active { ready_line(st); }
            }
            if ACTIVE.load(Ordering::Relaxed) && crate::clock::ms_since_process_start() - st.last_draw_ms >= 50.0 {
                draw(st);
            }
        }
    }

    /// A watcher acknowledgement must not lag behind the program's new stdout.
    /// Called only off the UI thread and after the reload releases compiler locks.
    pub(crate) fn flush() {
        let mut g = STATE.lock().unwrap();
        if let Some(st) = g.as_mut() {
            drain_now();
            pump(st, true);
        }
    }

    /// After the panic hook printed: everything in the pipe to the transcript, and to the
    /// terminal (through the display when it is free, else raw).
    fn panic_drain() {
        // try_lock: a panic on the tee thread itself (inside its drain) must not wait for itself;
        // that drain is then the one that copies this message
        if let Ok(mut g) = READER.try_lock() {
            if let Some(r) = g.as_mut() {
                if drain(r) {
                    unsafe { close(r.fd) };
                    *g = None;
                }
            }
        }
        match STATE.try_lock() {
            Ok(mut g) => {
                if let Some(st) = g.as_mut() { pump(st, true); }
            }
            Err(_) => {
                let out = OUT.load(Ordering::Acquire);
                if out >= 0 {
                    let bytes = std::mem::take(&mut *TEE.lock().unwrap_or_else(|e| e.into_inner()));
                    out_all(out, &bytes);
                }
            }
        }
    }

    /// Takes the bar down and gives fd 2 back its terminal. `summary`: first present, print the
    /// ready line. Idempotent.
    pub fn finish(summary: bool) {
        // The native first-present callback runs on the UI thread. It must never
        // wait for log I/O or a terminal reader; the display worker owns both.
        if summary {
            READY_LINE.store(true, Ordering::Release);
            kick_forward();
            return;
        }
        let mut g = STATE.lock().unwrap();
        let Some(st) = g.as_mut() else { return };
        ACTIVE.store(false, Ordering::Relaxed);
        drain_now();
        pump(st, true);
        erase(st);
        // fd 2 back to the terminal and our write end closed: the pipe ends unless a child
        // process still holds it (the tee keeps copying that child's output to the transcript)
        unsafe { dup2(st.out, 2); close(st.write_fd); }
        drain_now();
        pump(st, true);
        let clean = EXIT_CODE.load(Ordering::Acquire) == 0 && !ABNORMAL.load(Ordering::Acquire);
        if clean {
            if let Err(e) = std::fs::remove_file(&st.path) {
                out_all(st.out, format!("makepad-run: session log kept at {}: {e}\n", st.path.display()).as_bytes());
            }
        } else {
            out_all(st.out, format!("makepad-run: session log kept at {}\n", st.path.display()).as_bytes());
        }
        *g = None;
    }

    fn ready_line(st: &State) {
        let pkg = std::env::var("MAKEPAD_RUN_PACKAGE").unwrap_or_else(|_| "program".to_string());
        let ahead = crate::jit::ahead_level().name();
        let cache = match std::env::var("MAKEPAD_RUN_CACHE_STATE").as_deref() {
            Ok("warm") => format!(" (warm, ahead {ahead})"),
            Ok("cold") => format!(" (cold, ahead {ahead})"),
            _ => format!(" (ahead {ahead})"),
        };
        let ms = crate::clock::ms_since_process_start() as f64;
        let line = format!("{} {} ready in {}{}\n", style("1;32", "\u{2713}"), style("1;36", &pkg), secs(ms), style("2", &cache));
        out_all(st.out, line.as_bytes());
    }
}
