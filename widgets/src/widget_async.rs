use {
    crate::makepad_draw::*,
    crate::makepad_draw::makepad_platform::script::std::ScriptStd,
    crate::makepad_script::{script_err_not_found, ScriptFnRef, ScriptThreadId},
    crate::widget::{WidgetRef, WidgetUid},
    crate::widget_tree::CxWidgetExt,
    std::any::Any,
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
    widget_owners: HashMap<WidgetUid, SplashVmId>,
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

pub trait CxSplashVmExt {
    fn alloc_splash_vm(&mut self) -> SplashVmId;
    fn with_script_vm_id<R>(&mut self, vm_id: SplashVmId, f: impl FnOnce(&mut ScriptVm) -> R) -> R;
    fn with_script_vm_id_thread<R>(
        &mut self,
        vm_id: SplashVmId,
        thread_id: ScriptThreadId,
        f: impl FnOnce(&mut ScriptVm) -> R,
    ) -> R;
    fn widget_vm_id(&mut self, target_uid: WidgetUid) -> SplashVmId;
    fn register_widget_vm_id(&mut self, target_uid: WidgetUid, vm_id: SplashVmId);
    fn unregister_widget_vm_id(&mut self, target_uid: WidgetUid);
}

impl CxSplashVmExt for Cx {
    fn alloc_splash_vm(&mut self) -> SplashVmId {
        ensure_widget_async_hooks_registered(self);

        let id = {
            let state = self.global::<CxWidgetAsync>();
            if state.isolated_vms.next_id == 0 {
                state.isolated_vms.next_id = 1;
            }
            let id = SplashVmId(state.isolated_vms.next_id);
            state.isolated_vms.next_id += 1;
            id
        };

        let mut std = ScriptStd::new();
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

        self.global::<CxWidgetAsync>().isolated_vms.vms.insert(
            id,
            IsolatedSplashVm {
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

        let mut isolated = self
            .global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .remove(&vm_id)
            .unwrap_or_else(|| panic!("missing Splash VM {:?}", vm_id));

        let previous_vm_id = self.global::<CxWidgetAsync>().current_vm_id;
        self.global::<CxWidgetAsync>().current_vm_id = vm_id;

        let main_std = std::mem::replace(&mut self.script_data.std, isolated.std);
        let main_vm = self.script_vm.take();
        self.script_vm = isolated.vm.take();

        let out = self.with_vm(|vm| {
            let old_budget = vm.bx.run_budget.replace(ScriptRunBudget::from_durations(
                std::time::Duration::from_millis(64),
                std::time::Duration::from_millis(64),
                512,
            ));
            let out = f(vm);
            vm.bx.run_budget = old_budget;
            out
        });

        isolated.vm = self.script_vm.take();
        self.script_vm = main_vm;
        isolated.std = std::mem::replace(&mut self.script_data.std, main_std);

        self.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
        self.global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .insert(vm_id, isolated);

        out
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

        let mut isolated = self
            .global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .remove(&vm_id)
            .unwrap_or_else(|| panic!("missing Splash VM {:?}", vm_id));

        let previous_vm_id = self.global::<CxWidgetAsync>().current_vm_id;
        self.global::<CxWidgetAsync>().current_vm_id = vm_id;

        let main_std = std::mem::replace(&mut self.script_data.std, isolated.std);
        let main_vm = self.script_vm.take();
        self.script_vm = isolated.vm.take();

        let out = self.with_vm_thread(thread_id, |vm| {
            let old_budget = vm.bx.run_budget.replace(ScriptRunBudget::from_durations(
                std::time::Duration::from_millis(64),
                std::time::Duration::from_millis(64),
                512,
            ));
            let out = f(vm);
            vm.bx.run_budget = old_budget;
            out
        });

        isolated.vm = self.script_vm.take();
        self.script_vm = main_vm;
        isolated.std = std::mem::replace(&mut self.script_data.std, main_std);

        self.global::<CxWidgetAsync>().current_vm_id = previous_vm_id;
        self.global::<CxWidgetAsync>()
            .isolated_vms
            .vms
            .insert(vm_id, isolated);

        out
    }

    fn widget_vm_id(&mut self, target_uid: WidgetUid) -> SplashVmId {
        self.global::<CxWidgetAsync>()
            .widget_owners
            .get(&target_uid)
            .copied()
            .unwrap_or(MAIN_SPLASH_VM_ID)
    }

    fn register_widget_vm_id(&mut self, target_uid: WidgetUid, vm_id: SplashVmId) {
        if target_uid != WidgetUid(0) {
            self.global::<CxWidgetAsync>()
                .widget_owners
                .insert(target_uid, vm_id);
        }
    }

    fn unregister_widget_vm_id(&mut self, target_uid: WidgetUid) {
        self.global::<CxWidgetAsync>()
            .widget_owners
            .remove(&target_uid);
    }
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
        let vm_id = self.widget_vm_id(target_uid);
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
        let vm_id = self.widget_vm_id(target_uid);
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
        let vm_id = self.widget_vm_id(target_uid);
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
        let vm_id = self.widget_vm_id(target_uid);
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
