//! Relax-only: run-time modules (local/plans/hotreload.md §11). A program running under Relax
//! (`cfg(relax)`) loads a Rust source file as a module, takes typed fn pointers to its
//! exports, edits it live and unloads it. A failed load or edit changes nothing and returns
//! the compiler's one-line errors.
//!
//! ```ignore
//! let m = std::relax::Module::load("fx_rings", "effects/rings.rs", &["vj_fx"])?;
//! let frame: fn(&mut FxState, f32) = m.get("frame")?;
//! ```

use crate::string::String;

// These entries only exchange scalars; every input is valid. No pointers or
// caller-owned buffers cross the loader boundary.
extern "C" {
    #[link_name = "__relax_live_edit_enabled"]
    pub safe fn live_edit_enabled() -> bool;
    /// Registers the wake for the program's event loop, called after each installed
    /// live patch (from the watcher thread).
    #[link_name = "__relax_on_patch"]
    pub safe fn on_patch(wake: extern "C" fn());
}

extern "C" {
    fn __relax_realtime(root: usize, data: usize, meta: usize);
}

/// The type whose code `realtime` names (its address is the root's identity, not a call).
fn __realtime_root<T: ?Sized>(_: &T) {}

/// The code reachable from `x` runs on a real-time thread (an audio device's callback, a
/// thread with a real-time policy): it must never wait for a compile. Under Relax everything
/// it can reach -- the closure or fn's body, what that calls, the methods of the trait objects
/// it calls through, now and as new ones are made -- is compiled now, before the first call.
/// Pass the closure or fn pointer itself, or `&*boxed` for a trait object. Call it where the
/// callback is installed, before the device or thread starts.
pub fn realtime<T: ?Sized>(x: &T) {
    let p: *const T = x;
    // a trait object's (or slice's) metadata is the word after the data pointer
    let meta = if crate::mem::size_of::<*const T>() == 2 * crate::mem::size_of::<usize>() {
        unsafe { *(&p as *const *const T as *const usize).add(1) }
    } else {
        0
    };
    let root = __realtime_root::<T> as fn(&T) as usize;
    unsafe { __relax_realtime(root, p as *const u8 as usize, meta) }
}

extern "C" {
    fn __relax_budget_enter(module: u64, token: u64) -> u32;
    fn __relax_budget_leave();
    fn __relax_budget_new(fuel: u64, growth: u64) -> u64;
    fn __relax_budget_drop(token: u64);
    fn __relax_budget_read(token: u64, out: *mut [u64; 4]);
    fn __relax_budget_reset(token: u64, fuel: u64, growth: u64);
    fn __relax_host_new(module: u64, ptr: u64, ty: *const u8, len: usize) -> u64;
    fn __relax_host_drop(id: u64);
    fn __relax_host_acquire(id: u64, ty: *const u8, len: usize) -> u64;
    fn __relax_host_release(id: u64);
    fn __relax_sb_string_copy(p: u64, len: u64, out: *mut u8) -> u32;
    fn __relax_module_load(name: *const u8, nlen: usize, path: *const u8, plen: usize, deps: *const u8, dlen: usize) -> u64;
    fn __relax_module_load_src(name: *const u8, nlen: usize, path: *const u8, plen: usize, src: *const u8, slen: usize, deps: *const u8, dlen: usize) -> u64;
    fn __relax_module_load_stream(name: *const u8, nlen: usize, path: *const u8, plen: usize, src: *const u8, slen: usize, deps: *const u8, dlen: usize) -> u64;
    fn __relax_module_stream(h: u64, src: *const u8, slen: usize, kept: *mut usize) -> u32;
    fn __relax_module_get(h: u64, name: *const u8, nlen: usize, ty: *const u8, tlen: usize) -> u64;
    fn __relax_module_replace(h: u64, src: *const u8, slen: usize) -> u32;
    fn __relax_module_unload(h: u64);
    fn __relax_module_error(buf: *mut u8, cap: usize) -> usize;
    fn __relax_guard_enter(deadline_ms: u64) -> u64;
    fn __relax_guard_leave(token: u64);
    fn __relax_sb_enter(module: u32, out: *mut [i64; 4]) -> u32;
    fn __relax_sb_leave(prev: *const [i64; 4]);
    fn __relax_sb_alloca(size: usize, align: usize) -> usize;
    fn __relax_sb_stats(out: *mut [u64; 6], recycle: u32) -> u32;
    fn __relax_domain_id() -> u32;
    fn __relax_restart_supported();
    fn __relax_restart_requested() -> u32;
    fn __relax_restart_reason(buf: *mut u8, cap: usize) -> usize;
    fn __relax_restored_state(buf: *mut u8, cap: usize) -> usize;
    fn __relax_restart_now(state: *const u8, len: usize);
    fn __relax_live_stream(path: *const u8, plen: usize, text: *const u8, len: usize) -> u32;
    fn __relax_live_log(line: *const u8, len: usize);
    fn relax_store_file(u: *const u8, un: usize, c: *const u8, cn: usize, p: *const u8, pn: usize, out: *mut *const u8, out_len: *mut usize) -> u64;
    fn relax_vfs_release(id: u64);
}

