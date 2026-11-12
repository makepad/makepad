//! The program's environment under the JIT: the reserved `MAKEPAD_RUN_*` names exist twice in a
//! JIT process, once for the host and once for the program it runs.
//!
//! A JIT process is a host and a program with one process environment. The programs where that
//! matters are Relax itself (relax-compiler and relax-base tests): the same source reads the same
//! names as host (rustc-built) and as program (JIT-run). Under rustc the variables a program sets,
//! or gives a child, are that program's alone; under the JIT the host read them too (a child's
//! `MAKEPAD_RUN_BOOT_TRACE` printed the child host's boot trace into the program's stderr, a child's
//! `MAKEPAD_RUN_CACHE_DIR` moved the child host's cache into the program's scratch folder).
//!
//! Who reads what:
//! - A launch value (whoever started this process set it) is read by host and program, as under
//!   rustc both the launched binary and its code see it.
//! - A host setting (the host's `set_var`) is the host's; a program reads it by fall-through.
//! - A program value (JIT-run code's set_var / remove_var / setenv, and a Command's env edits) is
//!   the program's alone: it lives in the program's overlay, never in this process's environment,
//!   and crosses a spawn as the child program's value.
//!
//! The policy is the pure function `child_env`: what each child gets. A self-exec child (this
//! program again, `makepad run ... --`) gets every reserved name from this process's environment
//! (the host's) and the program's differing values in `MAKEPAD_RUN_PROGRAM_ENV` (`encode`), which
//! the child host takes out of its environment into the child program's overlay at its start. Any
//! other child gets the program's view as plain variables, without the run facts (`RUN_FACTS`: what
//! the host says of this run) the program did not write; of those only the mount crosses, into a
//! child in this process's folder or in the mount.
//!
//! JIT-run code reaches the overlay through the loader's symbols (relax-compiler jit.rs
//! `loader_symbol`): its `getenv` / `setenv` / `unsetenv` / `putenv` are `program_getenv` and the
//! rest, so Relax std's `var_os` / `set_var` / `remove_var` and the raw reads of an allocator alike
//! see the program's values; std's `vars_os` and a spawn's environment take `env_snapshot`, and
//! std's own plumbing (the self-exec command line, the virtual folder) reads the host's values
//! through `host_getenv`. Known limit: code that walks the `environ` array itself, or a C library
//! in the process calling its own `getenv`, sees the host's values of reserved names.
//! Windows' `Get/SetEnvironmentVariableW/A` and wide environment blocks use the same overlay;
//! names compare without case, and std's plumbing reads `host_get_environment_variable_w`.

/// The reserved namespace: names read by Relax as host and as program.
pub const PREFIX: &[u8] = b"MAKEPAD_RUN_";
/// A self-exec child's program values (`encode`), for the child host to take into its overlay.
pub const PROGRAM_ENV: &[u8] = b"MAKEPAD_RUN_PROGRAM_ENV";
/// The mount of a virtual folder, one unit: the folder, the mount table and the edits folder over
/// it (vfs.rs).
pub const WORKSPACE: &[u8] = b"MAKEPAD_RUN_WORKSPACE";
pub const VFS: &[u8] = b"MAKEPAD_RUN_VFS";
pub const EDITS: &[u8] = b"MAKEPAD_RUN_EDITS";
pub const MOUNT: [&[u8]; 3] = [WORKSPACE, VFS, EDITS];
/// Run facts: what the host says of this run, for this run again (a self-exec child) and nothing
/// else; the mount also for a child in this process's folder or in the mount. The self-exec
/// command line (Relax std's spawn of this program) and whether the run added its `--silent`.
pub const RUN_FACTS: [&[u8]; 5] = [b"MAKEPAD_RUN_SELF_EXEC", b"MAKEPAD_RUN_SELF_EXEC_SILENT_ADDED", WORKSPACE, VFS, EDITS];

pub fn run_fact(name: &[u8]) -> bool {
    RUN_FACTS.iter().any(|n| same_name(n, name))
}

/// The host states a run fact (`RUN_FACTS`).
pub fn set_run_fact(name: &str, value: impl AsRef<std::ffi::OsStr>) {
    assert!(run_fact(name.as_bytes()), "{name} is no run fact (progenv RUN_FACTS)");
    std::env::set_var(name, value);
}

/// A host's own spawn of a child that is not this run (a build script, cargo, a check of a
/// package): no run fact crosses; the caller sets what the child's folder needs.
pub fn scrub_run_facts(cmd: &mut std::process::Command) {
    for n in RUN_FACTS {
        cmd.env_remove(std::str::from_utf8(n).unwrap());
    }
}

pub fn reserved(name: &[u8]) -> bool {
    name.get(..PREFIX.len()).is_some_and(|n| same_name(n, PREFIX))
}

fn same_name(a: &[u8], b: &[u8]) -> bool {
    #[cfg(not(windows))]
    { a == b }
    #[cfg(windows)]
    {
        if a.is_ascii() && b.is_ascii() {
            return a.eq_ignore_ascii_case(b);
        }
        let (a, b) = (wide(a), wide(b));
        a.len() == b.len() && unsafe { win::compare_string_ordinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1) } == 2
    }
}

/// An environment as name / value byte pairs.
pub type Env = Vec<(Vec<u8>, Vec<u8>)>;
/// The program's own values of reserved names: Some(value) set, None removed.
pub type Own = Vec<(Vec<u8>, Option<Vec<u8>>)>;

