//! The live image: executable memory, the per-function patch table with lazy
//! stubs, the host runtime (formatting, panics, faults, hang watchdog) and the
//! compile-on-first-call path.

use crate::lower::{fn_name, Lcx};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::{Body, Res};
use crate::types::{TyId, TyKind};
// arch backend and OS runtime dispatch (arm64 lane): both backends expose the same API
#[cfg(target_arch = "aarch64")]
use crate::arm64 as x64;
#[cfg(target_os = "macos")]
#[path = "jit_macos.rs"]
mod os;
#[cfg(target_os = "macos")]
use os::{install_signals, os_alloc, write_code, PROT_RW, PROT_RWX};
#[cfg(not(target_arch = "aarch64"))]
use crate::x64;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum FnKey {
    Inst(DefId, Vec<TyId>),
    /// (file, closure expr, parent fn slot)
    Closure(u32, u32, u32),
    /// compiler-generated helper for a type: (kind, type); GLUE_DROP = drop in place
    Glue(u8, TyId),
}

pub const GLUE_DROP: u8 = 0;
/// `dyn Fn` entry for a callable without state (fn item, fn pointer, capture-free closure):
/// `fn(env, args..) -> R` forwarding to it
pub const GLUE_CALL_SHIM: u8 = 1;
/// a tuple-struct / tuple-variant constructor used as a fn value: GLUE_CTOR + variant index,
/// for the constructor's fn pointer type `fn(fields..) -> Adt`
pub const GLUE_CTOR: u8 = 16;

pub struct FnEntry {
    pub key: FnKey,
    pub name: String,
    pub code: u64,
    pub code_len: u32,
    pub pc_map: Vec<(u32, u64)>,
    pub file: u32,
    /// previous code versions (for rollback), newest last
    pub prev: Vec<(u64, u32, Vec<(u32, u64)>, u32)>,
    pub compiled: bool,
    pub patch_gen: u32,
    /// optimised RIR (input to inlining into callers, tier-up, recompiles)
    pub rir: Option<Box<crate::rir::Func>>,
    pub building: bool,
    /// slots that inlined this function (recompiled when it is patched)
    pub inlined_into: Vec<u32>,
}

pub struct Rt {
    pub fmt_str: u64,
    pub fmt_int: u64,
    pub fmt_float: u64,
    pub fmt_print: u64,
    pub panic_site: u64,
    pub panic_bounds: u64,
    pub str_eq: u64,
    pub fmod: u64,
    pub fmodf: u64,
    /// `panic_impl(msg, msg_len, file, file_len, line, col) -> !` (core::panicking)
    pub panic_impl: u64,
    /// `thread_run(entry: fn(*mut u8), arg) -> u64`: runs `entry(arg)` under a recovery point
    /// on the calling thread; 0 = returned, 1 = panicked/faulted (report printed)
    pub thread_run: u64,
    /// `args() -> (argc, argv)`: the program's C argv (NUL-terminated strings, argv[argc] = null)
    pub args: u64,
    pub memcpy: u64,
    pub fmt_push: u64,
    pub fmt_pop: u64,
}

pub struct Site {
    pub file: u32,
    pub pos: u32,
    pub msg: String,
}

/// function slots (instances incl. generics and closures; virtual memory only)
const TABLE_CAP: usize = 1 << 18;
#[cfg(not(target_arch = "aarch64"))]
const THUNK_SIZE: usize = 16;
#[cfg(not(target_arch = "aarch64"))]
const STUB_SIZE: usize = 32;
#[cfg(target_arch = "aarch64")]
const THUNK_SIZE: usize = x64::THUNK_SIZE;
#[cfg(target_arch = "aarch64")]
const STUB_SIZE: usize = x64::STUB_SIZE;

pub struct Unit {
    pub prog: Program,
    pub tcx: Tcx,
    pub lay: crate::layout::Layouts,
    pub fns: Vec<FnEntry>,
    pub fn_map: HashMap<FnKey, u32>,
    pub consts: HashMap<(DefId, Vec<TyId>), u64>,
    pub statics: HashMap<DefId, u64>,
    pub sites: Vec<Site>,
    pub rt: Rt,
    pub bodies: HashMap<u32, Body>,
    pub drop_cache: HashMap<TyId, bool>,
    /// (concrete type, dyn type) -> vtable address
    pub vtables: HashMap<(TyId, TyId), u64>,
    pub lang_cache: HashMap<String, Option<DefId>>,
    pub cur_fn: u32,
    pub errors: Vec<String>,
    pub core_crate: u32,
    // memory
    exec_base: u64,
    exec_size: u64,
    pub table_base: u64,
    thunk_base: u64,
    stub_base: u64,
    lazy_entry: u64,
    pub enter: u64,
    pub leave: u64,
    code_next: u64,
    data_next: u64,
    data_end: u64,
    /// immutable data range (consts, literals): loads from it fold at compile time
    pub ro_base: u64,
    rw_next: u64,
    rw_end: u64,
    pub opt: crate::opt::OptCfg,
    pub env: x64::Env,
    pub stats: Stats,
    pub patch_log: Vec<(u32, u32)>, // (slot, generation)
    pub patch_gen: u32,
    /// bytes of the per-thread block handed out by `tls_alloc`
    pub tls_next: u32,
    /// panicking stand-ins for foreign fns this host lacks, by symbol
    missing_stubs: HashMap<String, u64>,
}

#[derive(Default)]
pub struct Stats {
    pub inline_rules: [u32; 3],
    pub compiled: u32,
    pub opt_ns: u64,
    pub rir_insts: u64,
    pub typeck_ns: u64,
    pub lower_ns: u64,
    pub codegen_ns: u64,
    pub code_bytes: u64,
}

// ------------------------------------------------------------ OS memory

extern "C" {
    fn memcpy(d: *mut std::ffi::c_void, s: *const std::ffi::c_void, n: usize) -> *mut std::ffi::c_void;
    fn pthread_self() -> usize;
    fn pthread_kill(t: usize, sig: i32) -> i32;
}

#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

extern "C" {
    fn clock_gettime(clk: i32, ts: *mut Timespec) -> i32;
}

#[cfg(target_os = "linux")]
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
#[cfg(target_os = "macos")]
const CLOCK_THREAD_CPUTIME_ID: i32 = 16;