/// One file of a git repository in Relax's stores (`url` as a run spec names it, the 40-hex
/// `commit`, `path` inside the repository): None when the store does not keep that commit
/// (e.g. a release downloaded but not run yet) or has no such file.
pub fn store_file(url: &str, commit: &str, path: &str) -> Option<crate::vec::Vec<u8>> {
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_store_file(url.as_ptr(), url.len(), commit.as_ptr(), commit.len(), path.as_ptr(), path.len(), &mut p, &mut n) };
    if id == 0 {
        return None;
    }
    let v = unsafe { core::slice::from_raw_parts(p, n) }.to_vec();
    unsafe { relax_vfs_release(id) };
    Some(v)
}

/// One file of a git repository in Relax's stores (`url` as a run spec names it, the 40-hex
/// `commit`, `path` inside the repository): None when the store does not keep that commit
/// (e.g. a release downloaded but not run yet) or has no such file.
pub fn store_file(url: &str, commit: &str, path: &str) -> Option<crate::vec::Vec<u8>> {
    let (mut p, mut n) = (core::ptr::null(), 0usize);
    let id = unsafe { relax_store_file(url.as_ptr(), url.len(), commit.as_ptr(), commit.len(), path.as_ptr(), path.len(), &mut p, &mut n) };
    if id == 0 {
        return None;
    }
    let v = unsafe { core::slice::from_raw_parts(p, n) }.to_vec();
    unsafe { relax_vfs_release(id) };
    Some(v)
}

/// Reports progress of a long wait (a download): `done` of `total` (None: unknown) for `label`,
/// e.g. "CEF 132 MB". The launcher (the runner) shows it as its progress bar; in a terminal it
/// is a short line. A build script's reports reach the launcher through the Relax running it.
/// Throttled inside: call it as often as convenient.
pub fn progress(label: &str, done: u64, total: Option<u64>) {
    unsafe { __relax_progress(label.as_ptr(), label.len(), done, total.unwrap_or(u64::MAX)) }
}

/// HTTPS GET of `url` into the file `to` through Relax's own HTTP client (the system's TLS
/// stack; no process is started): written to `<to>.part` and renamed when complete, redirects
/// followed, progress reported as `label`. Returns the byte count.
pub fn download(url: &str, to: &crate::path::Path, label: &str) -> Result<u64, String> {
    let to = to.to_string_lossy();
    let mut err = crate::vec::Vec::with_capacity(1024);
    let mut n = 0usize;
    let r = unsafe { __relax_download(url.as_ptr(), url.len(), to.as_ptr(), to.len(), label.as_ptr(), label.len(), err.as_mut_ptr(), err.capacity(), &mut n) };
    if r >= 0 {
        return Ok(r as u64);
    }
    unsafe { err.set_len(n) };
    Err(String::from_utf8_lossy(&err).into_owned())
}

/// The folder for downloads any later run may reuse (a CEF distribution): `downloads/` in
/// Relax's cache folder. None without a home folder.
pub fn download_dir() -> Option<crate::path::PathBuf> {
    let mut buf = crate::vec::Vec::with_capacity(4096);
    let n = unsafe { __relax_download_dir(buf.as_mut_ptr(), buf.capacity()) };
    if n == 0 || n > buf.capacity() {
        return None;
    }
    unsafe { buf.set_len(n) };
    Some(crate::path::PathBuf::from(String::from_utf8_lossy(&buf).into_owned()))
}

