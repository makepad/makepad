//! A build script's `$RUSTC` questions, answered by the process that runs the script: Relax std's
//! spawn asks `relax_tool_spawn` first (process_unix.rs / process_windows.rs `tool_spawn`), and no
//! loader starts per `rustc --version`. Its answer is `rustc_answer`, the same a loader started
//! as RUSTC gives (relax_as_rustc).
//!
//! The child is a process to the script in every way it can see: std makes the stdio pipes it
//! would give a process, and the answer runs on a run thread of its own with the child's ends.
//! It reads the program the script pipes in (an argument `-`) to its end, writes the answer and
//! closes its ends, so a script that writes to its child's stdin and reads the answer behaves as
//! with a process. The script waits for it as for a process (`relax_tool_wait`).
//!
//! Every spawn of Relax std asks first, before it builds anything that starts a process. Only
//! plain spawns (no exec, `pre_exec` closures, user or group) with an option-led question of the
//! script's own `$RUSTC` are answered here; `$CARGO metadata`, the script's run and any other
//! program start as processes, and so does every spawn outside a build script's run.

use std::sync::{Condvar, Mutex};

/// handle -> the answer's exit status once it is written (None while it runs); `released`: the
/// script's Child went first (the handle goes when the answer ends)
struct Tool {
    status: Option<i32>,
    released: bool,
}

static TOOLS: Mutex<Vec<(u64, Tool)>> = Mutex::new(Vec::new());
static DONE: Condvar = Condvar::new();
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The arguments of `argv` (the program first) when this process runs a build script
/// (RELAX_AS_RUSTC, which the runner sets as the script's run starts) and `argv` asks the
/// script's own RUSTC an option-led question; None: a process answers it.
fn rustc_question(argv: &[Vec<u8>]) -> Option<Vec<String>> {
    std::env::var_os("RELAX_AS_RUSTC")?;
    let rustc = std::env::var_os("RUSTC")?;
    let (program, args) = argv.split_first()?;
    let program = std::path::Path::new(unsafe { std::ffi::OsStr::from_encoded_bytes_unchecked(program) });
    let same = program.as_os_str() == rustc.as_os_str()
        || matches!((program.canonicalize(), std::path::Path::new(&rustc).canonicalize()), (Ok(a), Ok(b)) if a == b);
    if !same {
        return None;
    }
    option_led(args)
}

/// A confined script's question (sandbox/script.rs): the program is the `rustc` its parent gave
/// it, byte for byte (no path of the script's is resolved), and the question is option-led.
#[cfg(unix)]
pub(crate) fn confined_question(argv: &[Vec<u8>], rustc: &[u8]) -> Option<Vec<String>> {
    let (program, args) = argv.split_first()?;
    if program.as_slice() != rustc {
        return None;
    }
    option_led(args)
}

fn option_led(args: &[Vec<u8>]) -> Option<Vec<String>> {
    if !args.first().is_some_and(|a| a.first() == Some(&b'-')) {
        return None;
    }
    Some(args.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect())
}

/// The words of std's encoded command (`<length>:<bytes>` each); None: malformed.
pub(crate) fn words(mut s: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut words = Vec::new();
    while let Some(colon) = s.iter().position(|&b| b == b':') {
        let n = std::str::from_utf8(&s[..colon]).ok()?.parse::<usize>().ok()?;
        let w = s.get(colon + 1..colon.checked_add(1 + n)?)?;
        words.push(w.to_vec());
        s = &s[colon + 1 + n..];
    }
    Some(words)
}

/// One end of the child's stdio, as this process's own file: `raw` the end std made for the child
/// (a descriptor or handle, which std closes after the call), or -1 for this process's own
/// stream `slot` (0 stdin, 1 stdout, 2 stderr), as a child that inherits it would have it.
fn adopt(raw: i64, slot: u8) -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        let fd = if raw < 0 { slot as i32 } else { raw as i32 };
        let owned = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }.try_clone_to_owned().ok()?;
        Some(std::fs::File::from(owned))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsHandle;
        let owned = if raw < 0 {
            match slot {
                0 => std::io::stdin().as_handle().try_clone_to_owned(),
                1 => std::io::stdout().as_handle().try_clone_to_owned(),
                _ => std::io::stderr().as_handle().try_clone_to_owned(),
            }
        } else {
            unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(raw as isize as std::os::windows::io::RawHandle) }.try_clone_to_owned()
        };
        Some(std::fs::File::from(owned.ok()?))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (raw, slot);
        None
    }
}

