use makepad_relax_api::relax_api;
use crate::{vm, ScriptStd, ScriptVmStdExt};
use makepad_script::id;
use makepad_script::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptTaskOnThreadCompletedHook {
    context: *mut u8,
    call: unsafe extern "C-unwind" fn(
        *mut u8,
        &mut ScriptHostContext,
        ScriptThreadId,
        ScriptValue,
    ) -> bool,
}
#[relax_api(plain)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ScriptTaskPumpHook {
    context: *mut u8,
    call: unsafe extern "C-unwind" fn(*mut u8, &mut ScriptHostContext) -> bool,
}

impl ScriptTaskOnThreadCompletedHook {
    /// # Safety
    /// The callback must accept every host/thread/result supplied by this VM.
    /// Its context and code must remain valid until the host is dropped.
    pub unsafe fn new(
        context: *mut u8,
        call: unsafe extern "C-unwind" fn(
            *mut u8,
            &mut ScriptHostContext,
            ScriptThreadId,
            ScriptValue,
        ) -> bool,
    ) -> Self {
        Self { context, call }
    }
}
impl ScriptTaskPumpHook {
    /// # Safety
    /// The callback must accept the host supplied by this VM. Its context and
    /// code must remain valid until the host is dropped.
    pub unsafe fn new(
        context: *mut u8,
        call: unsafe extern "C-unwind" fn(*mut u8, &mut ScriptHostContext) -> bool,
    ) -> Self {
        Self { context, call }
    }
}

#[relax_api(handle, clone)]
#[derive(Default, Clone)]
pub struct ScriptTaskHooks {
    pub on_thread_completed: Vec<ScriptTaskOnThreadCompletedHook>,
    pub pump: Vec<ScriptTaskPumpHook>,
}

#[derive(Clone)]
pub struct ScriptTask {
    pub start_task: Option<ScriptFnRef>,
    pub handle: ScriptHandle,
    pub queue: ScriptArrayRef,
    pub max_depth: usize,
    pub ended: bool,
    pub send_pause: VecDeque<ScriptThreadId>,
    pub recv_pause: VecDeque<ScriptThreadId>,
}

#[relax_api(handle)]
#[derive(Default)]
pub struct ScriptTasks {
    pub tasks: Rc<RefCell<Vec<ScriptTask>>>,
    pub pending_resumes: VecDeque<ScriptThreadId>,
    pub hooks: ScriptTaskHooks,
}

pub struct ScriptTaskGc {
    pub tasks: Rc<RefCell<Vec<ScriptTask>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for ScriptTaskGc {
    fn gc(&mut self) {
        self.tasks.borrow_mut().retain(|v| v.handle != self.handle)
    }

    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle
    }
}

#[relax_api]
pub fn add_script_task_on_thread_completed_hook(
    std: &mut ScriptStd,
    hook: ScriptTaskOnThreadCompletedHook,
) {
    if !std
        .data
        .tasks
        .hooks
        .on_thread_completed
        .iter()
        .any(|v| v.context == hook.context && (v.call as usize) == (hook.call as usize))
    {
        std.data.tasks.hooks.on_thread_completed.push(hook);
    }
}

#[relax_api]
pub fn add_script_task_pump_hook(std: &mut ScriptStd, hook: ScriptTaskPumpHook) {
    if !std
        .data
        .tasks
        .hooks
        .pump
        .iter()
        .any(|v| v.context == hook.context && (v.call as usize) == (hook.call as usize))
    {
        std.data.tasks.hooks.pump.push(hook);
    }
}

#[relax_api]
pub fn queue_script_thread_resume(std: &mut ScriptStd, thread_id: ScriptThreadId) {
    std.data.tasks.pending_resumes.push_back(thread_id);
}

pub fn set_script_task_trace(_std: &mut ScriptStd, _enabled: bool) {}

fn run_script_task_thread_completed_hooks(
    host: &mut ScriptHostContext,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    let hooks = host
        .std_ref::<ScriptStd>()
        .unwrap()
        .data
        .tasks
        .hooks
        .on_thread_completed
        .clone();
    let mut consumed = false;
    for hook in hooks {
        consumed |= unsafe { (hook.call)(hook.context, host, thread_id, result) };
    }
    consumed
}