/// CPU time of the calling thread in ns (compile-time stats are robust to machine load).
pub fn cpu_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe {
        clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// SIGUSR1: the hang watchdog's signal
#[cfg(target_os = "linux")]
const SIG_WATCHDOG: i32 = 10;
#[cfg(target_os = "macos")]
const SIG_WATCHDOG: i32 = 30;

#[cfg(target_os = "linux")]
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    pad: i32,
    restorer: usize,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct StackT {
    sp: *mut u8,
    flags: i32,
    pad: i32,
    size: usize,
}

#[cfg(target_os = "linux")]
const PROT_RWX: i32 = 7;
#[cfg(target_os = "linux")]
const PROT_RW: i32 = 3;
#[cfg(target_os = "linux")]
const MAP_PRIVATE_ANON: i32 = 0x22;

#[cfg(target_os = "linux")]
fn os_alloc(size: usize, prot: i32) -> u64 {
    let p = unsafe { mmap(std::ptr::null_mut(), size, prot, MAP_PRIVATE_ANON, -1, 0) };
    if p as isize == -1 {
        panic!("mmap failed");
    }
    p as u64
}

unsafe fn write_bytes(addr: u64, b: &[u8]) {
    std::ptr::copy_nonoverlapping(b.as_ptr(), addr as *mut u8, b.len());
}

/// Writes machine code (macOS: W^X flip + icache flush in jit_macos.rs).
#[cfg(target_os = "linux")]
unsafe fn write_code(addr: u64, b: &[u8]) {
    write_bytes(addr, b)
}

// ------------------------------------------------------------ thread-local blocks

/// Size of each thread's thread-local block (virtual; pages are touched on use).
const TLS_CAP: usize = 1 << 20;
static TLS_KEY: AtomicU64 = AtomicU64::new(u64::MAX);

extern "C" {
    fn dlopen(name: *const u8, flags: i32) -> *mut u8;
    fn pthread_key_create(key: *mut PthreadKey, dtor: Option<extern "C" fn(*mut u8)>) -> i32;
    fn pthread_getspecific(key: PthreadKey) -> *mut u8;
    fn pthread_setspecific(key: PthreadKey, v: *mut u8) -> i32;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

#[cfg(target_os = "macos")]
type PthreadKey = usize;
#[cfg(target_os = "linux")]
type PthreadKey = u32;

extern "C" fn tls_dtor(p: *mut u8) {
    unsafe {
        munmap(p, TLS_CAP);
    }
}

fn tls_key() -> PthreadKey {
    let k = TLS_KEY.load(Ordering::Acquire);
    if k != u64::MAX {
        return k as PthreadKey;
    }
    lock_unit();
    let mut key: PthreadKey = 0;
    if TLS_KEY.load(Ordering::Acquire) == u64::MAX {
        unsafe {
            pthread_key_create(&mut key, Some(tls_dtor));
        }
        TLS_KEY.store(key as u64, Ordering::Release);
    }
    unlock_unit();
    TLS_KEY.load(Ordering::Acquire) as PthreadKey
}

#[cfg(target_os = "linux")]
thread_local! {
    /// this thread's block (static TLS: JIT code reads it at fs:[env.tls_key])
    static TLS_BLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

thread_local! {
    static ALTSTACK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Gives the calling thread an alternate signal stack (once), so a stack overflow in JIT
/// code on a thread the program started is still reported. Called on the thread's first
/// lazy compile / thread-local block and from the signal handler.
fn ensure_altstack() {
    if ALTSTACK.with(|a| a.replace(true)) {
        return;
    }
    os::altstack_this_thread();
}

/// Allocates the calling thread's thread-local block (called by the TlsAddr slow path).
extern "C" fn rt_tls_block() -> u64 {
    ensure_altstack();
    let key = tls_key();
    unsafe {
        let b = pthread_getspecific(key);
        if !b.is_null() {
            return b as u64;
        }
        let b = os_alloc(TLS_CAP, PROT_RW);
        pthread_setspecific(key, b as *mut u8);
        #[cfg(target_os = "linux")]
        TLS_BLOCK.with(|c| c.set(b));
        b
    }
}

// ------------------------------------------------------------ global runtime state

/// The unit reached by the lazy-compile entry and the runtime: set by `call_jit` and
/// kept afterwards, so threads that JIT code started keep reaching it.
static mut UNIT: *mut Unit = std::ptr::null_mut();
static WATCHDOG_HIT: AtomicBool = AtomicBool::new(false);
static RUN_GEN: AtomicU64 = AtomicU64::new(0);

/// Re-entrant lock around everything that changes the unit (lazy compiles, patches,
/// const evaluation): JIT code on any thread may hit a lazy stub. Owner = pthread_self.
static LOCK_OWNER: AtomicU64 = AtomicU64::new(0);
static mut LOCK_DEPTH: u32 = 0;

pub fn lock_unit() {
    let me = unsafe { pthread_self() } as u64;
    if LOCK_OWNER.load(Ordering::Acquire) == me {
        unsafe { LOCK_DEPTH += 1 };
        return;
    }
    let mut spins = 0u32;
    while LOCK_OWNER.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_err() {
        spins += 1;
        if spins > 64 {
            std::thread::yield_now();
        } else {
            std::hint::spin_loop();
        }
    }
    unsafe { LOCK_DEPTH = 1 };
}

/// lock_unit unless another thread holds it.
pub fn try_lock_unit() -> bool {
    let me = unsafe { pthread_self() } as u64;
    if LOCK_OWNER.load(Ordering::Acquire) == me {
        unsafe { LOCK_DEPTH += 1 };
        return true;
    }
    if LOCK_OWNER.compare_exchange(0, me, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return false;
    }
    unsafe { LOCK_DEPTH = 1 };
    true
}

pub fn unlock_unit() {
    unsafe {
        LOCK_DEPTH -= 1;
        if LOCK_DEPTH == 0 {
            LOCK_OWNER.store(0, Ordering::Release);
        }
    }
}

/// Does the calling thread hold the unit lock (i.e. is it inside the compiler)?
fn in_compile() -> bool {
    LOCK_OWNER.load(Ordering::Relaxed) == unsafe { pthread_self() } as u64
}

thread_local! {
    /// saved host context of this thread's innermost `enter` (sp); 0 = no recovery point
    static CTX: std::cell::UnsafeCell<[u64; 2]> = const { std::cell::UnsafeCell::new([0; 2]) };
    /// stack of format buffers: format!/write! to non-Formatter targets push one
    static FMT: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(vec![String::new()]);
    /// the last popped buffer (valid until the next pop)
    static FMT_POPPED: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static PANIC: std::cell::RefCell<Option<PanicInfo>> = const { std::cell::RefCell::new(None) };
    /// "file:line:col" of a panic raised through panic_impl (std's own panics)
    static PANIC_LOC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

thread_local! {
    /// source location of the last panic caught inside the compiler (one-line diagnostics)
    static ICE_LOC: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Installs (once) a panic hook that stays silent for panics inside the compiler (they
/// become compile errors) and records their location; other panics print as usual.
fn install_ice_hook() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if in_compile() {
            if let Some(l) = info.location() {
                let s = format!(" (at {}:{})", l.file(), l.line());
                ICE_LOC.with(|c| *c.borrow_mut() = s);
            }
            return;
        }
        prev(info);
    }));
}

/// " (at src/file.rs:line)" of the last panic caught inside the compiler, once.
pub fn take_ice_location() -> String {
    ICE_LOC.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

/// A fault at or just below the stack pointer: the stack's guard page was hit (frames
/// over a page probe their pages in order, so the guard is always touched first).
fn is_stack_fault(fault: u64, sp: u64) -> bool {
    fault < sp.wrapping_add(4096) && fault.wrapping_add(1 << 20) >= sp
}

/// A fault in JIT code on a thread without a recovery point: report and abort.
unsafe fn fatal_report(sig: i32, fault: u64, fp: u64, pc: u64, sp: u64) -> ! {
    let frames = collect_frames(fp, pc);
    let kind = if is_stack_fault(fault, sp) { "stack overflow" } else { "fault" };
    let info = PanicInfo { kind: kind.to_string(), message: format!("signal {} at {:#x}", sig, fault), site: u64::MAX, frames, fault_addr: fault, location: String::new() };
    let r = (&*UNIT).report(&info);
    eprintln!("rapid: fault on a thread without a recovery point, aborting\n  report: {}", r);
    std::process::abort()
}

/// This thread's recovery context (stable address for the thread's lifetime).
fn ctx_ptr() -> *mut u64 {
    CTX.with(|c| c.get() as *mut u64)
}

#[derive(Clone)]
pub struct PanicInfo {
    pub kind: String,
    pub message: String,
    pub site: u64,
    pub frames: Vec<u64>,
    pub fault_addr: u64,
    /// "file:line:col" when the panic did not come from a compiler-known site
    pub location: String,
}

// ------------------------------------------------------------ host runtime functions

fn spec_pad(body: String, spec: u64, numeric: bool) -> String {
    let width = (spec & 0xffff) as usize;
    let align = ((spec >> 33) & 3) as u8;
    let zero = (spec >> 37) & 1 != 0;
    let fill = char::from_u32(((spec >> 48) & 0xffff) as u32).unwrap_or(' ');
    let n = body.chars().count();
    if n >= width {
        return body;
    }
    let pad = width - n;
    if zero && numeric {
        // zeros after sign / 0x prefix
        let (sign, rest) = if body.starts_with('-') || body.starts_with('+') { (body[..1].to_string(), body[1..].to_string()) } else { (String::new(), body.clone()) };
        let (pfx, digits) = if rest.starts_with("0x") || rest.starts_with("0b") || rest.starts_with("0o") { (rest[..2].to_string(), rest[2..].to_string()) } else { (String::new(), rest) };
        return format!("{}{}{}{}", sign, pfx, "0".repeat(pad), digits);
    }
    let fill_s = |k: usize| -> String {
        let mut s = String::new();
        for _ in 0..k {
            s.push(fill);
        }
        s
    };
    let align = if align == 0 { if numeric { 3 } else { 1 } } else { align };
    match align {
        1 => format!("{}{}", body, fill_s(pad)),
        2 => format!("{}{}{}", fill_s(pad / 2), body, fill_s(pad - pad / 2)),
        _ => format!("{}{}", fill_s(pad), body),
    }
}

extern "C" fn rt_fmt_str(p: *const u8, len: u64, spec: u64) {
    let s = unsafe { std::slice::from_raw_parts(p, len as usize) };
    let s = String::from_utf8_lossy(s).into_owned();
    let ty = ((spec >> 40) & 0xff) as u8;
    let body = if ty == b'?' { format!("{:?}", s) } else { s };
    let out = if spec & 0xffff != 0 { spec_pad(body, spec, false) } else { body };
    fmt_append(&out);
}

extern "C" fn rt_fmt_int(v: u64, kind: u64, spec: u64) {
    let bits = (kind >> 8) as u32;
    let k = kind & 0xff;
    let ty = ((spec >> 40) & 0xff) as u8;
    let alt = (spec >> 36) & 1 != 0;
    let plus = (spec >> 35) & 1 != 0;
    let mask: u64 = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
    let body = match k {
        3 => (if v != 0 { "true" } else { "false" }).to_string(),
        4 => {
            let c = char::from_u32(v as u32).unwrap_or('?');
            if ty == b'?' {
                format!("{:?}", c)
            } else {
                c.to_string()
            }
        }
        _ => match ty {
            b'x' => {
                if alt {
                    format!("{:#x}", v & mask)
                } else {
                    format!("{:x}", v & mask)
                }
            }
            b'X' => format!("{:X}", v & mask),
            b'b' => format!("{:b}", v & mask),
            b'o' => format!("{:o}", v & mask),
            _ => {
                let s = if k == 1 { format!("{}", v as i64) } else { format!("{}", v) };
                if plus && !s.starts_with('-') {
                    format!("+{}", s)
                } else {
                    s
                }
            }
        },
    };
    let out = spec_pad(body, spec, k == 1 || k == 2);
    fmt_append(&out);
}

extern "C" fn rt_fmt_float(v: f64, is64: u64, spec: u64) {
    let ty = ((spec >> 40) & 0xff) as u8;
    let has_prec = (spec >> 32) & 1 != 0;
    let prec = ((spec >> 16) & 0xffff) as usize;
    let plus = (spec >> 35) & 1 != 0;
    let body = if is64 != 0 {
        match (ty, has_prec) {
            (b'?', false) => format!("{:?}", v),
            (b'e', _) => format!("{:e}", v),
            (_, true) => format!("{:.*}", prec, v),
            _ => format!("{}", v),
        }
    } else {
        let x = v as f32;
        match (ty, has_prec) {
            (b'?', false) => format!("{:?}", x),
            (b'e', _) => format!("{:e}", x),
            (_, true) => format!("{:.*}", prec, x),
            _ => format!("{}", x),
        }
    };
    let body = if plus && !body.starts_with('-') { format!("+{}", body) } else { body };
    let out = spec_pad(body, spec, true);
    fmt_append(&out);
}

fn fmt_append(s: &str) {
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        if v.is_empty() {
            v.push(String::new());
        }
        let n = v.len();
        v[n - 1].push_str(s);
    });
}

fn fmt_take_top() -> String {
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        if v.is_empty() {
            v.push(String::new());
        }
        let n = v.len();
        std::mem::take(&mut v[n - 1])
    })
}

