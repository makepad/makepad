use {
    crate::makepad_draw::*,
    crate::makepad_draw::makepad_platform::script::std::ScriptStd,
    crate::makepad_draw::makepad_platform::script::timer::CxScriptTimer,
    crate::makepad_script::{script_err_not_found, ScriptFnRef, ScriptThreadId},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_tree::CxWidgetExt,
    std::any::Any,
    std::cell::RefCell,
    std::collections::{HashMap, VecDeque},
    std::sync::atomic::{AtomicU64, AtomicU8, Ordering},
};

static SCRIPT_ASYNC_COUNTER: AtomicU64 = AtomicU64::new(1);
pub(crate) const WIDGET_SCRIPT_INSTRUCTION_LIMIT: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScriptAsyncId(u64);

impl ScriptAsyncId {
    pub(crate) fn new() -> Self {
        Self(SCRIPT_ASYNC_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SplashVmId(pub u64);

pub const MAIN_SPLASH_VM_ID: SplashVmId = SplashVmId(0);

/// The widget theme a Splash isolate boots with. A host picks its own theme
/// after `theme_mod`, which an isolate's prelude never sees, so it says here.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SplashTheme {
    #[default]
    Dark,
    Light,
    Skeleton,
}

static SPLASH_THEME: AtomicU8 = AtomicU8::new(0);

pub fn set_splash_theme(theme: SplashTheme) {
    SPLASH_THEME.store(theme as u8, Ordering::Relaxed);
}

fn splash_theme() -> SplashTheme {
    match SPLASH_THEME.load(Ordering::Relaxed) {
        1 => SplashTheme::Light,
        2 => SplashTheme::Skeleton,
        _ => SplashTheme::Dark,
    }
}

thread_local! {
    /// Splash isolate VMs whose owning `Splash` widget has been dropped, awaiting
    /// reclamation. `Drop` can't reach `Cx`, so it only records the id here; the
    /// real teardown happens in [`gc_dead_splash_isolates`] on the next isolate
    /// allocation. We defer rather than free in `Drop` because the isolate owns a
    /// live `ScriptHeap`/`ScriptStd` that must be dropped with a `Cx` in hand and
    /// while nothing is executing in it.
    static DEAD_SPLASH_ISOLATES: RefCell<Vec<SplashVmId>> = const { RefCell::new(Vec::new()) };
}

/// Queue a Splash isolate for reclamation on the next isolate alloc. Called from
/// `Splash::drop`, which has no `Cx`. Ignores the main VM (id 0), never an isolate.
pub(crate) fn mark_splash_isolate_dead(vm_id: SplashVmId) {
    if vm_id == MAIN_SPLASH_VM_ID {
        return;
    }
    DEAD_SPLASH_ISOLATES.with(|g| g.borrow_mut().push(vm_id));
}

/// Reclaim any isolates queued by [`mark_splash_isolate_dead`]. Called on each new
/// isolate allocation so the live count tracks live Splashes instead of growing
/// unboundedly. Dropping an isolate frees its `ScriptHeap` (and every widget object
/// minted in it) plus its `ScriptStd`, so we must also purge every queue/map that
/// could later swap into the now-missing VM (which would panic in
/// `with_script_vm_id`) or dereference its freed heap. Cheap no-op when nothing is
/// queued.
pub fn gc_dead_splash_isolates(cx: &mut Cx) {
    let dead: Vec<SplashVmId> = DEAD_SPLASH_ISOLATES.with(|g| {
        let mut g = g.borrow_mut();
        std::mem::take(&mut *g)
    });
    if dead.is_empty() {
        return;
    }
    let dead_heaps: Vec<usize> = {
        let state = cx.global::<CxWidgetAsync>();
        state
            .heap_to_vm
            .iter()
            .filter(|(_, v)| dead.contains(v))
            .map(|(k, _)| *k)
            .collect()
    };
    // Stop and drop script timers whose callbacks live in a dying heap. Their fn refs
    // hold the heap's roots Rc alive, and firing one later would deref a freed heap.
    let stale_timers: Vec<_> = cx
        .script_data
        .timers
        .timers
        .iter()
        .filter(|t| dead_heaps.contains(&t.callback.heap_key()))
        .map(|t| (t.id, t.timer))
        .collect();
    for (id, timer) in stale_timers {
        cx.stop_timer(timer);
        cx.script_data.timers.timers.retain(|t| t.id != id);
    }
    // Sandbox roots and host-bridge state die with their isolates.
    crate::splash_storage::gc_roots(&dead_heaps);
    crate::splash_host::gc_bridge(&dead_heaps);
    // And the resource cache, which is keyed by heap ADDRESS: dropping a heap
    // frees that address for the next isolate, and a leftover entry would hand
    // the newcomer a dead heap's handle. See `CxScriptResources::gc_heaps`.
    cx.script_data.resources.gc_heaps(&dead_heaps);
    let state = cx.global::<CxWidgetAsync>();
    state.dead_heaps.extend(dead_heaps.iter().copied());
    for vm_id in dead {
        // Purge everything that HOLDS the isolate's values before dropping the
        // heap those values live in: `script_to_widget_calls` carries a
        // `ScriptObjectRef` into it, and `pending_script_to_widget_returns` a
        // bare `ScriptValue`. Same reason `gc_bridge` above runs first.
        state.heap_to_vm.retain(|_, v| *v != vm_id);
        state.ui_handle_types.remove(&vm_id);
        state.vm_root_uids.remove(&vm_id);
        state.done.retain(|d| d.vm_id != vm_id);
        state.widget_to_script_calls.retain(|r| r.vm_id != vm_id);
        state.script_to_widget_calls.retain(|r| r.vm_id != vm_id);
        state
            .pending_script_to_widget_returns
            .retain(|(v, _), _| *v != vm_id);
        state.thread_map.retain(|(v, _), _| *v != vm_id);
        state.isolated_vms.vms.remove(&vm_id);
    }
}

#[derive(Clone)]
pub struct ScriptAsyncCall {
    id: ScriptAsyncId,
    method: LiveId,
    me: ScriptValue,
    thread_id: Option<ScriptThreadId>,
}

#[derive(Clone, Default)]
pub struct ScriptAsyncCalls {
    calls: Vec<ScriptAsyncCall>,
}

impl ScriptAsyncCalls {
    pub fn take(&mut self, id: ScriptAsyncId) -> Option<ScriptAsyncCall> {
        if let Some(pos) = self.calls.iter().position(|v| v.id == id) {
            Some(self.calls.swap_remove(pos))
        } else {
            None
        }
    }
}

impl ScriptAsyncCall {
    pub fn id(&self) -> ScriptAsyncId {
        self.id
    }

    pub fn method(&self) -> LiveId {
        self.method
    }

    pub fn me(&self) -> ScriptValue {
        self.me
    }

    pub fn thread_id(&self) -> Option<ScriptThreadId> {
        self.thread_id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptAsyncResult {
    Return(ScriptValue),
    Pending,
    MethodNotFound,
}

struct WidgetAsyncDone {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    id: ScriptAsyncId,
    result: ScriptValue,
}

struct ScriptToWidgetCallRequest {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    method: LiveId,
    caller_thread: ScriptThreadId,
    args: ScriptObjectRef,
}

struct ScriptToWidgetReturn {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    method: LiveId,
    result: ScriptValue,
}

struct WidgetToScriptCallRequest {
    vm_id: SplashVmId,
    target_uid: WidgetUid,
    me: ScriptValue,
    source: ScriptObjectRef,
    script_fn: ScriptFnRef,
    args: ScriptValue,
}

struct IsolatedSplashVm {
    network_enabled: bool,
    std: ScriptStd,
    vm: Option<Box<ScriptVmBase>>,
}

#[derive(Default)]
struct IsolatedScriptVms {
    next_id: u64,
    vms: HashMap<SplashVmId, IsolatedSplashVm>,
}

#[derive(Default)]
struct CxWidgetAsync {
    done: VecDeque<WidgetAsyncDone>,
    widget_to_script_calls: VecDeque<WidgetToScriptCallRequest>,
    script_to_widget_calls: VecDeque<ScriptToWidgetCallRequest>,
    pending_script_to_widget_returns: HashMap<(SplashVmId, usize), ScriptToWidgetReturn>,
    thread_map: HashMap<(SplashVmId, usize), (WidgetUid, ScriptAsyncId)>,
    ui_handle_types: HashMap<SplashVmId, ScriptHandleType>,
    global_ui_root_uid: WidgetUid,
    /// Maps a heap identity (see [`ScriptObjectRef::heap_key`]) to the isolate VM
    /// that owns it. Only isolate heaps are inserted, and an entry is removed the
    /// moment its isolate dies — so a ref that misses here is either the main app
    /// heap's (checked against the app VM's own key) or a dead isolate's, whose
    /// calls are dropped rather than routed anywhere. This replaces per-widget uid
    /// registration: a widget's owning VM is derived directly from its own
    /// `source` ref, so there are no coverage gaps for lazily-created widgets.
    heap_to_vm: HashMap<usize, SplashVmId>,
    /// Each isolate's own view-root uid (set by [`inject_splash_ui_handle`]). Isolate `ui`
    /// handles are confined to this subtree so a mini-app can't reach host/sibling widgets.
    vm_root_uids: HashMap<SplashVmId, WidgetUid>,
    isolated_vms: IsolatedScriptVms,
    current_vm_id: SplashVmId,
    /// Round-robin cursor for the per-pump isolate GC pass (last vm id serviced).
    gc_rr_last: u64,
    /// Heaps of isolates that have been reclaimed. A widget can outlive the
    /// isolate that minted it by a frame or two and still try to call back into
    /// it; this is what tells such a ref apart from an app-VM one, so it can be
    /// dropped instead of misrouted. Keys are only added once their isolate is
    /// gone and removed again if a later heap is allocated at the same address.
    dead_heaps: std::collections::HashSet<usize>,
}

#[derive(Default)]
struct CxWidgetAsyncHooksInstalled(pub bool);

struct CxWidgetHandleGc {
    handle: ScriptHandle,
    uid: WidgetUid,
}

impl ScriptHandleGc for CxWidgetHandleGc {
    fn gc(&mut self) {}

    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle;
    }
}

/// Swap isolate `vm_id` onto `Cx` — its `ScriptStd` into `cx.script_data.std` and its
/// `ScriptVmBase` into `cx.script_vm` — for the duration of `f`, then put the previous
/// pair back.
///
/// **Every path that executes an isolate's script must go through this.** A `ScriptVm`
/// hands `&mut Cx` to native code via `with_cx`/`with_cx_mut`, which park the executing
/// `bx` into `cx.script_vm` and take it back out. Run an isolate while the app VM still
/// occupies that slot and the park silently drops the app VM's entire heap, nulls the
/// slot, and leaves every subsequent `Cx`-mediated script access resolving isolate
/// object pointers against the wrong heap. Handing the isolate's `std`/`vm` to a
/// function as side-channel `&mut` args is exactly that mistake.
///
/// Nested installs are fine: the enclosing `with_vm` owns the outer `bx` (so the slot
/// reads `None` here) and restores it on the way out.
fn with_isolate_installed<R>(cx: &mut Cx, vm_id: SplashVmId, f: impl FnOnce(&mut Cx) -> R) -> R {
    let mut isolated = cx
        .global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .remove(&vm_id)
        .unwrap_or_else(|| panic!("missing Splash VM {:?}", vm_id));

    let previous_vm_id = cx.global::<CxWidgetAsync>().current_vm_id;
    cx.global::<CxWidgetAsync>().current_vm_id = vm_id;

    let outer_std = std::mem::replace(&mut cx.script_data.std, isolated.std);
    let outer_vm = cx.script_vm.take();
    cx.script_vm = isolated.vm.take();

    // A panic inside isolate script must not skip the restore below, or the
    // app VM stays swapped out and every later script access resolves against
    // the wrong heap. Catch, restore, then let the panic continue to the
    // containment layer at the entry funnel.
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut *cx)));

