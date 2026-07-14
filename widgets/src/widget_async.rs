use {
    crate::makepad_draw::*,
    crate::makepad_draw::makepad_platform::script::std::ScriptStd,
    crate::makepad_script::{script_err_not_found, ScriptFnRef, ScriptThreadId},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_tree::CxWidgetExt,
    std::any::Any,
    std::cell::RefCell,
    std::collections::{HashMap, VecDeque},
    std::sync::atomic::{AtomicU64, Ordering},
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
pub(crate) fn gc_dead_splash_isolates(cx: &mut Cx) {
    let dead: Vec<SplashVmId> = DEAD_SPLASH_ISOLATES.with(|g| {
        let mut g = g.borrow_mut();
        std::mem::take(&mut *g)
    });
    if dead.is_empty() {
        return;
    }
    let state = cx.global::<CxWidgetAsync>();
    for vm_id in dead {
        state.isolated_vms.vms.remove(&vm_id);
        state.heap_to_vm.retain(|_, v| *v != vm_id);
        state.ui_handle_types.remove(&vm_id);
        state.done.retain(|d| d.vm_id != vm_id);
        state.widget_to_script_calls.retain(|r| r.vm_id != vm_id);
        state.script_to_widget_calls.retain(|r| r.vm_id != vm_id);
        state
            .pending_script_to_widget_returns
            .retain(|(v, _), _| *v != vm_id);
        state.thread_map.retain(|(v, _), _| *v != vm_id);
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
    /// that owns it. Only isolate heaps are inserted; a ref whose heap isn't here
    /// (the main app heap, or an empty ref) resolves to `MAIN_SPLASH_VM_ID`. This
    /// replaces per-widget uid registration: a widget's owning VM is derived
    /// directly from its own `source` ref, so there are no coverage gaps for
    /// lazily-created widgets and no wrong-heap fallbacks.
    heap_to_vm: HashMap<usize, SplashVmId>,
    isolated_vms: IsolatedScriptVms,
    current_vm_id: SplashVmId,
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

    let out = f(cx);

    isolated.vm = cx.script_vm.take();
    cx.script_vm = outer_vm;
    isolated.std = std::mem::replace(&mut cx.script_data.std, outer_std);

    cx.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
    cx.global::<CxWidgetAsync>()
        .isolated_vms
        .vms
        .insert(vm_id, isolated);

    out
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
    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> SplashVmId;
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
            crate::script_mod(&mut vm);
            vm.bx
        };

        // Record this isolate's heap identity so any ref minted here (widget
        // sources, templates, on_click fns) routes back to this VM.
        let heap_key = bx.heap.heap_key();
        let state = self.global::<CxWidgetAsync>();
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
        if vm_id == MAIN_SPLASH_VM_ID {
            return self.with_vm(f);
        }

        if self.global::<CxWidgetAsync>().current_vm_id == vm_id {
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
        if vm_id == MAIN_SPLASH_VM_ID {
            return self.with_vm_thread(thread_id, f);
        }

        if self.global::<CxWidgetAsync>().current_vm_id == vm_id {
            return self.with_vm_thread(thread_id, f);
        }

        with_isolate_installed(self, vm_id, |cx| {
            cx.with_vm_thread(thread_id, |vm| with_splash_budget(vm, f))
        })
    }

    fn script_ref_vm_id(&mut self, script_ref: &ScriptObjectRef) -> SplashVmId {
        let heap_key = script_ref.heap_key();
        if heap_key == 0 {
            return MAIN_SPLASH_VM_ID;
        }
        self.global::<CxWidgetAsync>()
            .heap_to_vm
            .get(&heap_key)
            .copied()
            .unwrap_or(MAIN_SPLASH_VM_ID)
    }
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
    cx.global::<CxWidgetAsync>().global_ui_root_uid = root_uid;
    cx.with_vm(|vm| {
        vm.cx_mut().global::<CxWidgetAsync>().current_vm_id = MAIN_SPLASH_VM_ID;
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
        let vm_id = self.script_ref_vm_id(&source);
        self.with_script_vm_id(vm_id, |vm| {
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
        let vm_id = self.script_ref_vm_id(&source);
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
        let vm_id = self.script_ref_vm_id(&source);
        self.with_script_vm_id(vm_id, |vm| {
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
        let vm_id = self.script_ref_vm_id(&source);
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

            if prop == live_id!(root) {
                let root_uid = vm.with_cx(|cx| cx.widget_tree().root_uid());
                if root_uid == WidgetUid(0) {
                    return script_err_not_found!(vm.trap(), "ui root not found");
                }
                return vm.build_ui_handle_for_uid(root_uid);
            }

            // Script UI handles intentionally use upward flood search semantics:
            // look in current subtree first, then expand outward through ancestors.
            let child_ref = vm.with_cx(|cx| {
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
            continue;
        }

        let req = cx
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .pop_front();
        if let Some(req) = req {
            progressed = true;
            let ret = cx.with_script_vm_id_thread(req.vm_id, req.caller_thread, |vm| {
                let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(req.target_uid));
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
            continue;
        }

        let done = cx.global::<CxWidgetAsync>().done.pop_front();
        if let Some(done) = done {
            progressed = true;
            cx.with_script_vm_id(done.vm_id, |vm| {
                let widget_ref = vm.with_cx(|cx| cx.widget_tree().widget(done.target_uid));
                widget_ref.script_result(vm, done.id, done.result);
            });
            continue;
        }

        break;
    }

    progressed
}

fn register_task_hooks(cx: &mut Cx) {
    cx.add_script_task_on_thread_completed_hook(on_widget_script_thread_completed_hook);
    cx.add_script_task_pump_hook(pump_widget_async_hook);
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