extern "C" fn rt_fmt_push() {
    FMT.with(|f| f.borrow_mut().push(String::new()));
}

#[repr(C)]
pub struct StrRet {
    ptr: *const u8,
    len: u64,
}

extern "C" fn rt_fmt_pop() -> StrRet {
    let s = FMT.with(|f| {
        let mut v = f.borrow_mut();
        let s = v.pop().unwrap_or_default();
        if v.is_empty() {
            v.push(String::new());
        }
        s
    });
    FMT_POPPED.with(|p| {
        *p.borrow_mut() = s;
        let b = p.borrow();
        StrRet { ptr: b.as_ptr(), len: b.len() as u64 }
    })
}

extern "C" fn rt_fmt_print(stream: u64) {
    let s = fmt_take_top();
    use std::io::Write;
    if stream == 2 {
        let _ = std::io::stderr().write_all(s.as_bytes());
    } else {
        let _ = std::io::stdout().write_all(s.as_bytes());
    }
}

extern "C" fn rt_str_eq(p1: *const u8, l1: u64, p2: *const u8, l2: u64) -> u64 {
    if l1 != l2 {
        return 0;
    }
    let a = unsafe { std::slice::from_raw_parts(p1, l1 as usize) };
    let b = unsafe { std::slice::from_raw_parts(p2, l2 as usize) };
    (a == b) as u64
}

extern "C" fn rt_fmod(a: f64, b: f64) -> f64 {
    a % b
}
extern "C" fn rt_fmodf(a: f32, b: f32) -> f32 {
    a % b
}

fn collect_frames(rbp: u64, ret: u64) -> Vec<u64> {
    let mut frames = vec![ret];
    let mut bp = rbp;
    let u = unsafe { &*UNIT };
    for _ in 0..64 {
        if bp == 0 || bp & 7 != 0 {
            break;
        }
        let ra = unsafe { *((bp + 8) as *const u64) };
        if u.find_fn(ra).is_none() {
            break;
        }
        frames.push(ra);
        bp = unsafe { *(bp as *const u64) };
    }
    frames
}

fn raise(kind: &str, message: String, site: u64, fault: u64, rbp: u64, ret: u64) -> ! {
    lock_unit();
    let frames = collect_frames(rbp, ret);
    let location = PANIC_LOC.with(|l| std::mem::take(&mut *l.borrow_mut()));
    let info = PanicInfo { kind: kind.to_string(), message, site, frames, fault_addr: fault, location };
    if unsafe { *ctx_ptr() } == 0 {
        // no recovery point on this thread (a thread JIT code started, or a C callback on
        // a foreign thread): report and abort, as panic=abort does
        let r = unsafe { (&*UNIT).report(&info) };
        eprintln!("rapid: {} on a thread without a recovery point, aborting\n  report: {}", kind, r);
        std::process::abort();
    }
    unlock_unit();
    PANIC.with(|p| *p.borrow_mut() = Some(info));
    FMT.with(|f| {
        let mut v = f.borrow_mut();
        v.clear();
        v.push(String::new());
    });
    unsafe {
        let leave: extern "C" fn(*mut u64, u64) -> ! = std::mem::transmute((&*UNIT).leave as usize);
        leave(ctx_ptr(), 1)
    }
}

// Panic entries are reached through `panic_tramp`, which passes the JIT caller's frame
// (frame pointer, return address) as arguments 6 and 7 (x6/x7 on arm64, the first two
// stack arguments on x86-64), so entries keep up to six arguments of their own.

#[allow(clippy::too_many_arguments)]
extern "C" fn rt_panic_site(site: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, rbp: u64, ret: u64) {
    let mut msg = fmt_take_top();
    if msg.is_empty() {
        let u = unsafe { &*UNIT };
        if (site as usize) < u.sites.len() {
            msg = u.sites[site as usize].msg.clone();
        }
    }
    raise("panic", msg, site, 0, rbp, ret)
}

#[allow(clippy::too_many_arguments)]
extern "C" fn rt_panic_bounds(idx: u64, len: u64, site: u64, _a3: u64, _a4: u64, _a5: u64, rbp: u64, ret: u64) {
    let msg = format!("index out of bounds: the len is {} but the index is {}", len, idx);
    raise("panic", msg, site, 0, rbp, ret)
}

#[allow(clippy::too_many_arguments)]
extern "C" fn rt_panic_impl(msg: *const u8, len: u64, file: *const u8, file_len: u64, line: u64, col: u64, rbp: u64, ret: u64) {
    let m = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(msg, len as usize)).into_owned() };
    let f = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(file, file_len as usize)).into_owned() };
    PANIC_LOC.with(|l| *l.borrow_mut() = format!("{}:{}:{}", f, line as u32, col as u32));
    raise("panic", m, u64::MAX, 0, rbp, ret)
}

static PROGRAM_ARGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
static ARGV: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();

/// Sets the program's argv (argv[0] first) before it starts; default: this process's args.
pub fn set_program_args(args: Vec<String>) {
    if let Ok(mut a) = PROGRAM_ARGS.lock() {
        *a = args;
    }
}

#[repr(C)]
pub struct ArgsRet {
    argc: u64,
    argv: u64,
}

/// (argc, argv) as C sees them, built once (the strings and the array live forever).
extern "C" fn rt_args() -> ArgsRet {
    let (argc, argv) = *ARGV.get_or_init(|| {
        let mut v = PROGRAM_ARGS.lock().map(|a| a.clone()).unwrap_or_default();
        if v.is_empty() {
            v = std::env::args().collect();
        }
        let mut ptrs: Vec<*const u8> = Vec::new();
        for a in &v {
            let mut b = a.clone().into_bytes();
            b.push(0);
            ptrs.push(Box::leak(b.into_boxed_slice()).as_ptr());
        }
        let n = ptrs.len();
        ptrs.push(std::ptr::null());
        (n, Box::leak(ptrs.into_boxed_slice()).as_ptr() as usize)
    });
    ArgsRet { argc: argc as u64, argv: argv as u64 }
}

/// Entry of a thread the program starts: runs `entry(arg)` (a Rapid fn pointer) with a
/// recovery point, so a panic or fault ends this thread with a report instead of the process.
extern "C" fn rt_thread_run(entry: u64, arg: u64) -> u64 {
    ensure_altstack();
    let u = unsafe { UNIT };
    match unsafe { call_jit(u, entry, arg) } {
        Ok(_) => 0,
        Err(p) => {
            lock_unit();
            let r = unsafe { (&*u).report(&p) };
            unlock_unit();
            eprintln!("rapid: thread {}: {}\n  report: {}", p.kind, p.message, r);
            1
        }
    }
}

extern "C" fn rt_compile(slot: u64) -> u64 {
    ensure_altstack();
    unsafe { compile_slot(UNIT, slot as u32) }
}

#[cfg(target_os = "linux")]
extern "C" fn on_signal(sig: i32, info: *mut u8, uc: *mut u8) {
    // the handler reads the unit's tables: wait out (or, for the watchdog, skip) a compile
    // on another thread so they are not changing underneath it
    if sig == SIG_WATCHDOG {
        if in_compile() || !try_lock_unit() {
            return;
        }
    } else {
        lock_unit();
    }
    ensure_altstack();
    unsafe { on_signal_locked(sig, info, uc) };
    unlock_unit();
}

#[cfg(target_os = "linux")]
unsafe fn on_signal_locked(sig: i32, info: *mut u8, uc: *mut u8) {
    unsafe {
        let gregs = uc.add(40) as *mut u64;
        let rip = *gregs.add(16);
        let rbp = *gregs.add(10);
        let fault = *(info.add(16) as *const u64);
        if sig == SIG_WATCHDOG {
            // hang watchdog: only act when JIT code is running and not inside the compiler
            if UNIT.is_null() || *ctx_ptr() == 0 || (&*UNIT).find_fn(rip).is_none() {
                return;
            }
            WATCHDOG_HIT.store(true, Ordering::SeqCst);
            let u = &*UNIT;
            let frames = collect_frames(rbp, rip);
            PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo { kind: "hang".to_string(), message: "watchdog: the call did not return in time".to_string(), site: u64::MAX, frames, fault_addr: 0, location: String::new() }));
            *gregs.add(16) = u.leave;
            *gregs.add(8) = ctx_ptr() as u64;
            *gregs.add(9) = 3;
            return;
        }
        let u = &*UNIT;
        if *ctx_ptr() == 0 && u.find_fn(rip).is_some() {
            fatal_report(sig, fault, rbp, rip, *gregs.add(15));
        }
        if *ctx_ptr() == 0 {
            // not ours: restore default and return to crash normally
            let act = SigAction { handler: 0, mask: [0; 16], flags: 0, pad: 0, restorer: 0 };
            sigaction(sig, &act, std::ptr::null_mut());
            return;
        }
        let rsp = *gregs.add(15);
        let kind = match sig {
            11 | 7 if is_stack_fault(fault, rsp) => "stack overflow",
            11 => "segfault",
            7 => "bus error",
            4 => "illegal instruction",
            8 => "arithmetic fault",
            _ => "signal",
        };
        let frames = collect_frames(rbp, rip);
        PANIC.with(|p| *p.borrow_mut() = Some(PanicInfo { kind: kind.to_string(), message: format!("signal {} at {:#x}", sig, fault), site: u64::MAX, frames, fault_addr: fault, location: String::new() }));
        // resume at `leave(ctx, 2)`
        *gregs.add(16) = u.leave;
        *gregs.add(8) = ctx_ptr() as u64;
        *gregs.add(9) = 2;
    }
}