    isolated.vm = cx.script_vm.take();
    cx.script_vm = outer_vm;
    isolated.std = std::mem::replace(&mut cx.script_data.std, outer_std);

    cx.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
    cx.global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .insert(vm_id, isolated);

    match out {
        Ok(out) => out,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// Runs isolate-side work and CONTAINS any panic it raises: the host app must
/// survive anything a mini-app isolate does. Returns false when a panic was
/// contained; the isolate is left degraded (a Force Stop cleans it up).
pub fn contain_isolate_panic(what: &str, f: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("(non-string panic)");
            crate::makepad_platform::error!(
                "contained a mini-app isolate panic in {what}: {msg}"
            );
            false
        }
    }
}

/// A Splash isolate runs untrusted-ish user script on the UI thread; cap how long any
/// single entry into it may run.
fn with_splash_budget<R>(vm: &mut ScriptVm, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
    let old_budget = vm.bx.run_budget.replace(ScriptRunBudget::from_durations(
        std::time::Duration::from_millis(64),
        std::time::Duration::from_millis(64),
        512,
    ));
    let out = f(vm);
    vm.bx.run_budget = old_budget;
    out
}

pub trait CxSplashVmExt {
    fn alloc_splash_vm(&mut self) -> SplashVmId;
    fn alloc_splash_vm_with_network(&mut self, network_enabled: bool) -> SplashVmId;
    fn with_script_vm_id<R>(&mut self, vm_id: SplashVmId, f: impl FnOnce(&mut ScriptVm) -> R) -> R;
    fn with_script_vm_id_thread<R>(
        &mut self,
        vm_id: SplashVmId,
        thread_id: ScriptThreadId,
        f: impl FnOnce(&mut ScriptVm) -> R,
    ) -> R;
    /// Resolve the VM that owns a widget's script objects directly from a ref
    /// minted by that widget (its `source`, a template, an `on_click` fn). This
    /// is exact — the heap identity comes from the ref itself — so it never
    /// mis-routes lazily-created widgets the way a uid registry could.
    ///
    /// `None` means the ref belongs to a heap that is gone: an isolate was torn
    /// down while widgets it minted were still in the tree. Such a call must be
    /// dropped, never redirected — see the note on the impl.
    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> Option<SplashVmId>;
}