/// `relax_tool_spawn`'s answer for a spawn that is refused: std returns NotFound, as for a program
/// that is not there, and starts nothing (process_unix.rs / process_windows.rs TOOL_REFUSED).
pub const TOOL_REFUSED: u64 = u64::MAX;

/// Relax std's spawn: `argv` the command (the program, then its arguments, each as
/// `<length>:<bytes>`), `stdin` / `stdout` / `stderr` the ends std made for the child (-1: this
/// process's own), `plain` 0 for an exec or a spawn with `pre_exec` closures, a user or a group. A
/// handle when the answer runs here (std's Child waits for it), 0 when a process answers (std
/// spawns it). This host refuses none (`TOOL_REFUSED`).
pub extern "C" fn relax_tool_spawn(argv: *const u8, len: usize, stdin: i64, stdout: i64, stderr: i64, plain: i32) -> u64 {
    if plain == 0 {
        return 0;
    }
    let s = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(argv, len) } };
    let Some(args) = words(s).and_then(|w| rustc_question(&w)) else { return 0 };
    let (Some(input), Some(out), Some(err)) = (adopt(stdin, 0), adopt(stdout, 1), adopt(stderr, 2)) else { return 0 };
    start(args, input, out, err)
}

/// Starts the answer to `args` on a run thread of its own, with the child's ends: its handle, 0
/// when it could not start.
pub(crate) fn start(args: Vec<String>, input: std::fs::File, out: std::fs::File, err: std::fs::File) -> u64 {
    let h = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    TOOLS.lock().unwrap_or_else(|e| e.into_inner()).push((h, Tool { status: None, released: false }));
    let started = crate::run_threads::spawn("run-tool", move || {
        use std::io::{Read, Write};
        let (mut input, mut out, mut err) = (input, out, err);
        // the program a probe pipes in, read to its end as rustc reads it; any other question
        // leaves stdin as rustc leaves it, closed with the answer
        let mut program = Vec::new();
        if args.iter().any(|a| a == "-") {
            let _ = input.read_to_end(&mut program);
        }
        drop(input);
        let (status, o, e) = crate::rustc_answer(&args, &program);
        let _ = out.write_all(&o);
        let _ = err.write_all(&e);
        drop((out, err));
        let mut t = TOOLS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = t.iter().position(|(k, _)| *k == h) {
            if t[i].1.released {
                t.swap_remove(i);
            } else {
                t[i].1.status = Some(status);
            }
        }
        DONE.notify_all();
    });
    if started.is_err() {
        TOOLS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(k, _)| *k != h);
        return 0;
    }
    h
}

/// The exit status of the answer `h` (`relax_tool_spawn`): `block` waits for it, as a wait for a
/// process does; -1 while it runs (`block` 0) or for no such handle.
pub extern "C" fn relax_tool_wait(h: u64, block: i32) -> i64 {
    let mut t = TOOLS.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        match t.iter().find(|(k, _)| *k == h) {
            None => return -1,
            Some((_, Tool { status: Some(s), .. })) => return *s as i64,
            Some(_) if block == 0 => return -1,
            Some(_) => t = DONE.wait(t).unwrap_or_else(|e| e.into_inner()),
        }
    }
}

/// The script's Child of answer `h` is gone: its handle goes now, or when the answer ends.
pub extern "C" fn relax_tool_release(h: u64) {
    let mut t = TOOLS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = t.iter().position(|(k, _)| *k == h) {
        if t[i].1.status.is_some() {
            t.swap_remove(i);
        } else {
            t[i].1.released = true;
        }
    }
}

#[cfg(test)]
mod tests {
    /// A build script's question carries rustc's words only: Relax's own words among them
    /// (`--no-asm`, the grants of options.rs GRANTS) change nothing of the run that answers, and
    /// one that is no grant does not end it (a parsed `--allow-nonsense` exits 2).
    #[test]
    fn a_rustc_question_never_sets_the_runs_flags() {
        isolated_test!(a_rustc_question_never_sets_the_runs_flags);
        for name in ["RELAX_NO_ASM", "RELAX_ALLOW_C"] {
            assert_eq!(std::env::var_os(name), None, "{name} is set in the test's own environment");
        }
        let args: Vec<String> = ["--print", "cfg", "--no-asm", "--allow-c", "--allow-host-modules", "--allow-module-fuel=1", "--allow-nonsense"].iter().map(|s| s.to_string()).collect();
        let _ = crate::rustc_answer(&args, b"");
        assert_eq!(std::env::var_os("RELAX_NO_ASM"), None, "the question's --no-asm set RELAX_NO_ASM");
        assert_eq!(std::env::var_os("RELAX_ALLOW_C"), None, "the question's --allow-c set RELAX_ALLOW_C");
    }
}