/// Where a child runs.
#[derive(Clone, Copy, Debug)]
pub enum ChildDir<'a> {
    /// in this process's folder (no `current_dir`)
    Same,
    /// in a virtual folder (a git run's): it starts in this process's real folder and its host serves
    /// `workspace` from the mount table `vfs`
    Virtual { workspace: &'a [u8], vfs: &'a [u8] },
    /// in a real folder
    Real,
}

/// One spawn as the program asks for it.
#[derive(Clone, Copy, Debug)]
pub struct Spawn<'a> {
    /// the Command's env edits in order: Some(value) set, None removed
    pub edits: &'a [(Vec<u8>, Option<Vec<u8>>)],
    /// `env_clear`
    pub clear: bool,
    /// the child is this program again (its host takes `PROGRAM_ENV`)
    pub self_exec: bool,
    pub dir: ChildDir<'a>,
    /// what Relax std's plumbing gives the child's host (an exec's clock origin), set last
    pub host: &'a [(Vec<u8>, Vec<u8>)],
}

fn set(env: &mut Env, name: &[u8], value: Option<&[u8]>) {
    match (env.iter().position(|(k, _)| same_name(k, name)), value) {
        (Some(i), Some(v)) => env[i].1 = v.to_vec(),
        (Some(i), None) => {
            env.remove(i);
        }
        (None, Some(v)) => env.push((name.to_vec(), v.to_vec())),
        (None, None) => {}
    }
}

/// What the program reads: `real` (this process's environment) with its own values applied.
pub fn program_view(real: &[(Vec<u8>, Vec<u8>)], own: &[(Vec<u8>, Option<Vec<u8>>)]) -> Env {
    let mut env: Env = real.iter().filter(|(k, _)| !same_name(k, PROGRAM_ENV)).cloned().collect();
    for (k, v) in own {
        set(&mut env, k, v.as_deref());
    }
    env
}

/// The environment a child starts with: `real` is this process's environment, `own` the program's
/// values. A self-exec child: every name outside the namespace from the program's view, every
/// reserved name from `real` (the host's), and `PROGRAM_ENV` holding the reserved names where the
/// program's view differs. Any other child: the program's view without the run facts the program
/// did not write. The mount follows the child's folder: this process's keeps it, a virtual one
/// names that folder, a real one names none (a self-exec child keeps the mount table its
/// `--workspace-dir` reads).
pub fn child_env(real: &[(Vec<u8>, Vec<u8>)], own: &[(Vec<u8>, Option<Vec<u8>>)], spawn: &Spawn) -> Env {
    let mut view = if spawn.clear { Env::new() } else { program_view(real, own) };
    for (k, v) in spawn.edits {
        set(&mut view, k, v.as_deref());
    }
    let wrote = |k: &[u8]| own.iter().any(|(o, _)| same_name(o, k)) || spawn.edits.iter().any(|(e, _)| same_name(e, k));
    let host_value = |k: &[u8]| real.iter().find(|(r, _)| same_name(r, k)).map(|(_, v)| v.clone());
    let mut env = if spawn.self_exec {
        let host: Env = real.iter().filter(|(k, _)| reserved(k) && !same_name(k, PROGRAM_ENV)).cloned().collect();
        let mut differ: Own = Vec::new();
        for (k, v) in &view {
            if reserved(k) && !host.iter().any(|(h, hv)| same_name(h, k) && hv == v) {
                differ.push((k.clone(), Some(v.clone())));
            }
        }
        for (h, _) in &host {
            if !view.iter().any(|(k, _)| same_name(k, h)) {
                differ.push((h.clone(), None));
            }
        }
        let mut env: Env = view.into_iter().filter(|(k, _)| !reserved(k)).collect();
        env.extend(host);
        if !differ.is_empty() {
            env.push((PROGRAM_ENV.to_vec(), encode(&differ)));
        }
        env
    } else {
        let mut env = view;
        env.retain(|(k, _)| !run_fact(k) || wrote(k.as_slice()));
        if matches!(spawn.dir, ChildDir::Same) && !spawn.clear {
            for n in MOUNT {
                if let Some(v) = host_value(n).filter(|_| !wrote(n)) {
                    set(&mut env, n, Some(v.as_slice()));
                }
            }
        }
        env
    };
    match spawn.dir {
        ChildDir::Same => {}
        ChildDir::Virtual { workspace, vfs } => {
            set(&mut env, WORKSPACE, Some(workspace));
            set(&mut env, VFS, Some(vfs));
            if !spawn.self_exec && !wrote(EDITS) {
                if let Some(v) = host_value(EDITS) {
                    set(&mut env, EDITS, Some(v.as_slice()));
                }
            }
        }
        ChildDir::Real if spawn.self_exec => set(&mut env, WORKSPACE, None),
        ChildDir::Real => {}
    }
    for (k, v) in spawn.host {
        set(&mut env, k, Some(v));
    }
    env
}