impl CxSplashVmExt for Cx {
    fn alloc_splash_vm(&mut self) -> SplashVmId {
        self.alloc_splash_vm_with_network(false)
    }

    fn alloc_splash_vm_with_network(&mut self, network_enabled: bool) -> SplashVmId {
        ensure_widget_async_hooks_registered(self);
        // Reclaim isolates from dropped Splashes before growing, so the live count
        // tracks the number of live Splash widgets rather than accumulating.
        gc_dead_splash_isolates(self);

        let id = {
            let state = self.global::<CxWidgetAsync>();
            if state.isolated_vms.next_id == 0 {
                state.isolated_vms.next_id = 1;
            }
            let id = SplashVmId(state.isolated_vms.next_id);
            state.isolated_vms.next_id += 1;
            id
        };

        let mut std = if network_enabled {
            ScriptStd::with_network_runtime(self.net.clone())
        } else {
            ScriptStd::new()
        };
        let bx = {
            let mut vm = ScriptVm {
                host: self,
                std: &mut std,
                bx: Box::new(ScriptVmBase::new()),
            };
            crate::makepad_draw::makepad_platform::script::script_mod(&mut vm);
            crate::theme_mod(&mut vm);
            match splash_theme() {
                SplashTheme::Light => {
                    vm.eval(crate::makepad_script::script! { mod.theme = mod.themes.light });
                }
                SplashTheme::Skeleton => {
                    vm.eval(crate::makepad_script::script! { mod.theme = mod.themes.skeleton });
                }
                SplashTheme::Dark => {}
            }
            crate::widgets_mod(&mut vm);
            // Splash isolates run untrusted-ish mini-app script; strip the
            // ambient-authority modules from the isolate's namespace entirely:
            // filesystem access (`fs`), child processes (`run`), and the resource
            // loader (`res`), whose handles reach BOTH the filesystem (abs_path
            // loads) and the network (web_url/http resources) without going
            // through the gated net runtime. Raw sockets are gated separately:
            // the stdlib's `net.socket_stream` errors when no net runtime is
            // configured, same as `net.http_request`.
            // `cx.quit` would let any mini-app close the whole host process.
            let strip = crate::makepad_script::script! {
                mod.fs = nil
                mod.run = nil
                mod.res = nil
                mod.cx.quit = nil
            };
            vm.eval(strip);
            // Re-register `fs` as the JAILED per-app storage module: inside an
            // isolate, "the filesystem" is the app's private sandbox directory
            // (assigned by the host via Splash::set_sandbox_dir; without one,
            // every call errors). See splash_storage.rs for the containment.
            crate::splash_storage::script_mod(&mut vm);
            // `host` is the brokered doorway to host services (location,
            // clipboard, IPC, ...); requests queue for the embedding host to
            // answer, and no host = nothing resolves. See splash_host.rs.
            crate::splash_host::script_mod(&mut vm);
            vm.bx
        };

        // Record this isolate's heap identity so any ref minted here (widget
        // sources, templates, on_click fns) routes back to this VM.
        let heap_key = bx.heap.heap_key();
        let state = self.global::<CxWidgetAsync>();
        // A heap key is an allocation address, so a fresh heap can land on one
        // a dead isolate used to own. Live registration wins over the memory of
        // the dead one.
        state.dead_heaps.remove(&heap_key);
        state.heap_to_vm.insert(heap_key, id);
        state.isolated_vms.vms.insert(
            id,
            IsolatedSplashVm {
                network_enabled,
                std,
                vm: Some(bx),
            },
        );

        id
    }

    fn with_script_vm_id<R>(&mut self, vm_id: SplashVmId, f: impl FnOnce(&mut ScriptVm) -> R) -> R {
        // "Already installed?" comes first, and the main-VM case has to prove
        // it too: `with_vm` runs against whatever VM is currently parked on
        // `Cx`, which during an isolate's own execution is that ISOLATE's. So
        // an unguarded main-VM branch here silently runs app-VM work in an
        // isolate's heap.
        let current = self.global::<CxWidgetAsync>().current_vm_id;
        if current == vm_id {
            return self.with_vm(f);
        }
        if vm_id == MAIN_SPLASH_VM_ID {
            if current != MAIN_SPLASH_VM_ID {
                // Running main-VM work here would execute against the
                // installed ISOLATE's heap and plant its values there; the
                // fault would only surface later, in a GC, with nothing left
                // pointing at this call.
                error!(
                    "BUG: main-VM script call while isolate {current:?} is installed on Cx"
                );
            }
            return self.with_vm(f);
        }

        with_isolate_installed(self, vm_id, |cx| cx.with_vm(|vm| with_splash_budget(vm, f)))
    }

    fn with_script_vm_id_thread<R>(
        &mut self,
        vm_id: SplashVmId,
        thread_id: ScriptThreadId,
        f: impl FnOnce(&mut ScriptVm) -> R,
    ) -> R {
        let current = self.global::<CxWidgetAsync>().current_vm_id;
        if current == vm_id {
            return self.with_vm_thread(thread_id, f);
        }
        if vm_id == MAIN_SPLASH_VM_ID {
            if current != MAIN_SPLASH_VM_ID {
                error!(
                    "BUG: main-VM script call while isolate {current:?} is installed on Cx"
                );
            }
            return self.with_vm_thread(thread_id, f);
        }

        with_isolate_installed(self, vm_id, |cx| {
            cx.with_vm_thread(thread_id, |vm| with_splash_budget(vm, f))
        })
    }

    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> Option<SplashVmId> {
        let heap_key = script_ref.heap_key();
        if heap_key == 0 {
            return Some(MAIN_SPLASH_VM_ID);
        }
        let state = self.global::<CxWidgetAsync>();
        if let Some(vm_id) = state.heap_to_vm.get(&heap_key).copied() {
            return Some(vm_id);
        }
        // Not a live isolate's heap. Either the app VM's own — the common case,
        // since the app VM is never in `heap_to_vm` — or one that has been
        // reclaimed, and those two must not be confused.
        //
        // An isolate's widgets can outlive it by a frame: a tile dropped
        // mid-gesture, an app force-stopped while its buttons are still in the
        // tree. Each of those holds refs minted by the dead heap. Treating them
        // as "not an isolate, therefore the app VM" routes a dead heap's
        // objects INTO the app VM, which stores them in an args object of its
        // own. Nothing complains at the time — the checked stores skip indices
        // they cannot resolve — and then the next GC walks that object,
        // indexes the app heap with a foreign heap's index, and panics
        // somewhere else entirely, in code that did nothing wrong.
        //
        // So a heap we have reclaimed is remembered, and its calls are dropped
        // rather than redirected — the same treatment
        // `script_timer_dispatch_hook` already gives a dead isolate's timers.
        if state.dead_heaps.contains(&heap_key) {
            return None;
        }
        Some(MAIN_SPLASH_VM_ID)
    }
}