#[cfg(target_os = "linux")]
mod os {
    /// sigaltstack for the calling thread (64 KB)
    pub fn altstack_this_thread() {
        unsafe {
            let stack = super::os_alloc(1 << 16, super::PROT_RW);
            let ss = super::StackT { sp: stack as *mut u8, flags: 0, pad: 0, size: 1 << 16 };
            super::sigaltstack(&ss, std::ptr::null_mut());
        }
    }
}

#[cfg(target_os = "linux")]
fn install_signals() {
    ensure_altstack();
    unsafe {
        for sig in [4, 7, 8, SIG_WATCHDOG, 11] {
            // SA_SIGINFO | SA_ONSTACK | SA_NODEFER
            let act = SigAction { handler: on_signal as *const () as usize, mask: [0; 16], flags: 4 | 0x0800_0000 | 0x4000_0000, pad: 0, restorer: 0 };
            sigaction(sig, &act, std::ptr::null_mut());
        }
    }
}

/// JIT trampoline in front of a (never returning) host panic function: passes the caller's
/// frame (frame pointer, return address) as arguments 6 and 7 (thread-safe, no globals).
#[cfg(target_arch = "aarch64")]
fn panic_tramp(target: u64) -> Vec<u8> {
    x64::panic_tramp(target)
}

#[cfg(not(target_arch = "aarch64"))]
fn panic_tramp(target: u64) -> Vec<u8> {
    let mut b = Vec::new();
    // SysV: arguments 7 and 8 are the first stack arguments. Entered by `call` (rsp = 8 mod
    // 16); the target sees [rsp] = return slot, [rsp+8] = rbp, [rsp+16] = JIT return
    // address, with rsp = 8 mod 16 as after a call. The target never returns.
    // mov r10, [rsp]
    b.extend_from_slice(&[0x4c, 0x8b, 0x14, 0x24]);
    // sub rsp, 8 ; push r10 ; push rbp ; push r10
    b.extend_from_slice(&[0x48, 0x83, 0xec, 0x08, 0x41, 0x52, 0x55, 0x41, 0x52]);
    // mov r11, imm64(target); jmp r11
    b.extend_from_slice(&[0x49, 0xbb]);
    b.extend_from_slice(&target.to_le_bytes());
    b.extend_from_slice(&[0x41, 0xff, 0xe3]);
    b
}

// ------------------------------------------------------------ the unit

impl Unit {
    pub fn new(prog: Program, tcx: Tcx, core_crate: u32) -> Box<Unit> {
        let exec_size: u64 = 1 << 30;
        let exec_base = os_alloc(exec_size as usize, PROT_RWX);
        let table_base = os_alloc(TABLE_CAP * 8, PROT_RW);
        let thunk_base = exec_base;
        let stub_base = thunk_base + (TABLE_CAP * THUNK_SIZE) as u64;
        let misc = stub_base + (TABLE_CAP * STUB_SIZE) as u64;
        let data_size: u64 = 1 << 30;
        let data_base = os_alloc(data_size as usize, PROT_RW);
        let rw_size: u64 = 256 << 20;
        let rw_base = os_alloc(rw_size as usize, PROT_RW);
        let mut u = Box::new(Unit {
            prog,
            tcx,
            lay: crate::layout::Layouts::new(),
            fns: Vec::new(),
            fn_map: HashMap::new(),
            consts: HashMap::new(),
            statics: HashMap::new(),
            sites: Vec::new(),
            rt: Rt { fmt_str: 0, fmt_int: 0, fmt_float: 0, fmt_print: 0, panic_site: 0, panic_bounds: 0, str_eq: 0, fmod: 0, fmodf: 0, panic_impl: 0, thread_run: 0, args: 0, memcpy: 0, fmt_push: 0, fmt_pop: 0 },
            bodies: HashMap::new(),
            drop_cache: HashMap::new(),
            vtables: HashMap::new(),
            lang_cache: HashMap::new(),
            cur_fn: u32::MAX,
            errors: Vec::new(),
            core_crate,
            exec_base,
            exec_size,
            table_base,
            thunk_base,
            stub_base,
            lazy_entry: 0,
            enter: 0,
            leave: 0,
            code_next: misc,
            data_next: data_base,
            data_end: data_base + data_size,
            ro_base: data_base,
            rw_next: rw_base,
            rw_end: rw_base + rw_size,
            opt: crate::opt::OptCfg::from_env(),
            env: x64::Env { table_base, thunk_base, thunk_size: THUNK_SIZE as u64, tls_key: 0, tls_slow: 0 },
            stats: Stats::default(),
            patch_log: Vec::new(),
            patch_gen: 0,
            tls_next: 0,
            missing_stubs: HashMap::new(),
        });
        let lazy = x64::lazy_entry(rt_compile as *const () as usize as u64);
        u.lazy_entry = u.emit_code(&lazy);
        let (el, leave_off) = x64::enter_leave();
        u.enter = u.emit_code(&el);
        u.leave = u.enter + leave_off as u64;
        let ps = panic_tramp(rt_panic_site as *const () as usize as u64);
        let pb = panic_tramp(rt_panic_bounds as *const () as usize as u64);
        let pi = panic_tramp(rt_panic_impl as *const () as usize as u64);
        u.rt = Rt {
            fmt_str: rt_fmt_str as *const () as usize as u64,
            fmt_int: rt_fmt_int as *const () as usize as u64,
            fmt_float: rt_fmt_float as *const () as usize as u64,
            fmt_print: rt_fmt_print as *const () as usize as u64,
            panic_site: 0,
            panic_bounds: 0,
            str_eq: rt_str_eq as *const () as usize as u64,
            fmod: rt_fmod as *const () as usize as u64,
            fmodf: rt_fmodf as *const () as usize as u64,
            panic_impl: 0,
            thread_run: rt_thread_run as *const () as usize as u64,
            args: rt_args as *const () as usize as u64,
            memcpy: memcpy as *const () as usize as u64,
            fmt_push: rt_fmt_push as *const () as usize as u64,
            fmt_pop: rt_fmt_pop as *const () as usize as u64,
        };
        u.rt.panic_site = u.emit_code(&ps);
        u.rt.panic_bounds = u.emit_code(&pb);
        u.rt.panic_impl = u.emit_code(&pi);
        u.init_tls();
        install_ice_hook();
        install_signals();
        u
    }

    fn emit_code(&mut self, b: &[u8]) -> u64 {
        let at = (self.code_next + 15) & !15;
        if at + b.len() as u64 > self.exec_base + self.exec_size {
            panic!("out of code memory");
        }
        unsafe { write_code(at, b) };
        self.code_next = at + b.len() as u64;
        at
    }

    /// Copies bytes into the data area; returns their address.
    pub fn data(&mut self, b: &[u8], align: u64) -> u64 {
        let a = align.max(1);
        let at = (self.data_next + a - 1) / a * a;
        if at + b.len() as u64 > self.data_end {
            panic!("out of data memory");
        }
        unsafe { write_bytes(at, b) };
        self.data_next = at + b.len() as u64;
        at
    }

    /// Mutable data (statics).
    pub fn rw_data(&mut self, size: u64, align: u64) -> u64 {
        let a = align.max(1);
        let at = (self.rw_next + a - 1) / a * a;
        if at + size > self.rw_end {
            panic!("out of static memory");
        }
        self.rw_next = at + size;
        at
    }

    pub fn ro_range(&self) -> (u64, u64) {
        (self.ro_base, self.data_next)
    }

    /// Reserves `size` bytes of every thread's thread-local block (zeroed per thread on
    /// first use); returns the offset for `Inst::TlsAddr`.
    pub fn tls_alloc(&mut self, size: u32, align: u32) -> u32 {
        let a = align.max(1);
        let at = (self.tls_next + a - 1) / a * a;
        if (at + size) as usize > TLS_CAP {
            panic!("thread-local block full");
        }
        self.tls_next = at + size;
        at
    }

