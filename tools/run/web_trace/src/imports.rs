//! The page's `env` imports: every `extern "C"` function the web build of the program declares
//! with `#[link(wasm_import_module = "env")]`, with the semantics web.js / web_worker.js /
//! wasm_bridge.js give it. Relax binds the program's imports to these through
//! `relax_compiler::web::set_embedder_symbols` (`symbol`).
//!
//! An import runs inside a call of the program (on the UI thread or a worker): it never touches
//! the page itself (the page is in the middle of that call). What it starts goes to the page's
//! queue (`Queue`), which the page works through when the call returns, as the browser runs a
//! fetch, a worker start or a microtask after the current task.
//!
//! Pointer arguments: a `u32` is a wasm address (`mem`); a `*const u8` / `usize` pair is the
//! program's own pointer and length (js_spawn_thread's name: the declaration says so).
//!
//! | import | semantics here |
//! |---|---|
//! | js_console_log(ptr, len) | UTF-8 text to stdout, prefixed `[page]` |
//! | js_console_error(ptr, len) | to stderr; `__MAKEPAD_WASM_PANIC__:` marks a panic (recorded) |
//! | js_time_now() -> f64 | `Date.now() / 1000`: the virtual clock plus a fixed epoch |
//! | js_monotonic_now() -> f64 | `performance.now() / 1000`: the virtual clock |
//! | js_wake_ui() | queues a signal delivery (microtask after the current call) |
//! | js_worker_wait(ms) | a worker sleeps until the virtual clock passes now + ms (Atomics.wait on a word nobody notifies); on the UI thread: an error, as web.js throws |
//! | js_spawn_thread(id, ctx, stack, name, len) -> u32 | queues a worker start (create_thread); 1 |
//! | js_network_http_request(..14) | queues a fetch of the URL (page.rs `fetch`) |
//! | js_network_http_cancel(lo, hi) | drops a queued fetch |
//! | js_network_ws_open / send / close | no network: the socket reports an error and closes |
//! | js_open_web_socket / js_web_socket_send_* | the legacy socket: closed at once |

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

/// The program's linear memory: wasm address -> pointer.
#[inline]
pub fn mem(addr: u32) -> *mut u8 {
    (relax_compiler::web::BASE as usize | addr as usize) as *mut u8
}

/// `len` bytes at wasm address `addr`, copied.
pub fn read_bytes(addr: u32, len: u32) -> Vec<u8> {
    if len == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(mem(addr), len as usize) }.to_vec()
}

pub fn read_str(addr: u32, len: u32) -> String {
    String::from_utf8_lossy(&read_bytes(addr, len)).into_owned()
}

// ---- the virtual clock: seconds since the page started (performance.now() / 1000)

static CLOCK_BITS: AtomicU64 = AtomicU64::new(0);
/// Date.now() at the page's start: 2026-01-01T00:00:00Z, fixed so runs repeat.
pub const EPOCH_S: f64 = 1_767_225_600.0;

pub fn now() -> f64 {
    f64::from_bits(CLOCK_BITS.load(Ordering::Acquire))
}

/// Advances the clock (the page only, between tasks) and wakes sleeping workers.
pub fn set_now(t: f64) {
    CLOCK_BITS.store(t.to_bits(), Ordering::Release);
    let _g = WORKERS.lock().unwrap();
    WORKER_CLOCK.notify_all();
}

// ---- what imports start, for the page to do after the current call

pub enum Queued {
    WakeUi,
    SpawnThread { request_id: u32, context_ptr: u32, stack_size: u32, name: String },
    /// a request's body is the program's; local files answer GET and HEAD whatever it is
    Fetch { request: [u32; 4], url: String, method: String, headers: String, max_body: u64 },
    CancelFetch,
    WsOpen { socket: [u32; 2], url: String },
    LegacySocketOpen { id: u32 },
    /// a worker said `started` / `finished` (web_worker.js postMessage)
    WorkerStarted(u32),
    WorkerFinished(u32),
}

pub static QUEUE: Mutex<Vec<Queued>> = Mutex::new(Vec::new());

fn queue(q: Queued) {
    QUEUE.lock().unwrap().push(q);
}

// ---- workers: which are running and which sleep in js_worker_wait

pub struct WorkerState {
    /// request id -> (sleeping until (virtual s), or running)
    pub sleeping: std::collections::BTreeMap<u32, Option<f64>>,
    /// request id -> the worker's OS thread (a mach port on macOS, a tid on Linux): whether it
    /// runs or waits (a lock, a channel, a condition) is the scheduler's to say.
    pub native: std::collections::BTreeMap<u32, u64>,
}