/// The isolate (if any) that owns a heap, for host-bridge response routing.
pub(crate) fn vm_for_heap(cx: &mut Cx, heap_key: usize) -> Option<SplashVmId> {
    cx.global::<CxWidgetAsync>().heap_to_vm.get(&heap_key).copied()
}

/// Deliver `Event::NetworkResponses` to a Splash isolate's script (resolving its
/// `net.http_request` callbacks / promises). Responses that belong to other VMs simply
/// find no matching request id in this isolate's `ScriptStd` and are ignored.
pub(crate) fn handle_splash_network_responses(
    cx: &mut Cx,
    vm_id: SplashVmId,
    responses: &[NetworkResponse],
) {
    if vm_id == MAIN_SPLASH_VM_ID || responses.is_empty() {
        return;
    }

    match cx.global::<CxWidgetAsync>().isolated_vms.vms.get(&vm_id) {
        Some(isolated) if isolated.network_enabled => {}
        _ => return,
    }

    // The isolate has to be installed on `Cx` while its handlers run — see
    // `with_isolate_installed`.
    with_isolate_installed(cx, vm_id, |cx| {
        cx.handle_script_network_events_for_current_vm(responses)
    });
}

#[doc(hidden)]
pub fn set_widget_async_trace(_cx: &mut Cx, _enabled: bool) {}

fn force_set_map_value(heap: &mut ScriptHeap, obj: ScriptObject, key: LiveId, value: ScriptValue) {
    heap.map_mut_with((key, value), obj, |(key, value), map| {
        map.insert(
            key.into(),
            ScriptMapValue {
                tag: Default::default(),
                value,
            },
        );
    });
}

#[doc(hidden)]
pub fn ensure_widget_async_hooks_registered(cx: &mut Cx) {
    cx.global::<CxWidgetAsync>();
    if cx.global::<CxWidgetAsyncHooksInstalled>().0 {
        return;
    }
    register_task_hooks(cx);
    cx.global::<CxWidgetAsyncHooksInstalled>().0 = true;
}

/// Inject `ui` as a real global into an isolated Splash VM, resolving against that splash's own
/// view root. The on_click/on_return callback path already injects `ui` into the *closure* scope,
/// but a closure that calls a helper `fn` (the natural way to write e.g. a calculator) leaves the
/// helper unable to see `ui`. Making `ui` a global on the splash VM fixes that so `ui.<id>` works
/// everywhere inside a runsplash block, not just inline in the handler.
pub(crate) fn inject_splash_ui_handle(cx: &mut Cx, vm_id: SplashVmId, root_uid: WidgetUid) {
    if vm_id == MAIN_SPLASH_VM_ID {
        return;
    }
    ensure_widget_async_hooks_registered(cx);
    // Remember this isolate's view root so its ui handles stay confined to that subtree.
    cx.global::<CxWidgetAsync>()
        .vm_root_uids
        .insert(vm_id, root_uid);
    cx.with_script_vm_id(vm_id, |vm| {
        let ui_handle = vm.build_ui_handle_for_uid(root_uid);
        vm.set_injected_global(id!(ui), ui_handle);
    });
}

pub(crate) fn update_global_ui_handle(cx: &mut Cx, root_uid: WidgetUid) {
    ensure_widget_async_hooks_registered(cx);
    if cx.global::<CxWidgetAsync>().global_ui_root_uid == root_uid {
        return;
    }
    // `with_vm` below runs against whatever VM is installed; with an isolate
    // installed this would mint the main `ui` handle in the isolate's heap
    // and leave current_vm_id clobbered to MAIN for the rest of the isolate's
    // execution. Defer to the next main-context call instead.
    if cx.global::<CxWidgetAsync>().current_vm_id != MAIN_SPLASH_VM_ID {
        error!(
            "BUG: update_global_ui_handle while isolate {:?} is installed; deferred",
            cx.global::<CxWidgetAsync>().current_vm_id
        );
        return;
    }
    cx.global::<CxWidgetAsync>().global_ui_root_uid = root_uid;
    cx.with_vm(|vm| {
        let ui_handle = vm.build_ui_handle_for_uid(root_uid);
        vm.set_injected_global(id!(ui), ui_handle);
    });
}

trait WidgetToScriptCallExt {
    fn build_ui_handle_for_uid(&mut self, target_uid: WidgetUid) -> ScriptValue;

    fn make_call_args_object_with_context(
        &mut self,
        source: ScriptObject,
        ui: ScriptValue,
        forwarded_args: ScriptValue,
    ) -> ScriptObject;

    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    );

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    );

    fn enqueue_script_to_widget_call(
        &mut self,
        target_uid: WidgetUid,
        method: LiveId,
        args: ScriptValue,
    );
}

impl<'a> WidgetToScriptCallExt for ScriptVm<'a> {
    fn build_ui_handle_for_uid(&mut self, target_uid: WidgetUid) -> ScriptValue {
        ensure_widget_async_hooks_registered(self.cx_mut());
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        if self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_types
            .get(&vm_id)
            .is_none()
        {
            register_ui_handle(self);
        }

        let ui_type = self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_types
            .get(&vm_id)
            .copied()
            .expect("ui handle type not registered");

        let gc = CxWidgetHandleGc {
            handle: ScriptHandle::ZERO,
            uid: target_uid,
        };
        self.bx.heap.new_handle(ui_type, Box::new(gc)).into()
    }

    fn make_call_args_object_with_context(
        &mut self,
        source: ScriptObject,
        ui: ScriptValue,
        forwarded_args: ScriptValue,
    ) -> ScriptObject {
        let args_obj = self.bx.heap.new_object();
        // Keep mixed (map + vec) semantics so named context vars like `ui` and `self`
        // are stored in map keys, while positional forwarded args stay in vec.
        self.bx.heap.set_object_storage_auto(args_obj);
        self.bx.heap.clear_object_deep(args_obj);

        let trap = self.bx.threads.cur().trap.pass();
        if let Some(obj) = forwarded_args.as_object() {
            self.bx.heap.merge_object(args_obj, obj, trap);
        } else if let Some(arr) = forwarded_args.as_array() {
            let len = self.bx.heap.array_len(arr);
            for index in 0..len {
                let value = self.bx.heap.array_index(arr, index, trap);
                self.bx.heap.vec_push(args_obj, NIL, value, trap);
            }
        } else if !forwarded_args.is_nil() {
            self.bx.heap.vec_push(args_obj, NIL, forwarded_args, trap);
        }

        self.bx
            .heap
            .set_value(args_obj, id!(self).into(), source.into(), trap);
        self.bx.heap.set_value(args_obj, id!(ui).into(), ui, trap);

        args_obj
    }

    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        if script_fn.as_object() == ScriptObject::ZERO {
            return ScriptAsyncResult::MethodNotFound;
        }