    /// Thread-local blocks: one pthread key per process holds each thread's block.
    #[cfg(target_os = "macos")]
    fn init_tls(&mut self) {
        let key = tls_key();
        let reader = self.emit_code(&x64::tsd_reader());
        let read: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(reader as usize) };
        // the inline fast path reads the TSD slot directly: check it sees pthread's value
        unsafe {
            let old = pthread_getspecific(key);
            pthread_setspecific(key, 0x5a5a_0000 as *mut u8);
            let seen = read(key as u64);
            pthread_setspecific(key, old);
            if seen != 0x5a5a_0000 {
                panic!("thread-local fast path: TSD slot {} reads {:#x}", key, seen);
            }
        }
        self.env.tls_key = key as u64;
        self.env.tls_slow = self.emit_code(&x64::tls_slow(rt_tls_block as *const () as usize as u64));
    }

    /// Linux x86-64: the block pointer lives in a static-TLS cell of this (executable)
    /// runtime, at a fixed offset from the thread pointer that JIT code reads via fs.
    #[cfg(target_os = "linux")]
    fn init_tls(&mut self) {
        let _ = tls_key();
        let reader = self.emit_code(&x64::fs_reader());
        let fs_base: extern "C" fn() -> u64 = unsafe { std::mem::transmute(reader as usize) };
        let cell = TLS_BLOCK.with(|c| c as *const std::cell::Cell<u64> as u64);
        let off = cell.wrapping_sub(fs_base()) as i64;
        if off < i32::MIN as i64 || off > i32::MAX as i64 {
            panic!("thread-local block cell is not in static TLS (offset {:#x})", off);
        }
        self.env.tls_key = off as u64;
        self.env.tls_slow = self.emit_code(&x64::tls_slow(rt_tls_block as *const () as usize as u64));
    }

    pub fn site(&mut self, file: u32, pos: u32, msg: &str) -> u64 {
        self.sites.push(Site { file, pos, msg: msg.to_string() });
        self.sites.len() as u64 - 1
    }

    /// `core::<path>` item (traits like Drop, Deref, Index; cached).
    pub fn lang(&mut self, path: &[&str]) -> Option<DefId> {
        let key = path.join("::");
        if let Some(x) = self.lang_cache.get(&key) {
            return *x;
        }
        let prog = &self.prog;
        let mut cur = Some(prog.mods[prog.crates[self.core_crate as usize].root_mod as usize].def);
        for p in path {
            cur = match (cur, prog.syms.get(p)) {
                (Some(c), Some(s)) => match prog.lookup_in_container(c, s, true) {
                    Some(d) => Some(d),
                    None => prog.lookup_in_container(c, s, false),
                },
                _ => None,
            };
        }
        self.lang_cache.insert(key, cur);
        cur
    }

    /// Does dropping a value of type `t` run any code?
    pub fn needs_drop(&mut self, t: TyId) -> bool {
        if let Some(&b) = self.drop_cache.get(&t) {
            return b;
        }
        // recursive types: assume no while computing (a cycle only goes through pointers)
        self.drop_cache.insert(t, false);
        let k = self.tcx.tys.kind(t).clone();
        let r = match k {
            TyKind::Adt(d, args) => {
                let has_impl = match self.lang(&["ops", "Drop"]) {
                    Some(td) => self.tcx.find_impl(td, t, &[]).is_some(),
                    None => false,
                };
                if has_impl {
                    true
                } else {
                    let adt = self.tcx.adts.get(&d).cloned();
                    let mut any = false;
                    if let Some(adt) = adt {
                        if !adt.is_union {
                            for v in &adt.variants {
                                for f in &v.fields {
                                    let ft = self.tcx.tys.subst(f.ty, &args);
                                    if self.needs_drop(ft) {
                                        any = true;
                                    }
                                }
                            }
                        }
                    }
                    any
                }
            }
            TyKind::Tuple(v) => {
                let mut any = false;
                for x in v {
                    if self.needs_drop(x) {
                        any = true;
                    }
                }
                any
            }
            TyKind::Array(e, n) => n > 0 && self.needs_drop(e),
            TyKind::Closure(_, _, _, up, _) => self.needs_drop(up),
            TyKind::Dyn(..) => true,
            _ => false,
        };
        self.drop_cache.insert(t, r);
        r
    }

    /// Methods of a trait object's vtable in slot order: the trait's methods with a `self`
    /// receiver in declaration order, then each supertrait's.
    pub fn dyn_methods(&self, td: DefId) -> Vec<DefId> {
        let mut out = Vec::new();
        let mut seen = Vec::new();
        self.dyn_methods_into(td, &mut out, &mut seen);
        out
    }

    fn dyn_methods_into(&self, td: DefId, out: &mut Vec<DefId>, seen: &mut Vec<DefId>) {
        if seen.contains(&td) {
            return;
        }
        seen.push(td);
        for &it in &self.prog.traits[self.prog.def(td).sub as usize] {
            if let Some(sig) = self.tcx.sigs.get(&it) {
                if sig.self_kind != 0 {
                    out.push(it);
                }
            }
        }
        for sup in self.tcx.trait_supers(&self.prog, td) {
            self.dyn_methods_into(sup, out, seen);
        }
    }

    /// The function of a closure type (its body is lowered from the owner instance's).
    pub fn closure_fn_id(&mut self, t: TyId) -> Option<u32> {
        if let TyKind::Closure(file, ce, _, _, owner) = self.tcx.tys.kind(t).clone() {
            if let TyKind::FnDef(d, args) = self.tcx.tys.kind(owner).clone() {
                let parent = self.fn_id(FnKey::Inst(d, args));
                return Some(self.fn_id(FnKey::Closure(file, ce, parent)));
            }
        }
        None
    }

    /// vtable of `src` as `dy` (built once): [drop, size, align, methods...]
    pub fn vtable(&mut self, src: TyId, dy: TyId) -> Result<u64, String> {
        if let Some(&a) = self.vtables.get(&(src, dy)) {
            return Ok(a);
        }
        let (td, targs) = match self.tcx.tys.kind(dy).clone() {
            TyKind::Dyn(td, targs, _) => (td, targs),
            _ => return Err("vtable for a non-dyn type".to_string()),
        };
        let l = self.lay.of(&mut self.tcx, src);
        let mut words: Vec<u64> = Vec::new();
        let drop = if self.needs_drop(src) {
            let id = self.fn_id(FnKey::Glue(GLUE_DROP, src));
            self.entry(id)
        } else {
            0
        };
        words.push(drop);
        words.push(crate::layout::round_up(l.size, l.align) as u64);
        words.push(l.align as u64);
        let is_fn = {
            let d = self.prog.def(td);
            Some(d.krate) == self.prog.prelude_crate && matches!(self.prog.name(td), "Fn" | "FnMut" | "FnOnce")
        };
        if is_fn {
            let captures = match self.tcx.tys.kind(src).clone() {
                TyKind::Closure(_, _, _, up, _) => !matches!(self.tcx.tys.kind(up), TyKind::Tuple(v) if v.is_empty()),
                _ => false,
            };
            let id = if captures {
                self.closure_fn_id(src).unwrap()
            } else {
                self.fn_id(FnKey::Glue(GLUE_CALL_SHIM, src))
            };
            words.push(self.entry(id));
        } else {
            let mut margs = vec![src];
            margs.extend(targs.iter().copied());
            for m in self.dyn_methods(td) {
                // supertrait methods take the supertrait's args; Self is what matters here
                let n = self.tcx.sigs.get(&m).map_or(0, |s| s.n_parent_generics) as usize;
                let mut a = margs.clone();
                a.truncate(n.max(1));
                while a.len() < n {
                    a.push(self.tcx.tys.error);
                }
                let (d, a) = self.tcx.resolve_trait_method(&self.prog, m, &a);
                let id = self.fn_id(FnKey::Inst(d, a));
                words.push(self.entry(id));
            }
        }
        let mut bytes = Vec::with_capacity(words.len() * 8);
        for w in &words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        let a = self.data(&bytes, 8);
        self.vtables.insert((src, dy), a);
        Ok(a)
    }

    /// Slot of a function instance (created with a lazy stub on first use).
    pub fn fn_id(&mut self, key: FnKey) -> u32 {
        if let Some(&id) = self.fn_map.get(&key) {
            return id;
        }
        let id = self.fns.len() as u32;
        if id as usize >= TABLE_CAP {
            panic!("function table full");
        }
        let name = fn_name(self, &key);
        let file = match &key {
            FnKey::Inst(d, _) => self.prog.def(*d).file,
            FnKey::Closure(f, _, _) => *f,
            FnKey::Glue(..) => 0,
        };
        let stub_addr = self.stub_base + id as u64 * STUB_SIZE as u64;
        let stub = x64::stub(id, self.lazy_entry);
        let slot_addr = self.table_base + id as u64 * 8;
        let thunk = x64::thunk(slot_addr, THUNK_SIZE);
        unsafe {
            write_code(stub_addr, &stub);
            write_code(self.thunk_base + id as u64 * THUNK_SIZE as u64, &thunk);
            *(slot_addr as *mut u64) = stub_addr;
        }
        self.fns.push(FnEntry { key: key.clone(), name, code: stub_addr, code_len: stub.len() as u32, pc_map: Vec::new(), file, prev: Vec::new(), compiled: false, patch_gen: 0, rir: None, building: false, inlined_into: Vec::new() });
        self.fn_map.insert(key, id);
        id
    }

    /// Permanent callable address of a slot (survives patching).
    pub fn entry(&self, id: u32) -> u64 {
        self.thunk_base + id as u64 * THUNK_SIZE as u64
    }

    pub fn find_fn(&self, pc: u64) -> Option<u32> {
        for (i, f) in self.fns.iter().enumerate() {
            if f.compiled && pc >= f.code && pc < f.code + f.code_len as u64 {
                return Some(i as u32);
            }
            for (c, l, _, _) in &f.prev {
                if pc >= *c && pc < *c + *l as u64 {
                    return Some(i as u32);
                }
            }
        }
        None
    }

    /// Foreign fns declared in Rapid core's `intrinsics*` modules become instructions.
    pub fn is_intrinsic(&self, d: DefId) -> bool {
        let def = self.prog.def(d);
        def.krate == self.core_crate && self.prog.name(def.parent).starts_with("intrinsics")
    }

    /// Loads every library the program's extern blocks name with `#[link(name = ..)]`
    /// (frameworks on macOS), once: what the system linker would have linked.
    fn load_link_libs(&self) {
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::SeqCst) {
            return;
        }
        for (fi, f) in self.prog.files.iter().enumerate() {
            for it in &f.ast.items {
                // active blocks (cfg) are the ones whose items got definitions
                let active = match &it.kind {
                    // an empty block (link-only) counts unless it has a cfg we cannot see here
                    crate::ast::ItemKind::ForeignMod(_, items) if items.is_empty() => !it.attrs.iter().any(|a| a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo).starts_with("cfg")),
                    crate::ast::ItemKind::ForeignMod(_, items) => items.iter().any(|si| self.prog.item_defs.contains_key(&(fi as u32, si.0))),
                    _ => false,
                };
                if !active {
                    continue;
                }
                for a in &it.attrs {
                    if a.toks.hi <= a.toks.lo || f.tok_text(a.toks.lo) != "link" {
                        continue;
                    }
                    let mut name = String::new();
                    let mut framework = false;
                    let mut k = a.toks.lo;
                    while k + 2 < a.toks.hi {
                        let key = f.tok_text(k);
                        if f.tok_text(k + 1) == "=" {
                            let v = f.tok_text(k + 2).trim_matches('"').to_string();
                            if key == "name" {
                                name = v;
                            } else if key == "kind" && v == "framework" {
                                framework = true;
                            }
                        }
                        k += 1;
                    }
                    if name.is_empty() {
                        continue;
                    }
                    let path = if framework {
                        format!("/System/Library/Frameworks/{}.framework/{}\0", name, name)
                    } else if cfg!(target_os = "macos") {
                        format!("lib{}.dylib\0", name)
                    } else {
                        format!("lib{}.so\0", name)
                    };
                    unsafe {
                        // RTLD_NOW | RTLD_GLOBAL
                        dlopen(path.as_ptr(), 2 | if cfg!(target_os = "macos") { 8 } else { 0x100 });
                    }
                }
            }
        }
    }

    /// Symbol name of a foreign item: `#[link_name = ".."]` or its own name.
    fn link_name(&self, d: DefId) -> String {
        let def = self.prog.def(d);
        let f = &self.prog.files[def.file as usize];
        for a in &f.ast.item(def.item).attrs {
            if a.toks.hi >= a.toks.lo + 3 && f.tok_text(a.toks.lo) == "link_name" && f.tok_text(a.toks.lo + 1) == "=" {
                return f.tok_text(a.toks.lo + 2).trim_matches('"').to_string();
            }
        }
        self.prog.name(d).to_string()
    }

    /// Address of a foreign fn or static (dlsym; loads the program's #[link] libraries
    /// on a miss).
    pub fn foreign_addr(&mut self, d: DefId) -> Option<u64> {
        let name = format!("{}\0", self.link_name(d));
        #[cfg(target_os = "macos")]
        let mut p = unsafe { os::find_symbol(name.as_ptr()) };
        #[cfg(target_os = "macos")]
        if p.is_null() {
            self.load_link_libs();
            p = unsafe { os::find_symbol(name.as_ptr()) };
        }
        #[cfg(target_os = "linux")]
        let mut p = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        #[cfg(target_os = "linux")]
        if p.is_null() {
            self.load_link_libs();
            p = unsafe { dlsym(std::ptr::null_mut(), name.as_ptr()) };
        }
        #[cfg(target_os = "linux")]
        if p.is_null() {
            // the C math library is not necessarily loaded into the host
            for lib in ["libm.so.6\0", "libc.so.6\0"] {
                let h = unsafe { dlopen(lib.as_ptr(), 2) };
                if !h.is_null() {
                    p = unsafe { dlsym(h, name.as_ptr()) };
                    if !p.is_null() {
                        break;
                    }
                }
            }
        }
        if !p.is_null() {
            return Some(p as u64);
        }
        if self.prog.def(d).kind == DefKind::ForeignFn {
            // a fn this host lacks (another platform's API behind a cfg the graph keeps):
            // calls compile, and panic with a one-line report only when one runs
            return Some(self.missing_foreign_stub(d));
        }
        None
    }

    /// Code that panics with "foreign function `x` is not available on this host" (one per
    /// symbol): loads its site into the first argument and tail-jumps to the panic
    /// trampoline, so the report names the JIT caller.
    fn missing_foreign_stub(&mut self, d: DefId) -> u64 {
        let name = self.link_name(d);
        if let Some(&a) = self.missing_stubs.get(&name) {
            return a;
        }
        let def = self.prog.def(d);
        let (file, pos) = (def.file, self.prog.files[def.file as usize].ast.item(def.item).lo);
        let site = self.site(file, pos, &format!("foreign function `{}` is not available on this host -- cfg the call out or link its library", name));
        let code = x64::jump_with_arg0(site, self.rt.panic_site);
        let a = self.emit_code(&code);
        self.missing_stubs.insert(name, a);
        a
    }

    /// Address of a constant's value (evaluated by running its initializer once).
    pub fn const_addr(&mut self, d: DefId, args: Vec<TyId>) -> Option<u64> {
        if let Some(&a) = self.consts.get(&(d, args.clone())) {
            return Some(a);
        }
        let a = self.eval_init(d, args.clone(), false)?;
        self.consts.insert((d, args), a);
        Some(a)
    }

    pub fn static_addr(&mut self, d: DefId) -> u64 {
        if let Some(&a) = self.statics.get(&d) {
            return a;
        }
        let a = self.eval_init(d, Vec::new(), true).unwrap_or(0);
        self.statics.insert(d, a);
        a
    }

    fn eval_init(&mut self, d: DefId, args: Vec<TyId>, mutable: bool) -> Option<u64> {
        let t = *self.tcx.const_tys.get(&d)?;
        let t = self.tcx.tys.subst(t, &args);
        let l = self.lay.of(&mut self.tcx, t);
        let buf = if mutable {
            self.rw_data(l.size.max(1) as u64, l.align.max(8) as u64)
        } else {
            self.data(&vec![0u8; l.size.max(1) as usize], l.align.max(8) as u64)
        };
        let id = self.fn_id(FnKey::Inst(d, args));
        let entry = self.entry(id);
        let r = unsafe { call_jit(self as *mut Unit, entry, buf) };
        match r {
            Ok(_) => Some(buf),
            Err(p) => {
                let m = format!("constant {} panicked: {}", self.prog.def_path(d), p.message);
                self.errors.push(m);
                None
            }
        }
    }

    /// Source position -> "file:line:col".
    pub fn pos_str(&self, file: u32, pos: u32) -> String {
        let (l, c) = crate::lexer::line_col(&self.prog.files[file as usize].src, pos);
        format!("{}:{}:{}", self.prog.file_paths[file as usize], l, c)
    }

    /// PC -> (fn slot, source position)
    pub fn pc_to_src(&self, pc: u64) -> Option<(u32, String)> {
        let id = self.find_fn(pc)?;
        let f = &self.fns[id as usize];
        let (base, map) = if pc >= f.code && pc < f.code + f.code_len as u64 {
            (f.code, &f.pc_map)
        } else {
            let mut found = (f.code, &f.pc_map);
            for (c, l, m, _) in &f.prev {
                if pc >= *c && pc < *c + *l as u64 {
                    found = (*c, m);
                }
            }
            found
        };
        let off = (pc - base) as u32;
        let mut pos = (f.file as u64) << 32;
        for (o, p) in map {
            if *o < off {
                pos = *p;
            } else {
                break;
            }
        }
        Some((id, self.pos_str((pos >> 32) as u32, pos as u32)))
    }

    /// Structured JSON crash report for a panic/fault/hang.
    pub fn report(&self, p: &PanicInfo) -> String {
        let mut s = String::from("{");
        s.push_str(&format!("\"kind\":{:?},\"message\":{:?}", p.kind, p.message));
        if (p.site as usize) < self.sites.len() {
            let st = &self.sites[p.site as usize];
            s.push_str(&format!(",\"location\":{:?}", self.pos_str(st.file, st.pos)));
        } else if !p.location.is_empty() {
            s.push_str(&format!(",\"location\":{:?}", p.location));
        }
        s.push_str(",\"backtrace\":[");
        for (i, f) in p.frames.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            match self.pc_to_src(f.wrapping_sub(if i == 0 { 0 } else { 1 })) {
                Some((id, loc)) => s.push_str(&format!("{{\"fn\":{:?},\"at\":{:?},\"patch_gen\":{}}}", self.fns[id as usize].name, loc, self.fns[id as usize].patch_gen)),
                None => s.push_str(&format!("{{\"pc\":\"{:#x}\"}}", f)),
            }
        }
        s.push_str("],\"recent_patches\":[");
        let n = self.patch_log.len();
        let from = n.saturating_sub(5);
        for (i, (slot, gen)) in self.patch_log[from..].iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&format!("{{\"fn\":{:?},\"gen\":{}}}", self.fns[*slot as usize].name, gen));
        }
        s.push_str("]}");
        s
    }

    /// Starts a patch group (one edit); its installs roll back together.
    pub fn begin_patch(&mut self) -> u32 {
        self.patch_gen += 1;
        self.patch_gen
    }

    /// Installs new code for a slot, keeping the previous version for rollback.
    /// `gen` > 0 marks the install as part of patch group `gen`.
    pub fn install(&mut self, id: u32, code: &[u8], pc_map: Vec<(u32, u64)>, gen: u32) -> u64 {
        let at = self.emit_code(code);
        let f = &mut self.fns[id as usize];
        if f.compiled {
            let old_map = std::mem::take(&mut f.pc_map);
            f.prev.push((f.code, f.code_len, old_map, f.patch_gen));
        }
        f.code = at;
        f.code_len = code.len() as u32;
        f.pc_map = pc_map;
        f.compiled = true;
        if gen > 0 {
            f.patch_gen = gen;
            self.patch_log.push((id, gen));
        }
        unsafe {
            let slot = (self.table_base + id as u64 * 8) as *const AtomicU64;
            (*slot).store(at, Ordering::SeqCst);
        }
        self.stats.code_bytes += code.len() as u64;
        at
    }

    /// Rolls back every slot installed by patch group `gen`. Their cached RIR is dropped,
    /// so later inlining recompiles them from source (the agent's next edit).
    pub fn rollback_gen(&mut self, gen: u32) -> Vec<u32> {
        let mut out = Vec::new();
        for id in 0..self.fns.len() {
            if self.fns[id].patch_gen != gen || gen == 0 {
                continue;
            }
            let f = &mut self.fns[id];
            let prev = match f.prev.pop() {
                Some(p) => p,
                None => continue,
            };
            f.code = prev.0;
            f.code_len = prev.1;
            f.pc_map = prev.2;
            f.patch_gen = prev.3;
            f.rir = None;
            unsafe {
                let slot = (self.table_base + id as u64 * 8) as *const AtomicU64;
                (*slot).store(prev.0, Ordering::SeqCst);
            }
            out.push(id as u32);
        }
        out
    }
}

