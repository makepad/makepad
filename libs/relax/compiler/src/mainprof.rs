//! MAKEPAD_RUN_PROFILE_MAIN=<file>: where the program's main thread spends its time after main
//! (warm-start analysis; no debugger). A thread samples the main thread every millisecond for
//! MAKEPAD_RUN_PROFILE_MAIN_MS (default 4000) ms: the pc and the return addresses of its frame
//! record chain. At exit each sample is written as `t_ms<TAB>leaf<TAB>first JIT frame<TAB>
//! role`, where names come from the unit's code index (JIT code) or dladdr (host code), and
//! role is the outermost recognisable activity on the stack: compile (lazy compile / cache
//! link on the main thread), wait (blocked in the compiler for another thread), jit (program
//! code), host (runtime / system code called by program code).

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
use relax_session::sync::Mutex;

const FRAMES: usize = 48;

struct Sample {
    t_ms: f32,
    pcs: [u64; FRAMES],
    n: u8,
}

static SAMPLES: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
#[cfg(target_os = "macos")]
static STARTED: AtomicBool = AtomicBool::new(false);

pub fn enabled() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("MAKEPAD_RUN_PROFILE_MAIN").is_some())
}

/// Called on the main thread right before main runs.
#[cfg(target_os = "macos")]
pub fn start() {
    if !enabled() || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    extern "C" {
        fn pthread_self() -> usize;
        fn pthread_get_stackaddr_np(t: usize) -> *mut u8;
        fn pthread_get_stacksize_np(t: usize) -> usize;
        fn pthread_mach_thread_np(t: usize) -> u32;
    }
    let me = unsafe { pthread_self() };
    let hi = unsafe { pthread_get_stackaddr_np(me) } as u64;
    let lo = hi - unsafe { pthread_get_stacksize_np(me) } as u64;
    let port = unsafe { pthread_mach_thread_np(me) };
    let ms: u64 = std::env::var("MAKEPAD_RUN_PROFILE_MAIN_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(4000);
    let _ = std::thread::Builder::new().name("run-mainprof".to_string()).spawn(move || {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, rel: i32) -> i32;
            fn thread_suspend(t: u32) -> i32;
            fn thread_resume(t: u32) -> i32;
            fn thread_get_state(t: u32, flavor: i32, state: *mut u32, count: *mut u32) -> i32;
        }
        unsafe { pthread_set_qos_class_self_np(0x21, 0) };
        let t0 = std::time::Instant::now();
        SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).reserve(ms as usize + 16);
        while t0.elapsed().as_millis() < ms as u128 {
            std::thread::sleep(std::time::Duration::from_micros(1000));
            let mut st = [0u32; 68];
            let mut count = 68u32;
            let mut s = Sample { t_ms: 0.0, pcs: [0; FRAMES], n: 0 };
            unsafe {
                if thread_suspend(port) != 0 {
                    break;
                }
                if thread_get_state(port, 6, st.as_mut_ptr(), &mut count) == 0 {
                    let w = |k: usize| (st[2 * k] as u64) | ((st[2 * k + 1] as u64) << 32);
                    s.pcs[0] = w(32);
                    s.pcs[1] = w(30); // lr: a leaf without a frame record of its own
                    let mut n = 2;
                    let mut fp = w(29);
                    while n < FRAMES && fp >= lo && fp + 16 <= hi && fp % 8 == 0 {
                        s.pcs[n] = *((fp + 8) as *const u64);
                        n += 1;
                        let next = *(fp as *const u64);
                        if next <= fp {
                            break;
                        }
                        fp = next;
                    }
                    s.n = n as u8;
                }
                thread_resume(port);
            }
            s.t_ms = t0.elapsed().as_secs_f32() * 1000.0;
            // published per sample: the process may end (and write them) while sampling
            if s.n > 0 {
                SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).push(s);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn start() {}

#[cfg(windows)]
fn host_name(pc: u64) -> String {
    format!("?{:x}", pc)
}

#[cfg(unix)]
fn host_name(pc: u64) -> String {
    #[repr(C)]
    struct DlInfo {
        fname: *const i8,
        fbase: *mut u8,
        sname: *const i8,
        saddr: *mut u8,
    }
    extern "C" {
        fn dladdr(addr: *const u8, info: *mut DlInfo) -> i32;
    }
    let mut info = DlInfo { fname: std::ptr::null(), fbase: std::ptr::null_mut(), sname: std::ptr::null(), saddr: std::ptr::null_mut() };
    if unsafe { dladdr(pc as *const u8, &mut info) } == 0 || info.sname.is_null() {
        return format!("?{:x}", pc);
    }
    let s = unsafe { std::ffi::CStr::from_ptr(info.sname.cast()) }.to_string_lossy().to_string();
    let lib = if info.fname.is_null() {
        String::new()
    } else {
        let f = unsafe { std::ffi::CStr::from_ptr(info.fname.cast()) }.to_string_lossy().to_string();
        f.rsplit('/').next().unwrap_or("").to_string()
    };
    format!("{}!{}", lib, s)
}

/// Writes the samples (at exit, unit lock held by the caller).
pub fn write(u: &crate::jit::Unit) {
    if !enabled() {
        return;
    }
    let path = match std::env::var("MAKEPAD_RUN_PROFILE_MAIN") {
        Ok(p) => p,
        Err(_) => return,
    };
    let samples = std::mem::take(&mut *SAMPLES.lock().unwrap_or_else(|e| e.into_inner()));
    let mut names: std::collections::HashMap<u64, (String, bool)> = std::collections::HashMap::new();
    let mut name = |pc: u64| -> (String, bool) {
        if let Some(x) = names.get(&pc) {
            return x.clone();
        }
        let r = if let Some(id) = u.find_fn(pc) {
            (crate::syms::stable_fn_name(u, id), true)
        } else {
            (host_name(pc), false)
        };
        names.insert(pc, r.clone());
        r
    };
    let mut text = String::new();
    for s in &samples {
        let mut leaf = String::new();
        let mut first_jit = String::new();
        let mut role = "host";
        let mut saw_jit = false;
        let mut chain = String::new();
        for (i, &pc) in s.pcs[..s.n as usize].iter().enumerate() {
            if i == 1 && pc == 0 {
                continue;
            }
            let (n, jit) = name(if i == 0 { pc } else { pc.wrapping_sub(1) });
            if i == 0 {
                leaf = n.clone();
            }
            if jit && !saw_jit {
                saw_jit = true;
                first_jit = n.clone();
                if role == "host" && i == 0 {
                    role = "jit";
                }
            }
            if !saw_jit && i > 0 {
                // the runtime frames between the leaf and the program (short names)
                if !chain.is_empty() {
                    chain.push('<');
                }
                chain.push_str(short_name(&n));
            }
            if !saw_jit {
                if n.contains("compile_slot") || n.contains("rt_compile") || n.contains("cache") && n.contains("lookup") {
                    role = "compile";
                }
                if n.contains("wait_not_inflight") || n.contains("lock_unit") || n.contains("_pthread_cond_wait") && role == "compile" {
                    role = "wait";
                }
            }
        }
        if role == "host" && first_jit.is_empty() {
            role = "outside";
        }
        text.push_str(&format!("{:.1}\t{}\t{}\t{}\t{}\n", s.t_ms, leaf, first_jit, role, chain));
    }
    let _ = std::fs::write(&path, text);
}

/// A mangled Rust symbol's last path segments (`..6stable..6Sip1285write` -> `stable::Sip128::write`
/// is not attempted; the last two identifiers are enough to tell frames apart).
fn short_name(n: &str) -> &str {
    let s = n.rsplit('!').next().unwrap_or(n);
    let mut k = s.len().saturating_sub(60);
    while !s.is_char_boundary(k) {
        k += 1;
    }
    &s[k..]
}