        let async_id = ScriptAsyncId::new();
        let ui_handle = self.build_ui_handle_for_uid(target_uid);
        let call_args =
            self.make_call_args_object_with_context(source.as_object(), ui_handle, args);
        let result = self.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
            vm.call_with_args_object_with_me(script_fn.clone().into(), call_args, me)
        });

        let thread = self.bx.threads.cur_ref();
        if thread.is_paused() {
            let thread_id = thread.thread_id();
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: Some(thread_id),
            });
            let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .thread_map
                .insert((vm_id, thread_id.to_index()), (target_uid, async_id));
            ScriptAsyncResult::Pending
        } else {
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: None,
            });
            let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .done
                .push_back(WidgetAsyncDone {
                    vm_id,
                    target_uid,
                    id: async_id,
                    result,
                });
            ScriptAsyncResult::Return(result)
        }
    }

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let args_obj = self.bx.heap.new_object();
        self.bx.heap.set_object_storage_vec2(args_obj);
        self.bx.heap.clear_object_deep(args_obj);
        let trap = self.bx.threads.cur().trap.pass();
        for value in args {
            self.bx.heap.vec_push(args_obj, NIL, *value, trap);
        }
        self.widget_to_script_async_call_fwd(
            target_uid,
            script_async,
            me,
            source,
            script_fn,
            args_obj.into(),
            from_method,
        )
    }

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    ) {
        if script_fn.as_object() == ScriptObject::ZERO {
            return;
        }
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .widget_to_script_calls
            .push_back(WidgetToScriptCallRequest {
                vm_id,
                target_uid,
                me,
                source,
                script_fn,
                args,
            });
    }

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    ) {
        let args_obj = self.bx.heap.new_object();
        self.bx.heap.set_object_storage_vec2(args_obj);
        self.bx.heap.clear_object_deep(args_obj);
        let trap = self.bx.threads.cur().trap.pass();
        for value in args {
            self.bx.heap.vec_push(args_obj, NIL, *value, trap);
        }
        self.widget_to_script_call_fwd(target_uid, me, source, script_fn, args_obj.into());
    }

    fn enqueue_script_to_widget_call(
        &mut self,
        target_uid: WidgetUid,
        method: LiveId,
        args: ScriptValue,
    ) {
        let args_ref = if let Some(args_obj) = args.as_object() {
            self.bx.heap.new_object_ref(args_obj)
        } else {
            let obj = self.bx.heap.new_object();
            self.bx.heap.set_object_storage_vec2(obj);
            self.bx.heap.clear_object_deep(obj);
            if !args.is_nil() {
                self.bx
                    .heap
                    .vec_push(obj, NIL, args, self.bx.threads.cur().trap.pass());
            }
            self.bx.heap.new_object_ref(obj)
        };

        let caller_thread = self.bx.threads.cur_ref().thread_id();
        let vm_id = self.cx_mut().global::<CxWidgetAsync>().current_vm_id;
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .push_back(ScriptToWidgetCallRequest {
                vm_id,
                target_uid,
                method,
                caller_thread,
                args: args_ref,
            });

        self.bx.threads.cur().pause();
    }
}

pub trait CxWidgetToScriptCallExt {
    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult;

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    );

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    );
}

impl CxWidgetToScriptCallExt for Cx {
    fn widget_to_script_async_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            error!(
                "widget->script call {:?} dropped: widget {:?} source belongs to a reclaimed isolate heap",
                from_method, target_uid
            );
            return ScriptAsyncResult::MethodNotFound;
        };
        self.with_script_vm_id(vm_id, |vm| {
            let src_key = source.heap_key();
            if src_key != 0 && src_key != vm.bx.heap.heap_key() {
                error!(
                    "BUG: widget->script call {:?} for widget {:?} routed to vm {:?} whose heap does not own the widget's source",
                    from_method, target_uid, vm_id
                );
            }
            vm.widget_to_script_async_call_fwd(
                target_uid,
                script_async,
                me,
                source,
                script_fn,
                args,
                from_method,
            )
        })
    }

    fn widget_to_script_async_call(
        &mut self,
        target_uid: WidgetUid,
        script_async: &mut ScriptAsyncCalls,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
        from_method: LiveId,
    ) -> ScriptAsyncResult {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            return ScriptAsyncResult::MethodNotFound;
        };
        self.with_script_vm_id(vm_id, |vm| {
            vm.widget_to_script_async_call(
                target_uid,
                script_async,
                me,
                source,
                script_fn,
                args,
                from_method,
            )
        })
    }

    fn widget_to_script_call_fwd(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: ScriptValue,
    ) {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            error!(
                "widget->script call dropped: widget {:?} source belongs to a reclaimed isolate heap",
                target_uid
            );
            return;
        };
        self.with_script_vm_id(vm_id, |vm| {
            let src_key = source.heap_key();
            if src_key != 0 && src_key != vm.bx.heap.heap_key() {
                error!(
                    "BUG: widget->script call for widget {:?} routed to vm {:?} whose heap does not own the widget's source",
                    target_uid, vm_id
                );
            }
            vm.widget_to_script_call_fwd(target_uid, me, source, script_fn, args);
        });
    }

    fn widget_to_script_call(
        &mut self,
        target_uid: WidgetUid,
        me: ScriptValue,
        source: ScriptObjectRef,
        script_fn: ScriptFnRef,
        args: &[ScriptValue],
    ) {
        let Some(vm_id) = self.script_ref_vm_id(&source) else {
            return;
        };
        self.with_script_vm_id(vm_id, |vm| {
            vm.widget_to_script_call(target_uid, me, source, script_fn, args);
        });
    }
}