/// Calls JIT code `entry(arg)` under a fresh recovery context. No Rust borrow of the
/// unit may be alive across this call; `u` becomes the runtime's unit pointer.
pub unsafe fn call_jit(u: *mut Unit, entry: u64, arg: u64) -> Result<u64, PanicInfo> {
    UNIT = u;
    let ctx = ctx_ptr();
    let saved_ctx = [*ctx, *ctx.add(1)];
    let enter: extern "C" fn(u64, u64, *mut u64) -> u64 = std::mem::transmute((&*u).enter as usize);
    let r = enter(entry, arg, ctx);
    *ctx = saved_ctx[0];
    *ctx.add(1) = saved_ctx[1];
    if r == 0 {
        Ok(0)
    } else {
        let p = PANIC.with(|p| p.borrow_mut().take());
        Err(p.unwrap_or(PanicInfo { kind: "unknown".to_string(), message: String::new(), site: u64::MAX, frames: Vec::new(), fault_addr: 0, location: String::new() }))
    }
}

/// Arms the hang watchdog for the calling thread: after `ms`, SIGUSR1 is sent to it
/// every 5 ms until a signal lands in JIT code, which then unwinds with a "hang" report.
/// No polling instructions in compiled code.
pub fn watchdog(ms: u64) -> u64 {
    let gen = RUN_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    WATCHDOG_HIT.store(false, Ordering::SeqCst);
    let target = unsafe { pthread_self() };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        while RUN_GEN.load(Ordering::SeqCst) == gen && !WATCHDOG_HIT.load(Ordering::SeqCst) {
            unsafe {
                pthread_kill(target, SIG_WATCHDOG);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
    gen
}

pub fn watchdog_done() {
    RUN_GEN.fetch_add(1, Ordering::SeqCst);
}

/// Compiles one slot (type check, lower, optimise, codegen) and installs it. Called
/// from the lazy stub on first call, and directly for eager compilation and patches.
pub unsafe fn compile_slot(up: *mut Unit, id: u32) -> u64 {
    lock_unit();
    let u = &mut *up;
    if u.fns[id as usize].compiled {
        let c = u.fns[id as usize].code;
        unlock_unit();
        return c;
    }
    // an internal compiler panic (front end or code generator) becomes this function's
    // compile error: the process never goes down for it (rt_compile is extern "C")
    let r = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_fn(up, id))) {
        Ok(r) => r,
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_string()
            };
            let u = &mut *up;
            u.fns[id as usize].building = false;
            Err(format!("internal compiler error in {}: {}{}", u.fns[id as usize].name, msg, take_ice_location()))
        }
    };
    match r {
        Ok((code, map)) => {
            let u = &mut *up;
            let a = u.install(id, &code, map, 0);
            unlock_unit();
            a
        }
        Err(e) => {
            let u = &mut *up;
            let name = u.fns[id as usize].name.clone();
            u.errors.push(format!("compile error in {}: {}", name, e));
            // a body that failed to compile panics when called
            let msg = format!("compile error in {}", name);
            let code = error_stub(u, &msg);
            let a = u.install(id, &code, Vec::new(), 0);
            unlock_unit();
            a
        }
    }
}