/// Streams source into a loaded file. Returns 0 for applied/partial, 1 for incomplete
/// with nothing applied, 2 for needs restart, or 3 for refused. Relax's live watcher drains
/// coalesced prefixes and reports errors after a one-second stall.
pub fn live_stream(path: &str, text: &str) -> u32 {
    unsafe { __relax_live_stream(path.as_ptr(), path.len(), text.as_ptr(), text.len()) }
}

/// Struct layouts edited live in transactions newer than `since_generation`: one line
/// "<generation>\t<type path>" per struct (the type path as `core::any::type_name` spells it
/// under Relax). Empty when nothing changed. A layout edit also bumps `style_generation`.


/// Adds one JSON line to the live session's log (the scoreboard: the restart's kept/lost report).
pub fn live_log(json_line: &str) {
    unsafe { __relax_live_log(json_line.as_ptr(), json_line.len()) }
}

/// After a restart: when the edit that caused it was saved (unix ms), for save -> back on screen.
pub fn restart_saved_ms() -> Option<f64> {
    crate::env::var("MAKEPAD_RUN_RESTART_T0").ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| *v > 0.0)
}

// ---- state-preserving restart (live sessions: an edit Relax cannot patch in place)

/// The id of the app domain the calling thread runs in: 0 is the process's own app, a hosted
/// app (started by a window manager) has its own id. Process-wide resources (signal handlers) are
/// the process's app's; a hosted app's background workers must end with it (domain teardown
/// waits for its threads and never terminates one).
pub fn domain() -> u32 {
    unsafe { __relax_domain_id() }
}

/// The program (its app framework) can restart with its state: from now on an edit that needs a
/// restart asks for one through `restart_requested` instead of only being reported.
pub fn restart_supported() {
    unsafe { __relax_restart_supported() }
}

/// A restart is due: at the next safe point, snapshot what the user sees and call `restart_now`
/// (or postpone it, e.g. while there is unsaved work). Cheap to poll.
pub fn restart_requested() -> bool {
    unsafe { __relax_restart_requested() != 0 }
}

/// Why the restart is due (the edit and the reason it could not apply in place).
pub fn restart_reason() -> String {
    unsafe {
        let n = __relax_restart_reason(core::ptr::null_mut(), 0);
        let mut v: crate::vec::Vec<u8> = crate::vec::Vec::with_capacity(n);
        let m = __relax_restart_reason(v.as_mut_ptr(), n);
        v.set_len(m.min(n));
        String::from_utf8_lossy(&v).into_owned()
    }
}

/// Restarts the program with `state`, which its fresh run reads with `restored_state`.
/// A hosted app queues a domain relaunch and returns so its event loop can finish shutdown;
/// a directly running program replaces its process and returns only on failure.
pub fn restart_now(state: &str) {
    unsafe { __relax_restart_now(state.as_ptr(), state.len()) }
}

/// The state the previous run passed to `restart_now`, if this run is such a restart.
pub fn restored_state() -> Option<String> {
    unsafe {
        let n = __relax_restored_state(core::ptr::null_mut(), 0);
        if n == usize::MAX {
            return None;
        }
        let mut v: crate::vec::Vec<u8> = crate::vec::Vec::with_capacity(n);
        let m = __relax_restored_state(v.as_mut_ptr(), n);
        v.set_len(m.min(n));
        Some(String::from_utf8_lossy(&v).into_owned())
    }
}

/// Sandbox code and data statistics: [recycles, code bytes in use, dead code bytes pending,
/// image read-only bytes, image read-write bytes, live arenas]. `recycle`: rebuild the live
/// module code first (blocking; Err when frames may still run in the space it would reuse).
pub fn sandbox_stats(recycle: bool) -> Result<[u64; 6], String> {
    let mut out = [0u64; 6];
    if unsafe { __relax_sb_stats(&mut out, recycle as u32) } == 0 {
        return Err(last_error());
    }
    Ok(out)
}