fn register_ui_handle(vm: &mut ScriptVm) {
    let vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
    if vm
        .cx_mut()
        .global::<CxWidgetAsync>()
        .ui_handle_types
        .contains_key(&vm_id)
    {
        return;
    }

    let ui_type = vm.new_handle_type(id_lut!(ui));

    vm.set_handle_getter(ui_type, move |vm, pself, prop| {
        if let Some(handle) = pself.as_handle() {
            let Some(target_uid) = vm
                .downcast_handle_gc::<CxWidgetHandleGc>(handle)
                .map(|gc| gc.uid)
            else {
                return script_err_not_found!(vm.trap(), "invalid ui handle");
            };

            // Isolate VMs are confined to their own splash subtree: `ui.root` resolves to
            // the splash's view root (never the app root), and name lookups only search
            // within that subtree. Without this, a mini-app could reach host widgets or
            // widgets belonging to a sibling mini-app.
            let cur_vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            let confine_root = if cur_vm_id == MAIN_SPLASH_VM_ID {
                None
            } else {
                Some(
                    vm.cx_mut()
                        .global::<CxWidgetAsync>()
                        .vm_root_uids
                        .get(&cur_vm_id)
                        .copied()
                        .unwrap_or(target_uid),
                )
            };

            if prop == live_id!(root) {
                let root_uid = match confine_root {
                    Some(root_uid) => root_uid,
                    None => vm.with_cx(|cx| cx.widget_tree().root_uid()),
                };
                if root_uid == WidgetUid(0) {
                    return script_err_not_found!(vm.trap(), "ui root not found");
                }
                return vm.build_ui_handle_for_uid(root_uid);
            }

            // Script UI handles intentionally use upward flood search semantics:
            // look in current subtree first, then expand outward through ancestors.
            let child_ref = vm.with_cx(|cx| {
                if let Some(confine_root) = confine_root {
                    // Confined (isolate) search: the target's subtree first, then the
                    // splash root's subtree. Never the whole tree.
                    let child_ref = cx.widget_tree().find_within(target_uid, &[prop]);
                    if !child_ref.is_empty() {
                        return child_ref;
                    }
                    return cx.widget_tree().find_within(confine_root, &[prop]);
                }

                let child_ref = cx.widget_tree().find_flood(target_uid, &[prop]);
                if !child_ref.is_empty() {
                    return child_ref;
                }

                let mut matches = cx
                    .widget_tree()
                    .find_all_anywhere_including_skipped(&[prop]);
                if matches.len() == 1 {
                    return matches.pop().unwrap();
                }

                WidgetRef::empty()
            });
            if child_ref.is_empty() {
                return script_err_not_found!(vm.trap(), "widget '{:?}' not found in tree", prop);
            }

            let child_uid = child_ref.widget_uid();
            if child_uid == WidgetUid(0) {
                return script_err_not_found!(vm.trap(), "widget has no uid");
            }

            let gc = CxWidgetHandleGc {
                handle: ScriptHandle::ZERO,
                uid: child_uid,
            };
            let child_handle = vm.bx.heap.new_handle(ui_type, Box::new(gc));
            return child_handle.into();
        }

        script_err_not_found!(vm.trap(), "invalid ui handle")
    });

    vm.set_handle_call(ui_type, move |vm, args, method| {
        let pself = script_value!(vm, args.self);
        if let Some(handle) = pself.as_handle() {
            let Some(uid) = vm
                .downcast_handle_gc::<CxWidgetHandleGc>(handle)
                .map(|gc| gc.uid)
            else {
                return script_err_not_found!(vm.trap(), "invalid ui handle");
            };

            let ui_handle = vm.build_ui_handle_for_uid(uid);
            force_set_map_value(&mut vm.bx.heap, args, id!(ui), ui_handle);

            let caller_thread = vm.bx.threads.cur_ref().thread_id();
            let vm_id = vm.cx_mut().global::<CxWidgetAsync>().current_vm_id;
            if let Some(pending) = vm
                .cx_mut()
                .global::<CxWidgetAsync>()
                .pending_script_to_widget_returns
                .remove(&(vm_id, caller_thread.to_index()))
            {
                if pending.vm_id == vm_id && pending.target_uid == uid && pending.method == method {
                    return pending.result;
                }
                vm.cx_mut()
                    .global::<CxWidgetAsync>()
                    .pending_script_to_widget_returns
                    .insert((vm_id, caller_thread.to_index()), pending);
            }

            vm.enqueue_script_to_widget_call(uid, method, args.into());
            return NIL;
        }

        script_err_not_found!(vm.trap(), "invalid ui handle for method call")
    });

    vm.cx_mut()
        .global::<CxWidgetAsync>()
        .ui_handle_types
        .insert(vm_id, ui_type);
}

fn on_widget_script_thread_completed(
    cx: &mut Cx,
    vm_id: SplashVmId,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    cx.global::<CxWidgetAsync>()
        .pending_script_to_widget_returns
        .remove(&(vm_id, thread_id.to_index()));

    let Some((target_uid, async_id)) = cx
        .global::<CxWidgetAsync>()
        .thread_map
        .remove(&(vm_id, thread_id.to_index()))
    else {
        return false;
    };

    cx.global::<CxWidgetAsync>()
        .done
        .push_back(WidgetAsyncDone {
            vm_id,
            target_uid,
            id: async_id,
            result,
        });
    true
}

fn pump_widget_async(cx: &mut Cx) -> bool {
    let mut progressed = false;

    loop {
        let req = cx
            .global::<CxWidgetAsync>()
            .widget_to_script_calls
            .pop_front();
        if let Some(req) = req {
            progressed = true;
            contain_isolate_panic("widget->script dispatch", || {
                cx.with_script_vm_id(req.vm_id, |vm| {
                    if req.script_fn.as_object() != ScriptObject::ZERO {
                        let ui_handle = vm.build_ui_handle_for_uid(req.target_uid);
                        let call_args = vm.make_call_args_object_with_context(
                            req.source.as_object(),
                            ui_handle,
                            req.args,
                        );
                        let _ = vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
                            vm.call_with_args_object_with_me(
                                req.script_fn.clone().into(),
                                call_args,
                                req.me,
                            )
                        });
                    }
                });
            });
            continue;
        }

        let req = cx
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .pop_front();
        if let Some(req) = req {
            progressed = true;
            contain_isolate_panic("script->widget dispatch", || {
            let ret = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| {
                let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(req.target_uid));
                if widget_ref.is_empty() {
                    error!(
                        "script->widget call {:?} dropped: widget {:?} not in the widget tree (vm {:?})",
                        req.method, req.target_uid, req.vm_id
                    );
                }
                match widget_ref.script_call(vm, req.method, req.args.as_object().into()) {
                    ScriptAsyncResult::Return(value) => value,
                    ScriptAsyncResult::Pending => NIL,
                    ScriptAsyncResult::MethodNotFound => script_err_not_found!(
                        vm.trap(),
                        "widget method {:?} not found for uid {:?}",
                        req.method,
                        req.target_uid
                    ),
                }
            });
            cx.global::<CxWidgetAsync>()
                .pending_script_to_widget_returns
                .insert(
                    (req.vm_id, req.caller_thread.to_index()),
                    ScriptToWidgetReturn {
                        vm_id: req.vm_id,
                        target_uid: req.target_uid,
                        method: req.method,
                        result: ret,
                    },
                );
            let result = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| vm.resume());
            let is_paused = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| {
                vm.thread().is_paused()
            });
            if !is_paused {
                on_widget_script_thread_completed(cx, req.vm_id, req.caller_thread, result);
            }
            });
            continue;
        }

        let done = cx.global::<CxWidgetAsync>().done.pop_front();
        if let Some(done) = done {
            progressed = true;
            contain_isolate_panic("async result delivery", || {
                cx.with_script_vm_id(done.vm_id, |vm| {
                    let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(done.target_uid));
                    if widget_ref.is_empty() {
                        error!(
                            "script_result dropped: widget {:?} not in the widget tree (vm {:?})",
                            done.target_uid, done.vm_id
                        );
                    }
                    widget_ref.script_result(vm, done.id, done.result);
                });
            });
            continue;
        }

        break;
    }

    // Isolate maintenance — runs on every pump, cheap when idle. Without
    // this, isolated Splash VMs never garbage-collect at all (only the app
    // VM has a paint-loop GC): a 60Hz script host accumulates per-tick
    // objects forever, and dead isolates only reclaimed on the next alloc.
    gc_dead_splash_isolates(cx);
    let state = cx.global::<CxWidgetAsync>();
    if !state.isolated_vms.vms.is_empty() {
        // Round-robin: give at most one isolate a GC opportunity per pump,
        // gated on the heap's own growth heuristic (needs_gc). Mark/sweep
        // runs directly on the parked ScriptVmBase — no Cx install needed.
        // An isolate currently installed on Cx is absent from the map and
        // naturally skipped.
        let mut ids: Vec<u64> = state.isolated_vms.vms.keys().map(|v| v.0).collect();
        ids.sort_unstable();
        let last = state.gc_rr_last;
        let next = ids.iter().copied().find(|id| *id > last).unwrap_or(ids[0]);
        state.gc_rr_last = next;
        if let Some(iso) = state.isolated_vms.vms.get_mut(&SplashVmId(next)) {
            if let Some(bx) = iso.vm.as_mut() {
                if bx.heap.needs_gc() {
                    contain_isolate_panic("isolate gc", || {
                        bx.heap.mark(&bx.threads, &bx.code);
                        bx.heap.sweep(false);
                    });
                }
            }
        }
    }

    progressed
}