/// The calling thread, as [`blocked`] asks about it.
pub fn native_thread() -> u64 {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_self() -> usize;
            fn pthread_mach_thread_np(thread: usize) -> u32;
        }
        unsafe { pthread_mach_thread_np(pthread_self()) as u64 }
    }
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn gettid() -> i32;
        }
        unsafe { gettid() as u64 }
    }
}

/// Whether the OS thread waits (blocked on a lock, a condition, a channel): it cannot move
/// until something else does. False when it runs or cannot be asked.
pub fn blocked(thread: u64) -> bool {
    #[cfg(target_os = "macos")]
    {
        // thread_basic_info: user and system time (2 x 2 i32), cpu_usage, policy, run_state,
        // flags, suspend_count, sleep_time; TH_STATE_WAITING = 3
        extern "C" {
            fn thread_info(thread: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        }
        let mut info = [0i32; 10];
        let mut count = 10u32;
        let r = unsafe { thread_info(thread as u32, 3, info.as_mut_ptr(), &mut count) };
        r == 0 && info[6] == 3
    }
    #[cfg(target_os = "linux")]
    {
        // /proc/self/task/<tid>/stat: `pid (comm) S ...`, S (sleeping) or D (disk)
        let Ok(stat) = std::fs::read_to_string(format!("/proc/self/task/{}/stat", thread)) else { return false };
        let state = stat.rsplit_once(')').and_then(|(_, rest)| rest.trim_start().chars().next());
        matches!(state, Some('S') | Some('D'))
    }
}
pub static WORKERS: Mutex<WorkerState> = Mutex::new(WorkerState { sleeping: std::collections::BTreeMap::new(), native: std::collections::BTreeMap::new() });
pub static WORKER_CLOCK: Condvar = Condvar::new();

std::thread_local! {
    /// The worker this thread runs (its request id), None on the UI thread.
    pub static WORKER: std::cell::Cell<Option<u32>> = const { std::cell::Cell::new(None) };
}

// ---- use counts: every import the program called, how often

pub const NAMES: [&str; 16] = [
    "js_console_log", "js_console_error", "js_time_now", "js_monotonic_now", "js_wake_ui", "js_worker_wait",
    "js_spawn_thread", "js_network_http_request", "js_network_http_cancel", "js_network_ws_open",
    "js_network_ws_send_binary", "js_network_ws_send_text", "js_network_ws_close", "js_open_web_socket",
    "js_web_socket_send_string", "js_web_socket_send_binary",
];
pub static CALLS: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
/// Panics the program reported (js_console_error's panic marker).
pub static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// `--until <text>`: the run ends at the first console line holding it.
pub static UNTIL: Mutex<Option<String>> = Mutex::new(None);
pub static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Draw shaders a run with a shader pack installed did not find in it (the platform's
/// `not in pack: draw shader '<name>' (<where>, key <hex>)` line): stub hits of that build.
pub static PACK_MISSES: Mutex<std::collections::BTreeSet<String>> = Mutex::new(std::collections::BTreeSet::new());

fn watch(line: &str) {
    if let Some(at) = line.find("not in pack: draw shader ") {
        let rest = &line[at + "not in pack: draw shader ".len()..];
        let shader = rest.split_once(';').map_or(rest, |(s, _)| s).trim();
        PACK_MISSES.lock().unwrap().insert(shader.to_string());
    }
    if UNTIL.lock().unwrap().as_deref().is_some_and(|until| line.contains(until)) {
        STOP.store(true, Ordering::Relaxed);
    }
}

fn count(i: usize) {
    CALLS[i].fetch_add(1, Ordering::Relaxed);
}

extern "C" fn js_console_log(ptr: u32, len: u32) {
    count(0);
    let line = read_str(ptr, len);
    println!("[page] {}", line);
    watch(&line);
}

extern "C" fn js_console_error(ptr: u32, len: u32) {
    count(1);
    let raw = read_str(ptr, len);
    watch(&raw);
    match raw.strip_prefix("__MAKEPAD_WASM_PANIC__:") {
        Some(p) => {
            eprintln!("[page] PANIC {}", p);
            PANICS.lock().unwrap().push(p.to_string());
        }
        None => eprintln!("[page] {}", raw),
    }
}

extern "C" fn js_time_now() -> f64 {
    count(2);
    EPOCH_S + now()
}

extern "C" fn js_monotonic_now() -> f64 {
    count(3);
    now()
}

extern "C" fn js_wake_ui() {
    count(4);
    queue(Queued::WakeUi);
}

extern "C" fn js_worker_wait(timeout_ms: f64) {
    count(5);
    let Some(id) = WORKER.with(|w| w.get()) else {
        eprintln!("[page] js_worker_wait may only run in a Web Worker");
        return;
    };
    let until = now() + timeout_ms.max(0.0) / 1000.0;
    let mut g = WORKERS.lock().unwrap();
    g.sleeping.insert(id, Some(until));
    WORKER_CLOCK.notify_all();
    while now() < until {
        g = WORKER_CLOCK.wait(g).unwrap();
    }
    g.sleeping.insert(id, None);
}

extern "C" fn js_spawn_thread(request_id: u32, context_ptr: u32, stack_size: u32, name_ptr: *const u8, name_len: usize) -> u32 {
    count(6);
    let name = if name_ptr.is_null() || name_len == 0 {
        String::new()
    } else {
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(name_ptr, name_len) }).into_owned()
    };
    queue(Queued::SpawnThread { request_id, context_ptr, stack_size, name });
    1
}