fn error_stub(u: &mut Unit, msg: &str) -> Vec<u8> {
    let mut f = crate::rir::Func::new("error".to_string(), 0);
    let b = f.block();
    let addr = u.data(msg.as_bytes(), 1);
    let p = f.vreg(crate::rir::Cls::I);
    let l = f.vreg(crate::rir::Cls::I);
    let z = f.vreg(crate::rir::Cls::I);
    let s = f.vreg(crate::rir::Cls::I);
    let blk = &mut f.blocks[b as usize];
    blk.insts.push(crate::rir::Inst::Addr(p, addr));
    blk.insts.push(crate::rir::Inst::Iconst(l, msg.len() as i64));
    blk.insts.push(crate::rir::Inst::Iconst(z, 0));
    blk.insts.push(crate::rir::Inst::Call(crate::rir::Callee::Host(u.rt.fmt_str), vec![p, l, z], Vec::new()));
    blk.insts.push(crate::rir::Inst::Iconst(s, u64::MAX as i64));
    blk.insts.push(crate::rir::Inst::Call(crate::rir::Callee::Host(u.rt.panic_site), vec![s], Vec::new()));
    for _ in 0..blk.insts.len() {
        blk.pos.push(0);
    }
    blk.term = crate::rir::Term::Unreachable;
    x64::compile(&f, &u.env).map(|c| c.code).unwrap_or_default()
}

/// Builds (or reuses) a slot's optimised RIR, then generates machine code.
pub unsafe fn compile_fn(up: *mut Unit, id: u32) -> Result<(Vec<u8>, Vec<(u32, u64)>), String> {
    build_rir(up, id)?;
    let u = &mut *up;
    let t0 = cpu_ns();
    let mut cg = (**u.fns[id as usize].rir.as_ref().unwrap()).clone();
    if u.opt.fold {
        crate::opt::addr_modes(&mut cg);
    }
    if u.opt.layout {
        crate::opt::rotate_loops(&mut cg);
    }
    let func = &cg;
    let c = x64::compile(func, &u.env)?;
    if let Ok(want) = std::env::var("RAPID_DUMP") {
        if u.fns[id as usize].name.ends_with(&want) {
            let base = format!("/tmp/rapid_dump_{}", want.replace("::", "_"));
            let _ = std::fs::write(format!("{}.bin", base), &c.code[..c.text_len as usize]);
            let _ = std::fs::write(format!("{}.rir", base), func.dump());
        }
    }
    u.stats.codegen_ns += cpu_ns() - t0;
    u.stats.compiled += 1;
    Ok((c.code, c.pc_map))
}

/// Source size of a slot's function in bytes (cheap pre-filter for inlining).
fn src_size(u: &Unit, id: u32) -> u32 {
    match &u.fns[id as usize].key {
        FnKey::Inst(d, _) => {
            let def = u.prog.def(*d);
            if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
                return u32::MAX;
            }
            let it = u.prog.files[def.file as usize].ast.item(def.item);
            it.hi - it.lo
        }
        FnKey::Closure(..) | FnKey::Glue(..) => u32::MAX,
    }
}

/// `#[inline(always)]` -> ATTR_ALWAYS, `#[inline]` -> ATTR_HINT.
fn inline_attrs(prog: &crate::program::Program, d: DefId) -> u8 {
    let def = prog.def(d);
    let f = &prog.files[def.file as usize];
    let it = f.ast.item(def.item);
    let mut r = 0;
    for a in &it.attrs {
        if a.toks.hi > a.toks.lo && f.tok_text(a.toks.lo) == "inline" {
            r |= crate::opt::ATTR_HINT;
            for i in a.toks.lo..a.toks.hi {
                if f.tok_text(i) == "always" {
                    r |= crate::opt::ATTR_ALWAYS;
                }
            }
        }
    }
    r
}

/// Source bytes below which a callee's RIR is built so it can be considered for inlining.
const INLINE_SRC_BYTES: u32 = 600;

/// Type-checks, lowers and optimises a slot's function into `fns[id].rir`.
/// Builds a function's RIR. An internal compiler panic becomes this function's compile error
/// (Rapid itself never goes down on bad input).
pub unsafe fn build_rir(up: *mut Unit, id: u32) -> Result<(), String> {
    // record where an internal panic happened (no stderr noise; the report carries it)
    static ICE_AT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        if let Some(l) = info.location() {
            if let Ok(mut g) = ICE_AT.lock() {
                *g = format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line());
            }
        }
    }));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_rir_unguarded(up, id)));
    std::panic::set_hook(prev);
    match r {
        Ok(r) => r,
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "panic".to_string()
            };
            let u = &mut *up;
            u.fns[id as usize].building = false;
            let at = ICE_AT.lock().map(|g| g.clone()).unwrap_or_default();
            Err(format!("internal compiler error in {} (rapid {}): {} -- Rapid bug, report it", u.fns[id as usize].name, at, msg))
        }
    }
}