// ---- used by the wrappers Relax generates for sandboxed modules (not an API)

/// An instance a sandboxed module made (a `Box` of one of its own types, living in its arena):
/// the host holds it opaquely and passes it back to the module's exports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inst {
    module: u32,
    addr: u64,
}

impl Inst {
    #[doc(hidden)]
    pub fn __from_box<T>(module: u32, b: crate::boxed::Box<T>) -> Inst {
        Inst { module, addr: crate::boxed::Box::into_raw(b) as u64 }
    }
    #[doc(hidden)]
    pub unsafe fn __get<T>(&self, module: u32) -> &'static T {
        if self.module != module {
            panic!("an instance of one module was passed to another module");
        }
        &*(self.addr as *const T)
    }
    #[doc(hidden)]
    pub unsafe fn __get_mut<T>(&self, module: u32) -> &'static mut T {
        if self.module != module {
            panic!("an instance of one module was passed to another module");
        }
        &mut *(self.addr as *mut T)
    }
}

#[doc(hidden)]
pub struct __SbFrame([i64; 4]);

impl Drop for __SbFrame {
    fn drop(&mut self) {
        unsafe { __relax_sb_leave(&self.0) }
    }
}

#[doc(hidden)]
pub fn __sb_enter(module: u32) -> __SbFrame {
    let mut prev = [0i64; 4];
    if unsafe { __relax_sb_enter(module, &mut prev) } != 0 {
        panic!("the module was unloaded");
    }
    __SbFrame(prev)
}

fn sb_alloca<T>(n: usize) -> *mut T {
    let p = unsafe { __relax_sb_alloca(core::mem::size_of::<T>() * n, core::mem::align_of::<T>()) };
    if p == 0 {
        panic!("an argument does not fit the module's shadow stack");
    }
    p as *mut T
}

#[doc(hidden)]
pub fn __sb_in<T>(x: &T) -> &'static T {
    let p = sb_alloca::<T>(1);
    unsafe {
        core::ptr::copy_nonoverlapping(x as *const T, p, 1);
        &*p
    }
}

#[doc(hidden)]
pub fn __sb_in_mut<T>(x: &T) -> &'static mut T {
    let p = sb_alloca::<T>(1);
    unsafe {
        core::ptr::copy_nonoverlapping(x as *const T, p, 1);
        &mut *p
    }
}

#[doc(hidden)]
pub fn __sb_out<T>(x: &mut T, p: &T) {
    unsafe { core::ptr::copy_nonoverlapping(p as *const T, x as *mut T, 1) }
}

#[doc(hidden)]
pub fn __sb_in_slice<T>(x: &[T]) -> &'static [T] {
    let p = sb_alloca::<T>(x.len().max(1));
    unsafe {
        core::ptr::copy_nonoverlapping(x.as_ptr(), p, x.len());
        core::slice::from_raw_parts(p, x.len())
    }
}

#[doc(hidden)]
pub fn __sb_in_slice_mut<T>(x: &[T]) -> &'static mut [T] {
    let p = sb_alloca::<T>(x.len().max(1));
    unsafe {
        core::ptr::copy_nonoverlapping(x.as_ptr(), p, x.len());
        core::slice::from_raw_parts_mut(p, x.len())
    }
}

#[doc(hidden)]
pub fn __sb_out_slice<T>(x: &mut [T], p: &[T]) {
    let n = if x.len() < p.len() { x.len() } else { p.len() };
    unsafe { core::ptr::copy_nonoverlapping(p.as_ptr(), x.as_mut_ptr(), n) }
}

#[doc(hidden)]
pub fn __sb_in_str(x: &str) -> &'static str {
    unsafe { core::str::from_utf8_unchecked(__sb_in_slice(x.as_bytes())) }
}