/// The wire form of program values: per entry its byte length in decimal, `:`, then `NAME=value`
/// (set) or `NAME` (removed). Lengths, not a separator: any byte can be in a value
/// (MAKEPAD_RUN_SELF_EXEC itself holds `\x1f`).
pub fn encode(own: &[(Vec<u8>, Option<Vec<u8>>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in own {
        let n = k.len() + v.as_ref().map_or(0, |v| v.len() + 1);
        out.extend_from_slice(n.to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(k);
        if let Some(v) = v {
            out.push(b'=');
            out.extend_from_slice(v);
        }
    }
    out
}

/// `encode`'s inverse; Err names where the text stops being the wire form.
pub fn parse(mut s: &[u8]) -> Result<Own, String> {
    let mut out = Own::new();
    while !s.is_empty() {
        let colon = s.iter().position(|&b| b == b':').ok_or_else(|| format!("no length before {:?}", String::from_utf8_lossy(s)))?;
        let n: usize = std::str::from_utf8(&s[..colon]).ok().and_then(|d| d.parse().ok())
            .ok_or_else(|| format!("bad length {:?}", String::from_utf8_lossy(&s[..colon])))?;
        let e = s.get(colon + 1..colon + 1 + n).ok_or_else(|| format!("entry of {n} bytes past the end"))?;
        // Windows keeps drive-current-directory names such as `=C:`.
        match e.iter().enumerate().find(|&(i, b)| *b == b'=' && (!cfg!(windows) || i != 0)).map(|(i, _)| i) {
            Some(eq) => out.push((e[..eq].to_vec(), Some(e[eq + 1..].to_vec()))),
            None => out.push((e.to_vec(), None)),
        }
        s = &s[colon + 1 + n..];
    }
    Ok(out)
}

// ---- the program's overlay: JIT-run code's writes of reserved names

/// One write of the program's, newest first. Never freed: a value pointer `getenv` gave out stays
/// valid after the program writes the name again (libc's setenv keeps old values too).
struct Entry {
    name: Box<[u8]>,
    value: Option<std::ffi::CString>,
    next: *const Entry,
}

static WRITES: std::sync::atomic::AtomicPtr<Entry> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

fn write(name: &[u8], value: Option<&[u8]>) {
    use std::sync::atomic::Ordering;
    // (a C string from the caller: no NUL inside)
    let value = value.map(|v| std::ffi::CString::new(v).unwrap());
    let node = Box::into_raw(Box::new(Entry { name: name.into(), value, next: std::ptr::null() }));
    let mut head = WRITES.load(Ordering::Acquire);
    loop {
        unsafe { (*node).next = head };
        match WRITES.compare_exchange_weak(head, node, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(h) => head = h,
        }
    }
}

/// The program's value of `name`: Some(Some) set, Some(None) removed, None: not one of its own.
/// No lock or ASCII-name allocation: the JIT-run allocator reads its flags here at its start.
#[cfg(any(unix, windows))]
fn lookup(name: &[u8]) -> Option<Option<&'static std::ffi::CStr>> {
    let mut p = WRITES.load(std::sync::atomic::Ordering::Acquire) as *const Entry;
    while let Some(w) = unsafe { p.as_ref() } {
        if same_name(&w.name, name) {
            return Some(w.value.as_deref());
        }
        p = w.next;
    }
    None
}

/// The program's own values, each name once (its latest write), oldest name first.
pub fn own() -> Own {
    let mut out = Own::new();
    let mut p = WRITES.load(std::sync::atomic::Ordering::Acquire) as *const Entry;
    while let Some(w) = unsafe { p.as_ref() } {
        if !out.iter().any(|(k, _)| same_name(k, &w.name)) {
            out.push((w.name.to_vec(), w.value.as_ref().map(|v| v.as_bytes().to_vec())));
        }
        p = w.next;
    }
    out.reverse();
    out
}

/// This process's environment as it is (the host's settings).
pub fn real() -> Env {
    std::env::vars_os().map(|(k, v)| (k.into_encoded_bytes(), v.into_encoded_bytes())).collect()
}

#[cfg(unix)]
mod libc {
    use std::ffi::{c_char, c_int};
    extern "C" {
        pub fn getenv(name: *const c_char) -> *mut c_char;
        pub fn setenv(name: *const c_char, value: *const c_char, overwrite: c_int) -> c_int;
        pub fn unsetenv(name: *const c_char) -> c_int;
        pub fn putenv(string: *mut c_char) -> c_int;
    }
}

/// `getenv` of JIT-run code.
#[cfg(unix)]
pub extern "C" fn program_getenv(name: *const std::ffi::c_char) -> *const std::ffi::c_char {
    let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
    if reserved(n) {
        if let Some(v) = lookup(n) {
            return v.map_or(std::ptr::null(), std::ffi::CStr::as_ptr);
        }
    }
    unsafe { libc::getenv(name) }
}

/// `setenv` of JIT-run code: a reserved name is the program's own.
#[cfg(unix)]
pub extern "C" fn program_setenv(name: *const std::ffi::c_char, value: *const std::ffi::c_char, overwrite: std::ffi::c_int) -> std::ffi::c_int {
    let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
    // a name with `=` is libc's EINVAL: it answers that itself
    if !reserved(n) || n.contains(&b'=') {
        return unsafe { libc::setenv(name, value, overwrite) };
    }
    if overwrite == 0 && !program_getenv(name).is_null() {
        return 0;
    }
    write(n, Some(unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes()));
    0
}

/// `unsetenv` of JIT-run code.
#[cfg(unix)]
pub extern "C" fn program_unsetenv(name: *const std::ffi::c_char) -> std::ffi::c_int {
    let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
    if !reserved(n) || n.contains(&b'=') {
        return unsafe { libc::unsetenv(name) };
    }
    write(n, None);
    0
}

/// `putenv` of JIT-run code (`NAME=value`; glibc's `NAME` alone removes it).
#[cfg(unix)]
pub extern "C" fn program_putenv(string: *mut std::ffi::c_char) -> std::ffi::c_int {
    let s = unsafe { std::ffi::CStr::from_ptr(string) }.to_bytes();
    let (n, v) = match s.iter().position(|&b| b == b'=') {
        Some(eq) => (&s[..eq], Some(&s[eq + 1..])),
        None => (s, None),
    };
    if !reserved(n) || (v.is_none() && !cfg!(target_os = "linux")) {
        return unsafe { libc::putenv(string) };
    }
    write(n, v);
    0
}

/// Relax std's plumbing reads (the self-exec command line, the virtual folder): the host's value.
#[cfg(unix)]
pub extern "C" fn host_getenv(name: *const std::ffi::c_char) -> *const std::ffi::c_char {
    unsafe { libc::getenv(name) }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_char;
    #[link(name = "kernel32")]
    extern "system" {
        #[link_name = "GetEnvironmentVariableW"]
        pub fn get_environment_variable_w(name: *const u16, buf: *mut u16, size: u32) -> u32;
        #[link_name = "GetEnvironmentVariableA"]
        pub fn get_environment_variable_a(name: *const c_char, buf: *mut c_char, size: u32) -> u32;
        #[link_name = "SetEnvironmentVariableW"]
        pub fn set_environment_variable_w(name: *const u16, value: *const u16) -> i32;
        #[link_name = "SetEnvironmentVariableA"]
        pub fn set_environment_variable_a(name: *const c_char, value: *const c_char) -> i32;
        #[link_name = "FreeEnvironmentStringsW"]
        pub fn free_environment_strings_w(env: *mut u16) -> i32;
        #[link_name = "SetLastError"]
        pub fn set_last_error(error: u32);
        #[link_name = "CompareStringOrdinal"]
        pub fn compare_string_ordinal(a: *const u16, a_len: i32, b: *const u16, b_len: i32, ignore_case: i32) -> i32;
    }

    // Blocks may be freed by a different thread; foreign blocks belong to kernel32.
    pub static BLOCKS: std::sync::Mutex<Vec<Box<[u16]>>> = std::sync::Mutex::new(Vec::new());
}

#[cfg(windows)]
unsafe fn wide_bytes(p: *const u16) -> Vec<u8> {
    use std::os::windows::ffi::OsStringExt;
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    std::ffi::OsString::from_wide(unsafe { std::slice::from_raw_parts(p, n) }).into_encoded_bytes()
}

// WTF-8 preserves lone surrogates; the A API can also store individual non-UTF-8 bytes.
#[cfg(windows)]
fn wide(mut bytes: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        match std::str::from_utf8(bytes) {
            Ok(s) => {
                out.extend(s.encode_utf16());
                break;
            }
            Err(e) => {
                let n = e.valid_up_to();
                out.extend(std::str::from_utf8(&bytes[..n]).unwrap().encode_utf16());
                bytes = &bytes[n..];
                if bytes.len() >= 3 && bytes[0] == 0xed && bytes[1] & 0xe0 == 0xa0 && bytes[2] & 0xc0 == 0x80 {
                    out.push(((bytes[0] as u16 & 0x0f) << 12) | ((bytes[1] as u16 & 0x3f) << 6) | (bytes[2] as u16 & 0x3f));
                    bytes = &bytes[3..];
                } else {
                    out.push(bytes[0] as u16);
                    bytes = &bytes[1..];
                }
            }
        }
    }
    out
}

#[cfg(windows)]
fn copy_value<T: Copy + Default>(value: Option<&[T]>, buf: *mut T, size: u32) -> u32 {
    let Some(value) = value else {
        unsafe { win::set_last_error(203) }; // ERROR_ENVVAR_NOT_FOUND
        return 0;
    };
    if size as usize <= value.len() {
        return (value.len() + 1) as u32;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
        *buf.add(value.len()) = T::default();
        win::set_last_error(0); // an empty value is a success too
    }
    value.len() as u32
}

/// `GetEnvironmentVariableW` of JIT-run code.
#[cfg(windows)]
pub extern "system" fn program_get_environment_variable_w(name: *const u16, buf: *mut u16, size: u32) -> u32 {
    if !name.is_null() {
        let n = unsafe { wide_bytes(name) };
        if reserved(&n) {
            if let Some(v) = lookup(&n) {
                let value = v.map(|v| wide(v.to_bytes()));
                return copy_value(value.as_deref(), buf, size);
            }
        }
    }
    unsafe { win::get_environment_variable_w(name, buf, size) }
}

/// `GetEnvironmentVariableA` reads the same overlay, as stored bytes.
#[cfg(windows)]
pub extern "system" fn program_get_environment_variable_a(name: *const std::ffi::c_char, buf: *mut std::ffi::c_char, size: u32) -> u32 {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        if reserved(n) {
            if let Some(v) = lookup(n) {
                return copy_value(v.map(std::ffi::CStr::to_bytes), buf as *mut u8, size);
            }
        }
    }
    unsafe { win::get_environment_variable_a(name, buf, size) }
}