unsafe fn build_rir_unguarded(up: *mut Unit, id: u32) -> Result<(), String> {
    {
        let u = &mut *up;
        if u.fns[id as usize].rir.is_some() {
            return Ok(());
        }
        if u.fns[id as usize].building {
            return Err("recursive build".to_string());
        }
        u.fns[id as usize].building = true;
    }
    let r = build_rir_inner(up, id);
    (&mut *up).fns[id as usize].building = false;
    let mut func = r?;
    let u = &mut *up;
    let cfg = u.opt;
    let mut nested_ns = 0u64;
    let t0 = cpu_ns();
    if cfg.inline {
        // callees small enough in source get their RIR built (bottom-up)
        let mut callees = Vec::new();
        for b in &func.blocks {
            for i in &b.insts {
                if let crate::rir::Inst::Call(crate::rir::Callee::Fn(c), _, _) = i {
                    if *c != id && !callees.contains(c) {
                        callees.push(*c);
                    }
                }
            }
        }
        for c in &callees {
            let u = &mut *up;
            // tier 0 inlines only builtins (functions of Rapid's core); user code is left
            // to tier 1 (profile-driven) unless the tier-1 preview flag is set
            let builtin = match &u.fns[*c as usize].key {
                FnKey::Inst(d, _) => u.prog.def(*d).krate == u.core_crate,
                FnKey::Closure(..) => false,
                FnKey::Glue(..) => true,
            };
            if !builtin && !cfg.inline_all {
                continue;
            }
            let attr = match &u.fns[*c as usize].key {
                FnKey::Inst(d, _) => inline_attrs(&u.prog, *d),
                FnKey::Closure(..) | FnKey::Glue(..) => 0,
            };
            if u.fns[*c as usize].rir.is_none() && !u.fns[*c as usize].building && (src_size(u, *c) <= INLINE_SRC_BYTES || attr & crate::opt::ATTR_ALWAYS != 0) {
                let saved = u.cur_fn;
                let tn = cpu_ns();
                let _ = build_rir(up, *c);
                nested_ns += cpu_ns() - tn;
                (&mut *up).cur_fn = saved;
            }
        }
        let u = &mut *up;
        let fns_ptr: *const Vec<FnEntry> = &u.fns;
        let core_crate = u.core_crate;
        let prog_ptr: *const crate::program::Program = &u.prog;
        let inline_all = cfg.inline_all;
        let get = |c: u32| -> Option<(*const crate::rir::Func, u8)> {
            let fns = &*fns_ptr;
            let prog = &*prog_ptr;
            let attrs = match &fns[c as usize].key {
                FnKey::Inst(d, _) => {
                    if !inline_all && prog.def(*d).krate != core_crate {
                        return None;
                    }
                    inline_attrs(prog, *d)
                }
                FnKey::Glue(..) => 0,
                FnKey::Closure(..) => return None,
            };
            match &fns[c as usize].rir {
                Some(f) => Some((&**f as *const crate::rir::Func, attrs)),
                None => None,
            }
        };
        let ti = crate::opt::pass_clock();
        let inlined = crate::opt::inline_calls(&mut func, id, &get);
        crate::opt::pass_time(5, ti);
        for (c, rule) in inlined {
            u.stats.inline_rules[rule] += 1;
            if !u.fns[c as usize].inlined_into.contains(&id) {
                u.fns[c as usize].inlined_into.push(id);
            }
        }
    }
    let u = &mut *up;
    if cfg.fold {
        crate::opt::simplify(&mut func, u.ro_range(), cfg.imm, cfg.cse);
        if cfg.sroa && crate::opt::sroa(&mut func) {
            crate::opt::simplify(&mut func, u.ro_range(), cfg.imm, cfg.cse);
        }
        if cfg.licm && crate::opt::licm(&mut func) {
            crate::opt::simplify(&mut func, u.ro_range(), cfg.imm, cfg.cse);
        }
    }
    let ts = crate::opt::pass_clock();
    if cfg.fold {
        crate::opt::sink_consts(&mut func);
    }
    crate::opt::pass_time(6, ts);
    let ts = crate::opt::pass_clock();
    if cfg.layout {
        crate::opt::relayout(&mut func);
    }
    crate::opt::pass_time(7, ts);
    if let Err(e) = func.verify() {
        return Err(format!("RIR verify after opt: {}\n{}", e, func.dump()));
    }
    u.stats.opt_ns += (cpu_ns() - t0).saturating_sub(nested_ns);
    u.stats.rir_insts += crate::opt::inst_count(&func) as u64;
    u.fns[id as usize].rir = Some(Box::new(func));
    Ok(())
}

/// RAPID_TRACE=typeck:<fn substring>,lower:<fn substring>: print each matching phase start
/// (a debugger-free way to see where the front end is).
fn trace(phase: &str, name: &str) {
    static SPEC: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        let mut v = Vec::new();
        if let Ok(s) = std::env::var("RAPID_TRACE") {
            for part in s.split(',') {
                if let Some((p, f)) = part.split_once(':') {
                    v.push((p.to_string(), f.to_string()));
                }
            }
        }
        v
    });
    for (p, f) in spec {
        if p == phase && name.contains(f.as_str()) {
            eprintln!("trace: {} {}", phase, name);
        }
    }
}

unsafe fn build_rir_inner(up: *mut Unit, id: u32) -> Result<crate::rir::Func, String> {
    let key = (&*up).fns[id as usize].key.clone();
    let fname = (&*up).fns[id as usize].name.clone();
    trace("typeck", &fname);
    if let FnKey::Glue(kind, t) = key {
        let u = &mut *up;
        let t0 = cpu_ns();
        let f = crate::lower::glue_func(u, kind, t);
        u.stats.lower_ns += cpu_ns() - t0;
        return Ok(f);
    }
    let t0 = cpu_ns();
    let body = match &key {
        FnKey::Inst(d, args) => {
            let u = &mut *up;
            crate::typeck::check_fn(&u.prog, &mut u.tcx, *d, args)
        }
        FnKey::Closure(_, _, parent) => {
            let u = &mut *up;
            match u.bodies.get(parent) {
                Some(b) => clone_body(b),
                None => return Err("closure parent body missing".to_string()),
            }
        }
        FnKey::Glue(..) => return Err("glue".to_string()),
    };
    if !body.errors.is_empty() {
        return Err(body.errors[0].clone());
    }
    let t1 = cpu_ns();
    // evaluate constants used by the body before lowering (may run JIT code)
    let mut const_uses = Vec::new();
    for (k, r) in &body.res {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst | DefKind::Static) {
                let args = body.res_args.get(k).cloned().unwrap_or_default();
                const_uses.push((*d, args, kind));
            }
        }
    }
    for r in body.pat_res.values() {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst) {
                const_uses.push((*d, Vec::new(), kind));
            }
        }
    }
    for r in body.fmt_named.values() {
        if let Res::Def(d) = r {
            let kind = (&*up).prog.def(*d).kind;
            if matches!(kind, DefKind::Const | DefKind::AssocConst) {
                const_uses.push((*d, Vec::new(), kind));
            }
        }
    }
    for (d, args, kind) in const_uses {
        // a const used inside its own initializer is an error rustc already reports
        if let FnKey::Inst(me, _) = &key {
            if *me == d {
                continue;
            }
        }
        if kind == DefKind::Static {
            (&mut *up).static_addr(d);
        } else {
            (&mut *up).const_addr(d, args);
        }
    }
    let u = &mut *up;
    u.cur_fn = id;
    let name = u.fns[id as usize].name.clone();
    trace("lower", &name);
    let (func, errors, closures) = {
        let body_ref: &Body = &*(&body as *const Body);
        let mut lcx = Lcx::new(u, body_ref, name);
        match &key {
            FnKey::Inst(d, _) => lcx.lower_fn_body(*d),
            FnKey::Closure(_, ce, _) => lcx.lower_closure_body(*ce),
            FnKey::Glue(..) => {}
        }
        lcx.finish();
        let e = std::mem::take(&mut lcx.errors);
        let c = std::mem::take(&mut lcx.new_closures);
        (lcx.f, e, c)
    };
    if !errors.is_empty() {
        return Err(errors[0].clone());
    }
    if let Err(e) = func.verify() {
        return Err(format!("RIR verify: {}\n{}", e, func.dump()));
    }
    let u = &mut *up;
    u.stats.typeck_ns += t1 - t0;
    u.stats.lower_ns += cpu_ns() - t1;
    // closures are lowered from their parent's body (looked up by the parent slot)
    if !closures.is_empty() {
        u.bodies.insert(id, clone_body(&body));
    }
    Ok(func)
}

fn clone_body(b: &Body) -> Body {
    Body {
        def: b.def,
        args: b.args.clone(),
        file: b.file,
        expr_lo: b.expr_lo,
        expr_ty: b.expr_ty.clone(),
        pat_lo: b.pat_lo,
        pat_ty: b.pat_ty.clone(),
        res: b.res.clone(),
        res_args: b.res_args.clone(),
        pat_res: b.pat_res.clone(),
        pat_local: b.pat_local.clone(),
        methods: b.methods.clone(),
        binops: b.binops.clone(),
        coerce: b.coerce.clone(),
        field_idx: b.field_idx.clone(),
        ov_derefs: b.ov_derefs.clone(),
        ov_index: b.ov_index.clone(),
        for_next: b.for_next.clone(),
        for_into: b.for_into.clone(),
        index_derefs: b.index_derefs.clone(),
        call_derefs: b.call_derefs.clone(),
        try_conv: b.try_conv.clone(),
        pat_derefs: b.pat_derefs.clone(),
        pat_bind_ref: b.pat_bind_ref.clone(),
        locals: {
            let mut v = Vec::new();
            for l in &b.locals {
                v.push(crate::typeck::Local { name: l.name, ty: l.ty, mutable: l.mutable, pat: l.pat });
            }
            v
        },
        param_pats: b.param_pats.clone(),
        ret: b.ret,
        body: b.body,
        fmt_named: b.fmt_named.clone(),
        closures: b.closures.clone(),
        closure_locals: b.closure_locals.clone(),
        errors: Vec::new(),
    }
}

/// Finds `#[test]` functions of a crate.
pub fn find_tests(prog: &Program, krate: u32) -> Vec<DefId> {
    let mut out = Vec::new();
    for (i, d) in prog.defs.iter().enumerate() {
        if d.krate != krate || d.kind != DefKind::Fn {
            continue;
        }
        let it = prog.files[d.file as usize].ast.item(d.item);
        let mut is_test = false;
        let mut ignored = false;
        for a in &it.attrs {
            if a.toks.hi > a.toks.lo {
                let n = prog.files[d.file as usize].tok_text(a.toks.lo);
                if n == "test" && a.toks.hi == a.toks.lo + 1 {
                    is_test = true;
                }
                if n == "ignore" {
                    ignored = true;
                }
            }
        }
        if is_test && !ignored {
            out.push(DefId(i as u32));
        }
    }
    out
}