/// Runs `f` (a call into a module) contained: a panic, a hardware fault or (with `deadline_ms`
/// > 0) a call that does not return in time ends the call with Err(message) instead of the
/// program. After an Err the module's own state may be partial: treat that instance as failed.
pub fn guard<R>(deadline_ms: u64, f: impl FnOnce() -> R) -> Result<R, String> {
    let t = unsafe { __relax_guard_enter(deadline_ms) };
    let r = crate::panic::catch_unwind(crate::panic::AssertUnwindSafe(f));
    unsafe { __relax_guard_leave(t) };
    r.map_err(|p| {
        if let Some(s) = p.downcast_ref::<&str>() {
            String::from(*s)
        } else if let Some(s) = p.downcast_ref::<String>() {
            s.clone()
        } else {
            String::from("the module call failed (panic, fault or deadline)")
        }
    })
}

/// A deterministic account shared by consecutive or nested module callbacks. Use one
/// `Budget::new(2_000_000, 48 * 1024 * 1024)` for game initialization, then reset to
/// `(500_000, 48 * 1024 * 1024)` once per tick and share it across every callback.
/// Loading compiles source without executing game code; call the init export with the
/// eval budget after `Module::load_source`.
///
/// One fuel unit is one canonical, uninlined, lowered RIR instruction or terminator.
/// Each block pays its full cost before running; an interrupted block is still charged.
/// Calls into trusted host API/runtime/C functions cost 32 units instead of one; work
/// inside a trusted host function is not metered. Confinement and metering instructions
/// are free. Charges are inserted before optimization and retained by both tiers, so
/// tier selection and compilation timing do not change consumption for the same path.
/// A block that cannot be paid for exhausts the remaining balance without executing.
/// These units are compiler-version specific, not native instructions or elapsed time.
///
/// Allocation growth is the increase in the arena heap's high-water address, including
/// size-class rounding and alignment. Free-list reuse costs nothing; freeing does not
/// refund earlier growth. The cap is checked before growing/mapping. Mapping rounds to
/// 16 MiB, so reserved/committed pages and RSS are not the reported growth. The existing
/// module lifetime memory limit also applies. Shadow-stack and compiler memory are not
/// heap growth. Reset starts a new accounting interval without clearing the module heap.
pub struct Budget {
    fuel: u64,
    token: u64,
    marker: core::marker::PhantomData<*mut ()>,
}

impl Budget {
    pub fn new(fuel: u64, alloc_growth_bytes: u64) -> Budget {
        Budget { fuel, token: unsafe { __relax_budget_new(fuel, alloc_growth_bytes) }, marker: core::marker::PhantomData }
    }
    fn data(&self) -> [u64; 4] {
        let mut data = [0u64; 4];
        unsafe { __relax_budget_read(self.token, &mut data); }
        data
    }
    pub fn remaining_fuel(&self) -> u64 { self.data()[0] }
    pub fn consumed_fuel(&self) -> u64 { self.fuel - self.data()[0] }
    pub fn consumed_alloc_growth(&self) -> u64 { self.data()[2] }
    pub fn reset(&mut self, fuel: u64, alloc_growth_bytes: u64) {
        self.fuel = fuel;
        unsafe { __relax_budget_reset(self.token, fuel, alloc_growth_bytes); }
    }
}

impl Drop for Budget { fn drop(&mut self) { unsafe { __relax_budget_drop(self.token); } } }

/// A contained module failure. The module remains loaded; its state may be partial.
/// The host may unload it or call it again with a fresh budget. Consumed values cover
/// the entire account, including earlier callbacks, and remain available on `Budget`.
#[derive(Debug)]
pub enum BudgetError {
    Fuel { consumed: u64 },
    Alloc { consumed: u64, limit: u64 },
    /// Panic, hardware fault, watchdog, invalid module, or another boundary failure.
    Fault(String),
}

/// A Copy capability for a host-owned object, created by `Module::with_host`.
/// It contains an opaque token, never a host pointer. Copies may outlive the scope,
/// but resolving them then returns Err. Handles belong to one module and one thread;
/// another module cannot resolve them. Simultaneous/reentrant mutable resolution fails.
///
/// The supported host API pattern is concrete free functions in an allowed dependency:
/// `pub fn spawn(h: Host<GameCtx>, n: u32) -> bool { h.with(|ctx| { ctx.count += n; }).is_ok() }`.
/// Module exports accept `Host<GameCtx>` by value and call those functions. Only trusted
/// host code may resolve the handle. Raw `&dyn GameApi` and module-side trait dispatch
/// on host objects are not supported; dispatch on a trait object inside the host shim.
pub struct Host<T> {
    id: u64,
    marker: core::marker::PhantomData<*mut T>,
}
impl<T> Copy for Host<T> {}
impl<T> Clone for Host<T> { fn clone(&self) -> Self { *self } }