/// `SetEnvironmentVariableW`: a reserved name is the program's own.
#[cfg(windows)]
pub extern "system" fn program_set_environment_variable_w(name: *const u16, value: *const u16) -> i32 {
    if !name.is_null() {
        let n = unsafe { wide_bytes(name) };
        if reserved(&n) && !n.contains(&b'=') {
            let v = if value.is_null() { None } else { Some(unsafe { wide_bytes(value) }) };
            write(&n, v.as_deref());
            return 1;
        }
    }
    unsafe { win::set_environment_variable_w(name, value) }
}

/// `SetEnvironmentVariableA`: a null value removes the program's value.
#[cfg(windows)]
pub extern "system" fn program_set_environment_variable_a(name: *const std::ffi::c_char, value: *const std::ffi::c_char) -> i32 {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        if reserved(n) && !n.contains(&b'=') {
            let v = if value.is_null() { None } else { Some(unsafe { std::ffi::CStr::from_ptr(value) }.to_bytes()) };
            write(n, v);
            return 1;
        }
    }
    unsafe { win::set_environment_variable_a(name, value) }
}

/// `GetEnvironmentStringsW`: the same program view as `env_snapshot`, in a Windows block.
#[cfg(windows)]
pub extern "system" fn program_get_environment_strings_w() -> *mut u16 {
    let mut block = Vec::new();
    for (k, v) in program_view(&real(), &own()) {
        block.extend(wide(&k));
        block.push(b'=' as u16);
        block.extend(wide(&v));
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    let mut block = block.into_boxed_slice();
    let p = block.as_mut_ptr();
    win::BLOCKS.lock().unwrap().push(block);
    p
}

#[cfg(windows)]
pub extern "system" fn program_free_environment_strings_w(env: *mut u16) -> i32 {
    let block = {
        let mut blocks = win::BLOCKS.lock().unwrap();
        blocks.iter().position(|b| b.as_ptr() == env).map(|i| blocks.swap_remove(i))
    };
    if let Some(block) = block {
        drop(block);
        1
    } else {
        unsafe { win::free_environment_strings_w(env) }
    }
}

/// Relax std's plumbing reads the real environment, without the program's overlay.
#[cfg(windows)]
pub extern "C" fn host_get_environment_variable_w(name: *const u16, buf: *mut u16, size: u32) -> u32 {
    unsafe { win::get_environment_variable_w(name, buf, size) }
}

std::thread_local! {
    /// the last `env_snapshot` of this thread (std copies it right away)
    static SNAPSHOT: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Relax std's `vars_os` and a spawn's environment: the program's view of the environment, every
/// entry set (`encode`), valid until this thread's next call.
pub extern "C" fn env_snapshot(out: *mut *const u8, out_len: *mut usize) {
    let view: Own = program_view(&real(), &own()).into_iter().map(|(k, v)| (k, Some(v))).collect();
    SNAPSHOT.with(|b| {
        let mut b = b.borrow_mut();
        *b = encode(&view);
        unsafe {
            *out = b.as_ptr();
            *out_len = b.len();
        }
    });
}

/// A self-exec child's start, before any thread or environment read: the program values its parent
/// passed (`PROGRAM_ENV`) become this process's overlay and leave its environment, so the host
/// reads its own settings and the program its own. Absent: a launch by a person or a tool, with
/// no program values.
pub fn init() {
    let Some(v) = std::env::var_os(std::str::from_utf8(PROGRAM_ENV).unwrap()) else { return };
    std::env::remove_var(std::str::from_utf8(PROGRAM_ENV).unwrap());
    let own = parse(v.as_encoded_bytes()).unwrap_or_else(|e| panic!("MAKEPAD_RUN_PROGRAM_ENV from the parent's Relax std: {e}"));
    for (k, v) in &own {
        write(k, v.as_deref());
    }
}

/// Relax std's spawn: the child's environment (`child_env` over this process's environment and the
/// program's values), every entry set (`encode`), valid until this thread's next call. `edits` are
/// the Command's, `host` std's plumbing values for the child's host, both in the wire form;
/// `flags`: 1 env_clear, 2 self-exec, 4 a virtual folder (its WORKSPACE and VFS are in `host`),
/// 8 a real folder.
pub extern "C" fn spawn_env(edits: *const u8, edits_len: usize, host: *const u8, host_len: usize, flags: u32, out: *mut *const u8, out_len: *mut usize) {
    let bytes = |p: *const u8, n: usize| if n == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(p, n) } };
    let edits = parse(bytes(edits, edits_len)).expect("a spawn's edits from Relax std");
    let mut host: Env = parse(bytes(host, host_len)).expect("a spawn's host values from Relax std").into_iter().filter_map(|(k, v)| Some((k, v?))).collect();
    let mount: Env = host.iter().filter(|(k, _)| same_name(k, WORKSPACE) || same_name(k, VFS)).cloned().collect();
    let dir = if flags & 4 != 0 {
        host.retain(|(k, _)| !same_name(k, WORKSPACE) && !same_name(k, VFS));
        let get = |n: &[u8]| mount.iter().find(|(k, _)| same_name(k, n)).map_or(&[][..], |(_, v)| v.as_slice());
        ChildDir::Virtual { workspace: get(WORKSPACE), vfs: get(VFS) }
    } else if flags & 8 != 0 {
        ChildDir::Real
    } else {
        ChildDir::Same
    };
    let spawn = Spawn { edits: &edits, clear: flags & 1 != 0, self_exec: flags & 2 != 0, dir, host: &host };
    let env: Own = child_env(&real(), &own(), &spawn).into_iter().map(|(k, v)| (k, Some(v))).collect();
    SNAPSHOT.with(|b| {
        let mut b = b.borrow_mut();
        *b = encode(&env);
        unsafe {
            *out = b.as_ptr();
            *out_len = b.len();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }
    fn get<'a>(env: &'a Env, k: &str) -> Option<&'a [u8]> {
        env.iter().find(|(n, _)| n == k.as_bytes()).map(|(_, v)| v.as_slice())
    }
    fn program_env(env: &Env) -> Own {
        get(env, "MAKEPAD_RUN_PROGRAM_ENV").map(|v| parse(v).unwrap()).unwrap_or_default()
    }

    /// One environment, the spawns a program makes from it, each assert naming its property.
    #[test]
    fn child_env_gives_each_child_its_side() {
        let real: Env = vec![
            (b("PATH"), b("/bin")),
            (b("MAKEPAD_RUN_CACHE_DIR"), b("/host/cache")),
            (b("MAKEPAD_RUN_SELF_EXEC"), b("/relax\x1frun\x1f--")),
            (b("MAKEPAD_RUN_LINT_FLAGS"), b("-D warnings")),
            (b("MAKEPAD_RUN_WORKSPACE"), b("/ws")),
            (b("MAKEPAD_RUN_VFS"), b("mounts")),
            (b("MAKEPAD_RUN_EDITS"), b("/edits")),
        ];
        // the program set STD in process and removed LINT_FLAGS
        let own: Own = vec![(b("MAKEPAD_RUN_STD"), Some(b("/std"))), (b("MAKEPAD_RUN_LINT_FLAGS"), None)];
        let edits = vec![
            (b("MAKEPAD_RUN_BOOT_TRACE"), Some(b("1"))),
            (b("MAKEPAD_RUN_CACHE_DIR"), Some(b("/scratch/cache"))),
            (b("HOME"), Some(b("/h"))),
            (b("PATH"), None),
        ];
        let spawn = Spawn { edits: &edits, clear: false, self_exec: true, dir: ChildDir::Same, host: &[] };

        let view = program_view(&real, &own);
        assert_eq!(get(&view, "MAKEPAD_RUN_STD"), Some(&b"/std"[..]), "the program reads its own set value");
        assert_eq!(get(&view, "MAKEPAD_RUN_LINT_FLAGS"), None, "the program reads its own removal");
        assert_eq!(get(&view, "MAKEPAD_RUN_CACHE_DIR"), Some(&b"/host/cache"[..]), "a name the program did not write reads the host's");

        let child = child_env(&real, &own, &spawn);
        for (k, v) in real.iter().filter(|(k, _)| reserved(k)) {
            let k = std::str::from_utf8(k).unwrap();
            assert_eq!(get(&child, k), Some(v.as_slice()), "a self-exec child keeps the host's reserved value of {k}");
        }
        assert_eq!(get(&child, "MAKEPAD_RUN_BOOT_TRACE"), None, "a program value is no variable of the self-exec child's host");
        assert_eq!(get(&child, "MAKEPAD_RUN_STD"), None, "an in-process program value is no variable of the self-exec child's host");
        let mut travel = program_env(&child);
        travel.sort();
        assert_eq!(travel, vec![
            (b("MAKEPAD_RUN_BOOT_TRACE"), Some(b("1"))),
            (b("MAKEPAD_RUN_CACHE_DIR"), Some(b("/scratch/cache"))),
            (b("MAKEPAD_RUN_LINT_FLAGS"), None),
            (b("MAKEPAD_RUN_STD"), Some(b("/std"))),
        ], "the program's set and removed reserved names travel in PROGRAM_ENV, and only those");
        assert_eq!(get(&child, "HOME"), Some(&b"/h"[..]), "a name outside the namespace comes from the program's view (set)");
        assert_eq!(get(&child, "PATH"), None, "a name outside the namespace comes from the program's view (removed)");

        let cleared = child_env(&real, &own, &Spawn { edits: &[], clear: true, self_exec: true, dir: ChildDir::Same, host: &[] });
        assert_eq!(get(&cleared, "MAKEPAD_RUN_CACHE_DIR"), Some(&b"/host/cache"[..]), "after env_clear the self-exec child's host still has its own settings");
        assert!(program_env(&cleared).iter().all(|(_, v)| v.is_none()) && program_env(&cleared).len() == real.iter().filter(|(k, _)| reserved(k)).count(),
            "after env_clear the self-exec child program sees no reserved name: {:?}", program_env(&cleared));
        assert_eq!(get(&cleared, "PATH"), None, "env_clear clears the names outside the namespace for both");

        let plain = child_env(&real, &[], &Spawn { edits: &[], clear: false, self_exec: true, dir: ChildDir::Same, host: &[] });
        assert_eq!(get(&plain, "MAKEPAD_RUN_PROGRAM_ENV"), None, "a self-exec child of a program with no values of its own gets no PROGRAM_ENV");

        let foreign = child_env(&real, &own, &Spawn { self_exec: false, ..spawn });
        assert_eq!(get(&foreign, "MAKEPAD_RUN_BOOT_TRACE"), Some(&b"1"[..]), "a foreign child gets the Command's values as plain variables");
        assert_eq!(get(&foreign, "MAKEPAD_RUN_STD"), Some(&b"/std"[..]), "a foreign child gets the in-process program values as plain variables");
        assert_eq!(get(&foreign, "MAKEPAD_RUN_LINT_FLAGS"), None, "a foreign child does not get a name the program removed");
        assert_eq!(get(&foreign, "MAKEPAD_RUN_PROGRAM_ENV"), None, "a foreign child gets no PROGRAM_ENV");

        let virt = child_env(&real, &own, &Spawn { dir: ChildDir::Virtual { workspace: b"/ws/sub", vfs: b"mounts" }, ..spawn });
        assert_eq!((get(&virt, "MAKEPAD_RUN_WORKSPACE"), get(&virt, "MAKEPAD_RUN_VFS")), (Some(&b"/ws/sub"[..]), Some(&b"mounts"[..])),
            "a child in a virtual folder gets that folder and the mount table");
        let real_dir = child_env(&real, &own, &Spawn { dir: ChildDir::Real, ..spawn });
        assert_eq!(get(&real_dir, "MAKEPAD_RUN_WORKSPACE"), None, "a child in a real folder names no virtual workspace");
        assert_eq!((get(&real_dir, "MAKEPAD_RUN_VFS"), get(&real_dir, "MAKEPAD_RUN_EDITS")), (Some(&b"mounts"[..]), Some(&b"/edits"[..])),
            "a self-exec child in a real folder keeps the mount its --workspace-dir reads");
        let same = child_env(&real, &own, &spawn);
        assert_eq!(get(&same, "MAKEPAD_RUN_WORKSPACE"), Some(&b"/ws"[..]), "a child in this process's folder keeps its workspace");

        let mount = |env: &Env| MOUNT.map(|n| get(env, std::str::from_utf8(n).unwrap()).map(<[u8]>::to_vec));
        let all = [Some(b("/ws")), Some(b("mounts")), Some(b("/edits"))];
        let foreign_same = child_env(&real, &own, &Spawn { self_exec: false, ..spawn });
        assert_eq!(get(&foreign_same, "MAKEPAD_RUN_SELF_EXEC"), None, "a foreign child gets no run fact the program did not write");
        assert_eq!(mount(&foreign_same), all, "the mount crosses into a foreign child in this process's folder");
        let foreign_virt = child_env(&real, &own, &Spawn { self_exec: false, dir: ChildDir::Virtual { workspace: b"/ws/sub", vfs: b"mounts" }, ..spawn });
        assert_eq!(mount(&foreign_virt), [Some(b("/ws/sub")), Some(b("mounts")), Some(b("/edits"))], "the mount crosses into a foreign child in a folder of the mount");
        let foreign_real = child_env(&real, &own, &Spawn { self_exec: false, dir: ChildDir::Real, ..spawn });
        assert_eq!(mount(&foreign_real), [None, None, None], "a foreign child in a real folder gets none of the mount");
        let launch = vec![(b("MAKEPAD_RUN_EDITS"), Some(b("/app/edits")))];
        let launched = child_env(&real, &own, &Spawn { edits: &launch, self_exec: false, dir: ChildDir::Real, ..spawn });
        assert_eq!(get(&launched, "MAKEPAD_RUN_EDITS"), Some(&b"/app/edits"[..]), "a run fact the program gives its child is the child's");
        let cleared_foreign = child_env(&real, &own, &Spawn { edits: &[], clear: true, self_exec: false, dir: ChildDir::Same, host: &[] });
        assert_eq!(mount(&cleared_foreign), [None, None, None], "env_clear clears the mount of a foreign child in this process's folder");
        let origin = vec![(b("MAKEPAD_RUN_EXEC_ORIGIN_EPOCH"), b("12.5"))];
        let exec = child_env(&real, &own, &Spawn { host: &origin, ..spawn });
        assert_eq!(get(&exec, "MAKEPAD_RUN_EXEC_ORIGIN_EPOCH"), Some(&b"12.5"[..]), "std's plumbing values reach the child's host");
        assert!(program_env(&exec).iter().all(|(k, _)| k != b"MAKEPAD_RUN_EXEC_ORIGIN_EPOCH"), "std's plumbing values are no program value");

        let odd: Own = vec![
            (b("MAKEPAD_RUN_A"), Some(b("x=y\x1fz"))),
            (b("MAKEPAD_RUN_B"), Some(Vec::new())),
            (b("MAKEPAD_RUN_C"), None),
            (b("MAKEPAD_RUN_D"), Some(vec![0xff, 0xfe, b':', b'9'])),
        ];
        assert_eq!(parse(&encode(&odd)).unwrap(), odd, "the wire form round-trips `=`, `\\x1f`, an empty value, a removal and non-UTF-8 bytes");
        assert!(parse(b"12:MAKEPAD_RUN").is_err(), "a truncated entry is an error, not a shorter list");
    }

    /// `n` as the raw platform call reads it: libc getenv (an allocator's flags) on unix,
    /// GetEnvironmentVariableA on Windows (std reads through the W call).
    #[cfg(any(unix, windows))]
    fn raw(n: &str) -> Option<Vec<u8>> {
        let c = std::ffi::CString::new(n).unwrap();
        #[cfg(unix)]
        {
            let p = unsafe { super::libc::getenv(c.as_ptr()) };
            (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            // the size with its NUL first; 0: not set
            let need = unsafe { super::win::get_environment_variable_a(c.as_ptr(), std::ptr::null_mut(), 0) } as usize;
            if need == 0 {
                return None;
            }
            let mut buf = vec![0u8; need];
            let got = unsafe { super::win::get_environment_variable_a(c.as_ptr(), buf.as_mut_ptr().cast(), need as u32) } as usize;
            buf.truncate(got);
            Some(buf)
        }
    }

    /// A program's writes of reserved names, read back the three ways a program reads: std's
    /// var_os, the raw platform call and vars(). Trivial under rustc; under the JIT the proof that
    /// the interposed platform calls and std's snapshot agree.
    #[cfg(any(unix, windows))]
    #[test]
    fn a_programs_reserved_writes_read_back_every_way() {
        let listed = |n: &str| std::env::vars_os().find(|(k, _)| k == n).map(|(_, v)| v.into_encoded_bytes());
        std::env::set_var("MAKEPAD_RUN_PROGENV_T2", "a=1");
        assert_eq!(std::env::var("MAKEPAD_RUN_PROGENV_T2").as_deref(), Ok("a=1"), "var_os reads the program's set value");
        assert_eq!(raw("MAKEPAD_RUN_PROGENV_T2"), Some(b("a=1")), "the raw call reads the program's set value");
        assert_eq!(listed("MAKEPAD_RUN_PROGENV_T2"), Some(b("a=1")), "vars() lists the program's set value");
        #[cfg(windows)]
        {
            assert_eq!(std::env::var("makepad_run_progenv_t2").as_deref(), Ok("a=1"), "a name in another case is the same name on Windows");
            assert_eq!(raw("Makepad_Run_Progenv_T2"), Some(b("a=1")), "the raw call matches names in any case on Windows");
        }
        std::env::remove_var("MAKEPAD_RUN_PROGENV_T2");
        assert!(std::env::var_os("MAKEPAD_RUN_PROGENV_T2").is_none(), "var_os reads the program's removal");
        assert_eq!(raw("MAKEPAD_RUN_PROGENV_T2"), None, "the raw call reads the program's removal");
        assert_eq!(listed("MAKEPAD_RUN_PROGENV_T2"), None, "vars() does not list a removed name");
    }

    #[cfg(unix)]
    const STAGE: &str = "PROGENV_SPAWN_STAGE";
    #[cfg(unix)]
    const SPAWNED: [&str; 5] = ["MAKEPAD_RUN_BOOT_TRACE", "MAKEPAD_RUN_TRACE", "MAKEPAD_RUN_CACHE_DIR", "MAKEPAD_RUN_PROGENV_GONE", "MAKEPAD_RUN_PROGENV_GRAND"];

    /// Each name of SPAWNED as std::env and the raw libc call read it, one `| ` line each (after a
    /// line break: rustc's harness prints the test's name before its output on the same line).
    #[cfg(unix)]
    fn report() -> String {
        let mut out = String::from("\n");
        for n in SPAWNED {
            let raw = raw(n).map(|v| String::from_utf8_lossy(&v).into_owned());
            out.push_str(&format!("| {n} {:?} {raw:?}\n", std::env::var(n).ok()));
        }
        out.push_str(&format!("| PROGRAM_ENV listed {}\n", std::env::vars_os().any(|(k, _)| k == "MAKEPAD_RUN_PROGRAM_ENV")));
        out
    }

    #[cfg(unix)]
    fn rerun(stage: &str) -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "progenv::tests::a_child_of_this_program_reads_what_its_parent_gave_it", "--nocapture", "--test-threads=1"]).env(STAGE, stage);
        c
    }

    /// The program spawning itself: the child and its own child read exactly what their parents
    /// gave them, the child's host none of it (no line of its own in the child's stderr: the boot
    /// trace a program gives its child was printed by that child's host under the JIT; no store
    /// in the folder a program names as its child's cache), and a foreign child gets the program's
    /// values as plain variables. Trivial under rustc; under the JIT the proof of the spawn split.
    #[cfg(unix)]
    #[test]
    fn a_child_of_this_program_reads_what_its_parent_gave_it() {
        match std::env::var(STAGE).as_deref() {
            Ok("grandchild") => {
                print!("{}", report());
                return;
            }
            Ok("child") => {
                print!("{}", report());
                let out = rerun("grandchild").env("MAKEPAD_RUN_PROGENV_GRAND", "2").output().unwrap();
                for l in String::from_utf8_lossy(&out.stdout).lines().filter(|l| l.starts_with("| ")) {
                    println!("|{l}");
                }
                println!("| grandchild stderr {:?}", String::from_utf8_lossy(&out.stderr));
                return;
            }
            _ => {}
        }
        let scratch = std::env::temp_dir().join(format!("progenv-spawn-{}", std::process::id()));
        let cache = scratch.join("cache");
        std::env::set_var("MAKEPAD_RUN_PROGENV_GONE", "set by the parent");
        let out = rerun("child").env("MAKEPAD_RUN_BOOT_TRACE", "1").env("MAKEPAD_RUN_TRACE", "units").env("MAKEPAD_RUN_CACHE_DIR", &cache)
            .env_remove("MAKEPAD_RUN_PROGENV_GONE").output().unwrap();
        let sh = std::process::Command::new("/bin/sh").args(["-c", "echo \"$MAKEPAD_RUN_PROGENV_GONE|${MAKEPAD_RUN_PROGRAM_ENV-none}|${MAKEPAD_RUN_SELF_EXEC-none}\""]).output().unwrap();
        std::env::remove_var("MAKEPAD_RUN_PROGENV_GONE");
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned());
        assert!(out.status.success() && stdout.contains("1 passed"), "the child ran: {stdout}\n{stderr}");
        let cache_s = cache.to_string_lossy().into_owned();
        let want = |prefix: &str, grand: Option<&str>| -> Vec<String> {
            // (std::env and the raw call agree: the same value twice)
            let v = |s: Option<&str>| format!("{s:?} {s:?}");
            vec![
                format!("{prefix} MAKEPAD_RUN_BOOT_TRACE {}", v(Some("1"))),
                format!("{prefix} MAKEPAD_RUN_TRACE {}", v(Some("units"))),
                format!("{prefix} MAKEPAD_RUN_CACHE_DIR {}", v(Some(&cache_s))),
                format!("{prefix} MAKEPAD_RUN_PROGENV_GONE {}", v(None)),
                format!("{prefix} MAKEPAD_RUN_PROGENV_GRAND {}", v(grand)),
                format!("{prefix} PROGRAM_ENV listed false"),
            ]
        };
        for l in want("|", None) {
            assert!(stdout.lines().any(|s| s == l), "the child reads what its parent gave it: `{l}` in\n{stdout}");
        }
        for l in want("||", Some("2")) {
            assert!(stdout.lines().any(|s| s == l), "the grandchild reads what both parents gave it: `{l}` in\n{stdout}");
        }
        assert!(stdout.contains("| grandchild stderr \"\""), "the grandchild's host printed nothing of its own: {stdout}");
        assert!(stderr.is_empty(), "the child's host printed nothing of its own into the program's stderr:\n{stderr}");
        assert!(!scratch.exists(), "the child's host wrote nothing into the cache folder the program gave its child");
        assert_eq!(String::from_utf8_lossy(&sh.stdout).trim(), "set by the parent|none|none",
            "a foreign child gets the program's values as plain variables, no PROGRAM_ENV and no run fact");
    }
}