#[allow(clippy::too_many_arguments)]
extern "C" fn js_network_http_request(
    request_id_lo: u32, request_id_hi: u32, metadata_id_lo: u32, metadata_id_hi: u32,
    url_ptr: u32, url_len: u32, method_ptr: u32, method_len: u32, headers_ptr: u32, headers_len: u32,
    _body_ptr: u32, _body_len: u32, max_body_lo: u32, max_body_hi: u32,
) {
    count(7);
    queue(Queued::Fetch {
        request: [request_id_lo, request_id_hi, metadata_id_lo, metadata_id_hi],
        url: read_str(url_ptr, url_len),
        method: read_str(method_ptr, method_len),
        headers: read_str(headers_ptr, headers_len),
        max_body: max_body_lo as u64 | (max_body_hi as u64) << 32,
    });
}

extern "C" fn js_network_http_cancel(_lo: u32, _hi: u32) {
    count(8);
    // every fetch completes in the task after its request: nothing is ever still pending here
    queue(Queued::CancelFetch);
}

extern "C" fn js_network_ws_open(lo: u32, hi: u32, url_ptr: u32, url_len: u32, _headers_ptr: u32, _headers_len: u32) {
    count(9);
    queue(Queued::WsOpen { socket: [lo, hi], url: read_str(url_ptr, url_len) });
}

extern "C" fn js_network_ws_send_binary(_lo: u32, _hi: u32, _ptr: u32, _len: u32) {
    count(10);
}

extern "C" fn js_network_ws_send_text(_lo: u32, _hi: u32, _ptr: u32, _len: u32) {
    count(11);
}

extern "C" fn js_network_ws_close(_lo: u32, _hi: u32) {
    count(12);
}

extern "C" fn js_open_web_socket(id: u32, _url_ptr: u32, _url_len: u32) {
    count(13);
    queue(Queued::LegacySocketOpen { id });
}

extern "C" fn js_web_socket_send_string(_id: u32, _ptr: u32, _len: u32) {
    count(14);
}

extern "C" fn js_web_socket_send_binary(_id: u32, _ptr: u32, _len: u32) {
    count(15);
}

/// The import a symbol of the program binds to (relax_compiler::web::set_embedder_symbols).
pub fn symbol(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"js_console_log" => js_console_log as *const () as usize,
        b"js_console_error" => js_console_error as *const () as usize,
        b"js_time_now" => js_time_now as *const () as usize,
        b"js_monotonic_now" => js_monotonic_now as *const () as usize,
        b"js_wake_ui" => js_wake_ui as *const () as usize,
        b"js_worker_wait" => js_worker_wait as *const () as usize,
        b"js_spawn_thread" => js_spawn_thread as *const () as usize,
        b"js_network_http_request" => js_network_http_request as *const () as usize,
        b"js_network_http_cancel" => js_network_http_cancel as *const () as usize,
        b"js_network_ws_open" => js_network_ws_open as *const () as usize,
        b"js_network_ws_send_binary" => js_network_ws_send_binary as *const () as usize,
        b"js_network_ws_send_text" => js_network_ws_send_text as *const () as usize,
        b"js_network_ws_close" => js_network_ws_close as *const () as usize,
        b"js_open_web_socket" => js_open_web_socket as *const () as usize,
        b"js_web_socket_send_string" => js_web_socket_send_string as *const () as usize,
        b"js_web_socket_send_binary" => js_web_socket_send_binary as *const () as usize,
        _ => return None,
    })
}