impl<T> Host<T> {
    /// Resolves a live capability while executing a host API callback for its module.
    /// The mutable reference cannot escape this closure. Errors include stale/wrong
    /// module handles, a reentrant borrow, and a panic/fault in the callback.
    pub fn with<R>(self, f: impl FnOnce(&mut T) -> R) -> Result<R, String> {
        let ty = core::any::type_name::<T>();
        let p = unsafe { __relax_host_acquire(self.id, ty.as_ptr(), ty.len()) };
        if p == 0 { return Err(String::from("host handle is stale, foreign, or already borrowed")); }
        let r = guard(0, || f(unsafe { &mut *(p as *mut T) }));
        unsafe { __relax_host_release(self.id) };
        r
    }
}

#[doc(hidden)]
pub fn __sb_string(s: String) -> String {
    let s = core::mem::ManuallyDrop::new(s);
    let n = s.len();
    let mut v = crate::vec::Vec::with_capacity(n);
    if unsafe { __relax_sb_string_copy(s.as_ptr() as u64, n as u64, v.as_mut_ptr()) } == 0 {
        panic!("invalid sandbox string result");
    }
    unsafe { v.set_len(n); }
    String::from_utf8(v).expect("invalid sandbox string UTF-8")
}

fn last_error() -> String {
    unsafe {
        let n = __relax_module_error(core::ptr::null_mut(), 0);
        let mut v: crate::vec::Vec<u8> = crate::vec::Vec::with_capacity(n);
        let m = __relax_module_error(v.as_mut_ptr(), n);
        v.set_len(m.min(n));
        String::from_utf8_lossy(&v).into_owned()
    }
}

/// How an edit of a module applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    /// patched in place: exports keep their addresses and run the new code
    Patched,
    /// the edit changes layouts or the item set: unload the module and load the new text
    NeedsReload,
}

/// The result of feeding a module's whole source so far.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Patched in place; `kept` items still have their previous version.
    Applied { kept: usize },
    /// No new complete, valid item applied; the running code is retained.
    Incomplete,
    /// The item set or layouts changed in a way that requires a fresh module load.
    NeedsReload,
    /// Coalesced within 16 ms; the next call or live watcher drains the latest text.
    Pending,
}

impl core::fmt::Debug for Stream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Stream::Applied { kept } => f.debug_struct("Applied").field("kept", kept).finish(),
            Stream::Incomplete => f.write_str("Incomplete"),
            Stream::NeedsReload => f.write_str("NeedsReload"),
            Stream::Pending => f.write_str("Pending"),
        }
    }
}

/// A sandboxed run-time Rust module for an app's own AI-generated code.
/// Apart from the permitted standard library, it sees only the crates supplied as `deps`.
/// Loading, replacing and streaming all enforce the same sandbox restrictions.
/// Use `get` to take typed exports, and `guard` to contain faults or deadlines when calling
/// them. This is a handle: after `unload`, handle operations fail rather than dangle.
/// Stream whole source prefixes with `load_stream` / `stream`, then validate the final text
/// with `finish_stream`. Complete top-level items apply independently; incomplete items
/// retain their last applied version, including cuts inside tokens or function bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Module(u64);