fn register_task_hooks(cx: &mut Cx) {
    cx.add_script_task_on_thread_completed_hook(on_widget_script_thread_completed_hook);
    cx.add_script_task_pump_hook(pump_widget_async_hook);
    cx.add_script_timer_dispatch_hook(script_timer_dispatch_hook);
}

/// Routes a firing script timer to the isolate VM that owns its callback. Without this,
/// `std.start_timeout`/`start_interval` called inside a Splash isolate would run their
/// callbacks on the main VM against the wrong heap.
fn script_timer_dispatch_hook(cx: &mut Cx, timer: &CxScriptTimer, time: ScriptValue) -> bool {
    let heap_key = timer.callback.heap_key();
    if heap_key == 0 {
        return false;
    }
    let vm_id = cx
        .global::<CxWidgetAsync>()
        .heap_to_vm
        .get(&heap_key)
        .copied();
    match vm_id {
        Some(vm_id) => {
            // Same budget/limit as any other isolate entry, so a runaway timer callback
            // can't hang the host.
            contain_isolate_panic("timer callback", || {
                cx.with_script_vm_id(vm_id, |vm| {
                    vm.with_instruction_limit(WIDGET_SCRIPT_INSTRUCTION_LIMIT, |vm| {
                        vm.call(timer.callback.as_object().into(), &[time]);
                    });
                });
            });
            true
        }
        None => {
            // Not a live isolate's heap. The main VM's own timers fall through to the
            // default dispatch; anything else is a stale timer from a dead isolate.
            let main_heap_key = cx.with_vm(|vm| vm.bx.heap.heap_key());
            if heap_key == main_heap_key {
                false
            } else {
                cx.stop_timer(timer.timer);
                let id = timer.id;
                cx.script_data.timers.timers.retain(|t| t.id != id);
                true
            }
        }
    }
}

fn on_widget_script_thread_completed_hook(
    host: &mut dyn Any,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    host.downcast_mut::<Cx>()
        .map(|cx| on_widget_script_thread_completed(cx, MAIN_SPLASH_VM_ID, thread_id, result))
        .unwrap_or(false)
}

fn pump_widget_async_hook(host: &mut dyn Any) -> bool {
    host.downcast_mut::<Cx>()
        .map(pump_widget_async)
        .unwrap_or(false)
}

#[cfg(test)]
mod isolate_tests {
    use super::*;
    use crate::splash::Splash;
    use crate::view::View;
    use crate::widget_tree::set_ui_root;

    const BODY: &str = r#"
    let items = []
    fn go(){ ui.item_list.render() }
    fn load(){
        host.request("matrix.room_threads", {limit: 20}, fn(r){
            if r.is_ok {
                items = r.data.threads
                ui.header.set_text("" + items.len() + " threads")
            } else {
                items = []
                ui.header.set_text(r.error)
            }
            ui.item_list.render()
        })
    }
    header := Label{ text: "Loading" }
    item_list := View{ height: Fit, on_render: || {
        for it in items {
            Label{text: it.body}
        }
    } }
"#;

    fn item_list_children(cx: &Cx, host: &WidgetRef) -> usize {
        let w = host.widget(cx, &[live_id!(item_list)]);
        w.borrow::<View>()
            .map(|v| v.children.len())
            .unwrap_or(usize::MAX)
    }

    fn render_cycle(cx: &mut Cx, host: &WidgetRef, json: &'static str) -> usize {
        let splash = host.widget(cx, &[live_id!(splash)]);
        let item_list = host.widget(cx, &[live_id!(item_list)]);
        render_cycle_no_heal(cx, &splash, &item_list, json)
    }

    // Triggers load()+respond via direct WidgetRefs, so no tree lookup can
    // re-seed graph nodes; this is the app's own timer/callback view of the
    // world, where ui.X resolution relies purely on the existing graph.
    fn render_cycle_no_heal(
        cx: &mut Cx,
        splash: &WidgetRef,
        item_list: &WidgetRef,
        json: &'static str,
    ) -> usize {
        let called = splash
            .borrow_mut::<Splash>()
            .expect("splash widget")
            .call_script_fn(cx, live_id!(load), &[]);
        assert!(called, "load() not found in body scope");
        pump_widget_async(cx);
        let reqs = crate::splash_host::take_splash_host_requests();
        assert_eq!(reqs.len(), 1, "expected one bridge request");
        let req = &reqs[0];
        let outcome =
            crate::splash_host::splash_host_respond(cx, req.heap_key, req.req_id, Ok(json));
        eprintln!("### respond outcome {outcome:?}");
        pump_widget_async(cx);
        item_list
            .borrow::<View>()
            .map(|v| v.children.len())
            .unwrap_or(usize::MAX)
    }

