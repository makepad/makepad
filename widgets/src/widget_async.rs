use {
    crate::makepad_draw::*,
    crate::makepad_script::{script_err_not_found, ScriptFnRef, ScriptThreadId},
    crate::widget::WidgetUid,
    crate::widget_tree::CxWidgetExt,
    std::collections::{HashMap, VecDeque},
    std::sync::atomic::{AtomicU64, Ordering},
};

static SCRIPT_ASYNC_COUNTER: AtomicU64 = AtomicU64::new(1);

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
    target_uid: WidgetUid,
    id: ScriptAsyncId,
    result: ScriptValue,
}

struct ScriptToWidgetCallRequest {
    target_uid: WidgetUid,
    method: LiveId,
    caller_thread: ScriptThreadId,
    args: ScriptObjectRef,
}

struct ScriptToWidgetReturn {
    target_uid: WidgetUid,
    method: LiveId,
    result: ScriptValue,
}

struct WidgetToScriptCallRequest {
    target_uid: WidgetUid,
    me: ScriptValue,
    source: ScriptObjectRef,
    script_fn: ScriptFnRef,
    args: ScriptValue,
}

#[derive(Default)]
struct CxWidgetAsync {
    done: VecDeque<WidgetAsyncDone>,
    widget_to_script_calls: VecDeque<WidgetToScriptCallRequest>,
    script_to_widget_calls: VecDeque<ScriptToWidgetCallRequest>,
    pending_script_to_widget_returns: HashMap<usize, ScriptToWidgetReturn>,
    thread_map: HashMap<usize, (WidgetUid, ScriptAsyncId)>,
    ui_handle_type: Option<ScriptHandleType>,
    global_ui_root_uid: WidgetUid,
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
        if self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_type
            .is_none()
        {
            register_ui_handle(self);
        }

        let ui_type = self
            .cx_mut()
            .global::<CxWidgetAsync>()
            .ui_handle_type
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
        let result = self.call_with_args_object_with_me(script_fn.clone().into(), call_args, me);

        let thread = self.bx.threads.cur_ref();
        if thread.is_paused() {
            let thread_id = thread.thread_id();
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: Some(thread_id),
            });
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .thread_map
                .insert(thread_id.to_index(), (target_uid, async_id));
            ScriptAsyncResult::Pending
        } else {
            script_async.calls.push(ScriptAsyncCall {
                id: async_id,
                method: from_method,
                me,
                thread_id: None,
            });
            self.cx_mut()
                .global::<CxWidgetAsync>()
                .done
                .push_back(WidgetAsyncDone {
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
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .widget_to_script_calls
            .push_back(WidgetToScriptCallRequest {
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
        self.cx_mut()
            .global::<CxWidgetAsync>()
            .script_to_widget_calls
            .push_back(ScriptToWidgetCallRequest {
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
        self.with_vm(|vm| {
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
        self.with_vm(|vm| {
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
        self.with_vm(|vm| {
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
        self.with_vm(|vm| {
            vm.widget_to_script_call(target_uid, me, source, script_fn, args);
        });
    }
}

fn register_ui_handle(vm: &mut ScriptVm) {
    if vm
        .cx_mut()
        .global::<CxWidgetAsync>()
        .ui_handle_type
        .is_some()
    {
        return;
    }

    let ui_type = vm.new_handle_type(id_lut!(ui));

    vm.set_handle_getter(ui_type, move |vm, pself, prop| {
        if let Some(handle) = pself.as_handle() {
            let Some(parent_uid) = vm
                .downcast_handle_gc::<CxWidgetHandleGc>(handle)
                .map(|gc| gc.uid)
            else {
                return script_err_not_found!(vm.trap(), "invalid ui handle");
            };

            let child_ref = vm.with_cx(|cx| cx.widget_tree().find_flood(parent_uid, &[prop]));
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
            if let Some(pending) = vm
                .cx_mut()
                .global::<CxWidgetAsync>()
                .pending_script_to_widget_returns
                .remove(&caller_thread.to_index())
            {
                if pending.target_uid == uid && pending.method == method {
                    return pending.result;
                }
                vm.cx_mut()
                    .global::<CxWidgetAsync>()
                    .pending_script_to_widget_returns
                    .insert(caller_thread.to_index(), pending);
            }

            vm.enqueue_script_to_widget_call(uid, method, args.into());
            return NIL;
        }

        script_err_not_found!(vm.trap(), "invalid ui handle for method call")
    });

    vm.cx_mut().global::<CxWidgetAsync>().ui_handle_type = Some(ui_type);
}

fn on_widget_script_thread_completed(
    cx: &mut Cx,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    cx.global::<CxWidgetAsync>()
        .pending_script_to_widget_returns
        .remove(&thread_id.to_index());

    let Some((target_uid, async_id)) = cx
        .global::<CxWidgetAsync>()
        .thread_map
        .remove(&thread_id.to_index())
    else {
        return false;
    };

    cx.global::<CxWidgetAsync>()
        .done
        .push_back(WidgetAsyncDone {
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
            cx.with_vm(|vm| {
                if req.script_fn.as_object() != ScriptObject::ZERO {
                    let ui_handle = vm.build_ui_handle_for_uid(req.target_uid);
                    let call_args = vm.make_call_args_object_with_context(
                        req.source.as_object(),
                        ui_handle,
                        req.args,
                    );
                    let _ = vm.call_with_args_object_with_me(
                        req.script_fn.clone().into(),
                        call_args,
                        req.me,
                    );
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
            let ret = cx.with_vm_thread(req.caller_thread, |vm| {
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
                    req.caller_thread.to_index(),
                    ScriptToWidgetReturn {
                        target_uid: req.target_uid,
                        method: req.method,
                        result: ret,
                    },
                );
            cx.queue_script_thread_resume(req.caller_thread);
            continue;
        }

        let done = cx.global::<CxWidgetAsync>().done.pop_front();
        if let Some(done) = done {
            progressed = true;
            cx.with_vm(|vm| {
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
    cx.add_script_task_on_thread_completed_hook(on_widget_script_thread_completed);
    cx.add_script_task_pump_hook(pump_widget_async);
}