impl Module {
    /// Calls typed exports under a shared instruction/allocation account. Each export
    /// entry loads the remaining balance, and normal exits, panics, faults and watchdog
    /// recovery write consumption back. Several `call_with` calls may share one budget,
    /// or one closure may invoke several exports. Nested callbacks to the same module
    /// also charge active outer accounts. Other modules need their own accounts.
    ///
    /// `guard(deadline_ms, || module.call_with(&mut budget, || tick(api)))` adds a wall
    /// clock safety net. Budget checks are deterministic and do not depend on a deadline.
    /// Calls without a Budget retain the process-configured per-entry limits.
    /// `String` and `Result<T, String>` export results (T is Copy plain host data) are
    /// copied into host-owned storage and survive unloading; arena strings are freed.
    pub fn call_with<R>(&self, budget: &mut Budget, f: impl FnOnce() -> R) -> Result<R, BudgetError> {
        if unsafe { __relax_budget_enter(self.0, budget.token) } != 0 {
            return Err(BudgetError::Fault(String::from("the sandboxed module was unloaded or is invalid")));
        }
        let r = guard(0, f);
        unsafe { __relax_budget_leave() };
        let data = budget.data();
        match data[3] {
            1 => Err(BudgetError::Fuel { consumed: budget.consumed_fuel() }),
            2 => Err(BudgetError::Alloc { consumed: data[2], limit: data[1] }),
            _ => r.map_err(BudgetError::Fault),
        }
    }

    /// Lends `object` to this module for the duration of `f`. Callbacks receive a Copy
    /// `Host<T>`; the registry invalidates every copy on return, panic or guard recovery.
    /// The host keeps ownership and cannot access `object` while it is lent.
    pub fn with_host<T, R>(&self, object: &mut T, f: impl FnOnce(Host<T>) -> R) -> Result<R, String> {
        let ty = core::any::type_name::<T>();
        let id = unsafe { __relax_host_new(self.0, object as *mut T as u64, ty.as_ptr(), ty.len()) };
        if id == 0 { return Err(String::from("the sandboxed module was unloaded or is invalid")); }
        let r = guard(0, || f(Host { id, marker: core::marker::PhantomData }));
        unsafe { __relax_host_drop(id) };
        r
    }

    /// Loads `path` as crate `name`, depending on the running program's crates `deps`.
    pub fn load(name: &str, path: &str, deps: &[&str]) -> Result<Module, String> {
        let mut d = String::new();
        for x in deps {
            d.push_str(x);
            d.push('\n');
        }
        let h = unsafe { __relax_module_load(name.as_ptr(), name.len(), path.as_ptr(), path.len(), d.as_ptr(), d.len()) };
        if h == 0 {
            Err(last_error())
        } else {
            Ok(Module(h))
        }
    }

    /// Loads source text as crate `name`; `path` names it in errors and positions (an embedded
    /// preset, a user or AI edit held in memory). Sources use relaxed Rust.
    pub fn load_source(name: &str, path: &str, src: &str, deps: &[&str]) -> Result<Module, String> {
        let mut d = String::new();
        for x in deps {
            d.push_str(x);
            d.push('\n');
        }
        let h = unsafe { __relax_module_load_src(name.as_ptr(), name.len(), path.as_ptr(), path.len(), src.as_ptr(), src.len(), d.as_ptr(), d.len()) };
        if h == 0 {
            Err(last_error())
        } else {
            Ok(Module(h))
        }
    }

    /// Loads the complete top-level items of `text_so_far`, or an empty module if none are
    /// complete yet. `path` names diagnostics; `deps` grants access to those crates only,
    /// alongside the permitted standard library. Incomplete tokens and bodies never run.
    /// Complete items must type-check together and obey the sandbox, as with `load_source`.
    /// Later prefixes can add exports through `stream`; call `finish_stream` at the end.
    pub fn load_stream(name: &str, path: &str, text_so_far: &str, deps: &[&str]) -> Result<Module, String> {
        let mut d = String::new();
        for x in deps {
            d.push_str(x);
            d.push('\n');
        }
        let h = unsafe { __relax_module_load_stream(name.as_ptr(), name.len(), path.as_ptr(), path.len(), text_so_far.as_ptr(), text_so_far.len(), d.as_ptr(), d.len()) };
        if h == 0 { Err(last_error()) } else { Ok(Module(h)) }
    }