fn run_script_task_pump_hooks(host: &mut ScriptHostContext) -> bool {
    let hooks = host
        .std_ref::<ScriptStd>()
        .unwrap()
        .data
        .tasks
        .hooks
        .pump
        .clone();
    let mut progressed = false;
    for hook in hooks {
        progressed |= unsafe { (hook.call)(hook.context, host) };
    }
    progressed
}

#[relax_api]
pub fn handle_script_tasks(host: &mut ScriptHostContext) {
    loop {
        let mut progressed = false;

        let mut next_thread = None;
        let mut start_task = None;

        if let Some(thread_id) = host
            .std_mut::<ScriptStd>()
            .unwrap()
            .data
            .tasks
            .pending_resumes
            .pop_front()
        {
            next_thread = Some(thread_id);
        } else {
            let tasks = host
                .std_ref::<ScriptStd>()
                .unwrap()
                .data
                .tasks
                .tasks
                .clone();
            let mut tasks = tasks.borrow_mut();
            for task in tasks.iter_mut() {
                let queue = task.queue.as_array();
                let queue_len = host.vm_base().unwrap().heap_ref().array_len(queue);
                if let Some(st) = task.start_task.take() {
                    start_task = Some((st, task.handle));
                    break;
                }
                if !task.recv_pause.is_empty() && queue_len > 0 {
                    next_thread = task.recv_pause.pop_back();
                    break;
                }
                if !task.send_pause.is_empty() && queue_len < task.max_depth {
                    next_thread = task.send_pause.pop_back();
                    break;
                }
            }
        }

        if let Some((start_task, handle)) = start_task.take() {
            progressed = true;
            vm::with_vm(host, |vm| {
                vm.call(start_task.into(), &[handle.into()]);
            });
        } else if let Some(next_thread) = next_thread.take() {
            progressed = true;
            let result = vm::with_vm_thread(host, next_thread, |vm| vm.resume());

            let is_paused = host
                .vm_base()
                .and_then(|bx| bx.thread_is_paused(next_thread.to_index()))
                .unwrap_or(true);

            if !is_paused {
                run_script_task_thread_completed_hooks(host, next_thread, result);
            }
        }

        if run_script_task_pump_hooks(host) {
            progressed = true;
        }

        if !progressed {
            break;
        }
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    fn add_send_method(
        vm: &mut ScriptVm,
        handle_type: ScriptHandleType,
        fn_id: LiveId,
        end_on_send: bool,
    ) {
        vm.add_handle_method(handle_type, fn_id, script_args_def!(), move |vm, args| {
            if let Some(handle) = script_value!(vm, args.self).as_handle() {
                let tasks = vm.std_ref::<ScriptStd>().data.tasks.tasks.clone();
                let mut tasks = tasks.borrow_mut();
                if let Some(task) = tasks.iter_mut().find(|v| v.handle == handle) {
                    let queue = task.queue.as_array();
                    let array_len = vm.heap().array_len(queue);

                    if task.max_depth == 0 || array_len < task.max_depth {
                        let value = {
                            let vec_len = vm.heap().vec_len(args.into());
                            if vec_len == 0 {
                                NIL
                            } else if vec_len == 1 {
                                vm.vec_value_trap(args, 0)
                            } else {
                                vm.heap_mut().set_reffed(args);
                                args.into()
                            }
                        };
                        vm.array_push_trap(queue, value);
                        if end_on_send {
                            task.ended = true;
                        }
                        return ((array_len + 1) as f64).into();
                    }

                    if task.send_pause.len() > 100 {
                        return script_err_limit!(vm.trap(), "too many paused calls");
                    }
                    task.send_pause.push_front(vm.pause());
                    return NIL;
                }
            }
            NIL
        });
    }

    fn add_recv_method(
        vm: &mut ScriptVm,
        handle_type: ScriptHandleType,
        fn_id: LiveId,
        wait_for_end: bool,
    ) {
        vm.add_handle_method(handle_type, fn_id, script_args_def!(), move |vm, args| {
            if let Some(handle) = script_value!(vm, args.self).as_handle() {
                let tasks = vm.std_ref::<ScriptStd>().data.tasks.tasks.clone();
                let mut tasks = tasks.borrow_mut();
                if let Some(task) = tasks.iter_mut().find(|v| v.handle == handle) {
                    if let Some(value) = vm.heap_mut().array_pop_front_option(task.queue.as_array()) {
                        if !wait_for_end || task.ended {
                            return value;
                        }
                    }
                    if task.ended {
                        return NIL;
                    }
                    if task.recv_pause.len() > 100 {
                        return script_err_limit!(vm.trap(), "too many paused calls");
                    }
                    task.recv_pause.push_front(vm.pause());
                    return NIL;
                }
            }
            script_err_unexpected!(vm.trap(), "unexpected task state")
        });
    }

    fn add_queue_getter(vm: &mut ScriptVm, handle_type: ScriptHandleType) {
        vm.set_handle_getter(handle_type, |vm, pself, prop| {
            if prop == id!(queue) {
                if let Some(handle) = pself.as_handle() {
                    let tasks = vm.std_ref::<ScriptStd>().data.tasks.tasks.clone();
                    let mut tasks = tasks.borrow_mut();
                    if let Some(task) = tasks.iter_mut().find(|v| v.handle == handle) {
                        return task.queue.as_array().into();
                    }
                }
            }
            script_err_not_found!(vm.trap(), "invalid task prop")
        });
    }

    fn create_task_handle(
        vm: &mut ScriptVm,
        handle_type: ScriptHandleType,
        start_task: Option<ScriptFnRef>,
        max_depth: usize,
    ) -> ScriptValue {
        let tasks = vm.std_ref::<ScriptStd>().data.tasks.tasks.clone();
        let handle_gc = ScriptTaskGc {
            tasks: tasks.clone(),
            handle: ScriptHandle::ZERO,
        };
        let handle = vm.heap_mut().new_handle(handle_type, Box::new(handle_gc));
        let array = vm.heap_mut().new_array();
        let queue = vm.heap_mut().new_array_ref(array);
        tasks.borrow_mut().push(ScriptTask {
            max_depth,
            start_task,
            handle,
            ended: false,
            recv_pause: Default::default(),
            send_pause: Default::default(),
            queue,
        });
        handle.into()
    }

    let std_mod = vm.module(id!(std));
    let task_type = vm.new_handle_type(id_lut!(task));
    let promise_type = vm.new_handle_type(id_lut!(promise));

    add_send_method(vm, task_type, id_lut!(emit), false);
    add_send_method(vm, task_type, id_lut!(end), true);
    add_recv_method(vm, task_type, id_lut!(next), false);
    add_recv_method(vm, task_type, id_lut!(last), true);
    add_queue_getter(vm, task_type);

    add_send_method(vm, promise_type, id_lut!(resolve), true);
    add_recv_method(vm, promise_type, id_lut!(await), true);
    add_queue_getter(vm, promise_type);

    vm.add_method(
        std_mod,
        id_lut!(task),
        script_args_def!(start_fn_or_depth = NIL),
        move |vm, args| {
            let start_fn_or_depth = script_value!(vm, args.start_fn_or_depth);
            let (start_task, max_depth) = if vm.is_fn(start_fn_or_depth.into()) {
                (
                    Some(
                        vm.heap_mut()
                            .new_fn_ref(start_fn_or_depth.as_object().unwrap()),
                    ),
                    1,
                )
            } else {
                (None, start_fn_or_depth.as_f64().unwrap_or(0.0) as usize)
            };
            create_task_handle(vm, task_type, start_task, max_depth)
        },
    );

    vm.add_method(
        std_mod,
        id_lut!(promise),
        script_args_def!(start_fn = NIL),
        move |vm, args| {
            let start_fn = script_value!(vm, args.start_fn);
            let start_task = if start_fn.is_nil() {
                None
            } else if vm.is_fn(start_fn.into()) {
                Some(vm.heap_mut().new_fn_ref(start_fn.as_object().unwrap()))
            } else {
                return script_err_wrong_value!(vm.trap(), "std.promise expects fn or nil");
            };
            create_task_handle(vm, promise_type, start_task, 1)
        },
    );
}