    #[test]
    fn force_stop_midflight_then_stale_respond() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.bx.heap.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        // run 1: request made, ANSWERED, but force-stopped BEFORE the pump
        // (paused callback thread + queued ui calls die with the isolate)
        let host = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
        host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let hk1 = host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .isolate_heap_key(&mut cx)
            .unwrap();
        let _ = host.widget(&cx, &[live_id!(item_list)]);
        assert!(host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .call_script_fn(&mut cx, live_id!(load), &[]));
        pump_widget_async(&mut cx);
        // a second request left UNANSWERED at force stop
        assert!(host
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .call_script_fn(&mut cx, live_id!(load), &[]));
        pump_widget_async(&mut cx);
        let reqs = crate::splash_host::take_splash_host_requests();
        assert_eq!(reqs.len(), 2);
        // answer the FIRST request but do NOT pump: callback ran, ui calls queued
        let outcome = crate::splash_host::splash_host_respond(
            &mut cx,
            reqs[0].heap_key,
            reqs[0].req_id,
            Ok(r#"{"threads":[{"sender":"a","body":"one"}]}"#),
        );
        eprintln!("### run1 respond outcome {outcome:?}");
        // force stop NOW, queues still full
        drop(host);
        gc_dead_splash_isolates(&mut cx);

        // run 2
        let host2 = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host2.clone());
        host2.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let hk2 = host2
            .widget(&cx, &[live_id!(splash)])
            .borrow_mut::<Splash>()
            .unwrap()
            .isolate_heap_key(&mut cx)
            .unwrap();
        eprintln!("### heap reuse: hk1={hk1} hk2={hk2} same={}", hk1 == hk2);

        // stale respond for the dead isolate's outstanding request arrives late
        let stale = crate::splash_host::splash_host_respond(
            &mut cx,
            reqs[1].heap_key,
            reqs[1].req_id,
            Ok(r#"{"threads":[{"sender":"z","body":"stale"}]}"#),
        );
        eprintln!("### stale respond outcome {stale:?}");

        let n = render_cycle(
            &mut cx,
            &host2,
            r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#,
        );
        eprintln!("### run2 children = {n}");

        // GC everything hard, hunting the cross-heap panic
        cx.with_vm(|vm| vm.gc());
        let ids: Vec<SplashVmId> = cx
            .global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .keys()
            .copied()
            .collect();
        for id in ids {
            if let Some(iso) = cx.global::<CxWidgetAsync>().isolated_vms.vms.get_mut(&id) {
                if let Some(bx) = iso.vm.as_mut() {
                    bx.heap.mark(&bx.threads, &bx.code);
                    bx.heap.sweep(false);
                }
            }
        }
        assert_eq!(n, 2, "run2 render did not commit");
    }

    #[test]
    fn pane_refresh_drops_inserted_host() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.bx.heap.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        let host = cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
        cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
        host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
        let n = render_cycle(
            &mut cx,
            &host,
            r#"{"threads":[{"sender":"a","body":"one"}]}"#,
        );
        eprintln!("### baseline children = {n}");
        assert_eq!(n, 1);

        let splash = host.widget(&cx, &[live_id!(splash)]);
        let item_list = host.widget(&cx, &[live_id!(item_list)]);

        let topdown0 = !cx
            .widget_tree()
            .find_within(pane_uid, &[live_id!(item_list)])
            .is_empty();
        eprintln!("### before flush: top-down find from pane: {topdown0}");

        // what teardown / any structural event does to the owner
        cx.widget_tree_mark_dirty(pane_uid);
        // any flood search flushes with mark_structure_dirty=true
        let _ = cx.widget_tree().root_uid();
        let in_graph = !cx.widget_tree().widget(splash.widget_uid()).is_empty();
        let topdown = !cx
            .widget_tree()
            .find_within(pane_uid, &[live_id!(item_list)])
            .is_empty();
        eprintln!("### after flush: splash node in graph: {in_graph}, top-down find from pane: {topdown}");

        let n = render_cycle_no_heal(
            &mut cx,
            &splash,
            &item_list,
            r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#,
        );
        eprintln!("### after pane refresh children = {n}");
        assert_eq!(n, 2, "render after pane refresh did not commit");
    }

    #[test]
    fn second_isolate_render_commits() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        let template = cx.with_vm(|vm| {
            crate::script_mod(vm);
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                use mod.widgets.*
                View{ splash := Splash{} }
            });
            let obj = v.as_object().expect("template did not eval to an object");
            vm.bx.heap.new_object_ref(obj)
        });
        let pane = cx.with_vm(|vm| {
            let v = vm.eval(crate::makepad_script::script! {
                use mod.prelude.widgets.*
                View{ height: Fit }
            });
            WidgetRef::script_from_value(vm, v)
        });
        set_ui_root(&mut cx, &pane);
        let pane_uid = pane.widget_uid();

        for run in 0..2 {
            let host =
                cx.with_vm(|vm| WidgetRef::script_from_value(vm, template.as_object().into()));
            cx.widget_tree_insert_child_deep(pane_uid, live_id!(apphost), host.clone());
            host.widget(&cx, &[live_id!(splash)]).set_text(&mut cx, BODY);
            let hk = host
                .widget(&cx, &[live_id!(splash)])
                .borrow_mut::<Splash>()
                .unwrap()
                .isolate_heap_key(&mut cx);
            eprintln!("### run {run}: heap_key {hk:?}");
            // warm the graph so the confined ui getter can resolve while the
            // Splash itself is mut-borrowed below
            let warm = host.widget(&cx, &[live_id!(item_list)]);
            assert!(!warm.is_empty(), "run {run}: item_list not found in tree");
            drop(warm);
            let called = host
                .widget(&cx, &[live_id!(splash)])
                .borrow_mut::<Splash>()
                .expect("splash widget")
                .call_script_fn(&mut cx, live_id!(load), &[]);
            assert!(called, "run {run}: load() not found in body scope");
            pump_widget_async(&mut cx);
            // the host drains and answers the bridge request, like robrix's broker
            let reqs = crate::splash_host::take_splash_host_requests();
            assert_eq!(reqs.len(), 1, "run {run}: expected one bridge request");
            let req = &reqs[0];
            let outcome = crate::splash_host::splash_host_respond(
                &mut cx,
                req.heap_key,
                req.req_id,
                Ok(r#"{"threads":[{"sender":"a","body":"one"},{"sender":"b","body":"two"}]}"#),
            );
            eprintln!("### run {run}: respond outcome {outcome:?}");
            pump_widget_async(&mut cx);
            let n = item_list_children(&cx, &host);
            eprintln!("### run {run}: item_list children = {n}");

            // aggressive GC afterwards, hunting the cross-heap panic
            cx.with_vm(|vm| vm.gc());
            {
                let state = cx.global::<CxWidgetAsync>();
                let ids: Vec<SplashVmId> =
                    state.isolated_vms.vms.keys().copied().collect();
                for id in ids {
                    if let Some(iso) = cx
                        .global::<CxWidgetAsync>()
                        .isolated_vms
                        .vms
                        .get_mut(&id)
                    {
                        if let Some(bx) = iso.vm.as_mut() {
                            bx.heap.mark(&bx.threads, &bx.code);
                            bx.heap.sweep(false);
                        }
                    }
                }
            }
            assert!(n >= 1, "run {run}: render did not commit (children={n})");

            // force stop: drop every strong ref, then reclaim like the
            // end-of-cycle pump does
            drop(host);
            pump_widget_async(&mut cx);
        }
    }
}