    /// Feeds the module's whole source so far (not a delta). Each complete top-level item
    /// applies independently; cut tokens, incomplete bodies and invalid items keep their
    /// last applied version. Missing items in a prefix are never deleted.
    /// Calls within 16 ms coalesce; feed again or run with the live watcher to drain them.
    /// Source errors are quiet until a roughly one-second stall (reported by the watcher)
    /// or `finish_stream`; `Err` here indicates an invalid handle, not unfinished source.
    /// `&str` cannot split a UTF-8 character: buffer incomplete bytes before calling.
    pub fn stream(&self, text: &str) -> Result<Stream, String> {
        let mut kept = 0;
        match unsafe { __relax_module_stream(self.0, text.as_ptr(), text.len(), &mut kept) } {
            0 => Ok(Stream::Applied { kept }),
            1 => Ok(Stream::Incomplete),
            2 => Ok(Stream::NeedsReload),
            3 => Ok(Stream::Pending),
            _ => Err(last_error()),
        }
    }

    /// Ends streaming and applies the full text as a normal `replace` transaction, without
    /// coalescing or retaining omitted items. Errors include source positions and leave the
    /// running version intact. Pending prefixes are discarded even if the final text fails.
    pub fn finish_stream(&self, text: &str) -> Result<Edit, String> {
        self.replace(text)
    }

    /// Export `name` as fn-pointer type `F` (checked against the export's signature).
    pub fn get<F: Copy + 'static>(&self, name: &str) -> Result<F, String> {
        let ty = core::any::type_name::<F>();
        if core::mem::size_of::<F>() != core::mem::size_of::<usize>() {
            return Err(crate::format!("`{}` is not a fn pointer type", ty));
        }
        let a = unsafe { __relax_module_get(self.0, name.as_ptr(), name.len(), ty.as_ptr(), ty.len()) };
        if a == 0 {
            return Err(last_error());
        }
        Ok(unsafe { core::mem::transmute_copy::<usize, F>(&(a as usize)) })
    }

    /// Applies a new text of the module (one transaction; nothing changes on an error).
    pub fn replace(&self, src: &str) -> Result<Edit, String> {
        match unsafe { __relax_module_replace(self.0, src.as_ptr(), src.len()) } {
            0 => Ok(Edit::Patched),
            2 => Ok(Edit::NeedsReload),
            _ => Err(last_error()),
        }
    }

    /// Unloads the module: its names leave the program; fn pointers taken from it keep their
    /// code (calling them is the caller's choice), the handle fails from now on.
    pub fn unload(self) {
        unsafe { __relax_module_unload(self.0) }
    }
}

/// Relax-only: the API of makepad inspect's injected probes (a probe is a fn an agent loads into the
/// running program at a hook point: `makepad inspect <app> probe add <fn|file:line> <probe.rs>`).
/// Everything is recorded for the probe that runs on this thread now; outside a probe the calls
/// do nothing.
pub mod probe {
    use crate::fmt::Debug;

    extern "C" {
        fn __relax_probe_emit(text: *const u8, len: usize);
        fn __relax_probe_count(key: *const u8, klen: usize);
        fn __relax_probe_hist(key: *const u8, klen: usize, x: f64);
        fn __relax_probe_sample(key: *const u8, klen: usize, text: *const u8, len: usize, keep: usize);
        fn __relax_probe_pause();
    }

    /// Sends `v` (its Debug form) to the agent as one `probe` event (rate-limited).
    pub fn emit<T: Debug>(v: T) {
        let s = crate::format!("{:?}", v);
        unsafe { __relax_probe_emit(s.as_ptr(), s.len()) }
    }

    /// Counts one hit under `key`.
    pub fn count(key: &str) {
        unsafe { __relax_probe_count(key.as_ptr(), key.len()) }
    }

    /// Adds `x` to the histogram `key` (count, min, max, mean, log2 buckets).
    pub fn hist(key: &str, x: f64) {
        unsafe { __relax_probe_hist(key.as_ptr(), key.len(), x) }
    }

    /// Keeps a uniform sample of at most `keep` values (Debug forms) under `key`.
    pub fn sample<T: Debug>(key: &str, v: T, keep: usize) {
        let s = crate::format!("{:?}", v);
        unsafe { __relax_probe_sample(key.as_ptr(), key.len(), s.as_ptr(), s.len(), keep) }
    }

    /// Parks this thread here as a breakpoint (an agent continues it): a conditional breakpoint
    /// is `if cond { probe::pause() }`.
    pub fn pause() {
        unsafe { __relax_probe_pause() }
    }
}
