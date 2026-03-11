use crate::{vm, ScriptStd, ScriptVmStdExt};
use makepad_script::id;
use makepad_script::*;
use std::any::Any;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

pub type ScriptTaskOnThreadCompletedHook = fn(&mut dyn Any, ScriptThreadId, ScriptValue) -> bool;
pub type ScriptTaskPumpHook = fn(&mut dyn Any) -> bool;

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

pub fn add_script_task_on_thread_completed_hook(std: &mut ScriptStd, hook: ScriptTaskOnThreadCompletedHook) {
    if !std
        .data
        .tasks
        .hooks
        .on_thread_completed
        .iter()
        .any(|v| (*v as usize) == (hook as usize))
    {
        std.data.tasks.hooks.on_thread_completed.push(hook);
    }
}

pub fn add_script_task_pump_hook(std: &mut ScriptStd, hook: ScriptTaskPumpHook) {
    if !std
        .data
        .tasks
        .hooks
        .pump
        .iter()
        .any(|v| (*v as usize) == (hook as usize))
    {
        std.data.tasks.hooks.pump.push(hook);
    }
}

pub fn queue_script_thread_resume(std: &mut ScriptStd, thread_id: ScriptThreadId) {
    std.data.tasks.pending_resumes.push_back(thread_id);
}

pub fn set_script_task_trace(_std: &mut ScriptStd, _enabled: bool) {}

fn run_script_task_thread_completed_hooks(
    host: &mut dyn Any,
    std: &mut ScriptStd,
    thread_id: ScriptThreadId,
    result: ScriptValue,
) -> bool {
    let hooks = std.data.tasks.hooks.on_thread_completed.clone();
    let mut consumed = false;
    for hook in hooks {
        consumed |= hook(host, thread_id, result);
    }
    consumed
}

fn run_script_task_pump_hooks(host: &mut dyn Any, std: &mut ScriptStd) -> bool {
    let hooks = std.data.tasks.hooks.pump.clone();
    let mut progressed = false;
    for hook in hooks {
        progressed |= hook(host);
    }
    progressed
}

pub fn handle_script_tasks<H: Any>(
    host: &mut H,
    std: &mut ScriptStd,
    script_vm: &mut Option<Box<ScriptVmBase>>,
) {
    loop {
        let mut progressed = false;

        let mut next_thread = None;
        let mut start_task = None;

        if let Some(thread_id) = std.data.tasks.pending_resumes.pop_front() {
            next_thread = Some(thread_id);
        } else {
            let mut tasks = std.data.tasks.tasks.borrow_mut();
            for task in tasks.iter_mut() {
                let queue = task.queue.as_array();
                let queue_len = script_vm.as_ref().unwrap().heap.array_len(queue);
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
            vm::with_vm(host, std, script_vm, |vm| {
                vm.call(start_task.into(), &[handle.into()]);
            });
        } else if let Some(next_thread) = next_thread.take() {
            progressed = true;
            let result = vm::with_vm_thread(host, std, script_vm, next_thread, |vm| vm.resume());

            let is_paused = script_vm
                .as_ref()
                .and_then(|bx| bx.threads.get(next_thread.to_index()))
                .map(|thread| thread.is_paused())
                .unwrap_or(true);

            if !is_paused {
                run_script_task_thread_completed_hooks(host, std, next_thread, result);
            }
        }

        if run_script_task_pump_hooks(host, std) {
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
                    let array_len = vm.bx.heap.array_len(queue);

                    if task.max_depth == 0 || array_len < task.max_depth {
                        let value = {
                            let vec_len = vm.bx.heap.vec_len(args.into());
                            if vec_len == 0 {
                                NIL
                            } else if vec_len == 1 {
                                vm.bx
                                    .heap
                                    .vec_value(args, 0, vm.bx.threads.cur().trap.pass())
                            } else {
                                vm.bx.heap.set_reffed(args);
                                args.into()
                            }
                        };
                        vm.bx.heap.array_push(queue, value, vm.bx.threads.trap());
                        if end_on_send {
                            task.ended = true;
                        }
                        return ((array_len + 1) as f64).into();
                    }

                    if task.send_pause.len() > 100 {
                        return script_err_limit!(vm.bx.threads.trap(), "too many paused calls");
                    }
                    task.send_pause.push_front(vm.bx.threads.cur().pause());
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
                    if let Some(value) = vm.bx.heap.array_pop_front_option(task.queue.as_array()) {
                        if !wait_for_end || task.ended {
                            return value;
                        }
                    }
                    if task.ended {
                        return NIL;
                    }
                    if task.recv_pause.len() > 100 {
                        return script_err_limit!(vm.bx.threads.trap(), "too many paused calls");
                    }
                    task.recv_pause.push_front(vm.bx.threads.cur().pause());
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
        let handle = vm.bx.heap.new_handle(handle_type, Box::new(handle_gc));
        let array = vm.bx.heap.new_array();
        let queue = vm.bx.heap.new_array_ref(array);
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
            let (start_task, max_depth) = if vm.bx.heap.is_fn(start_fn_or_depth.into()) {
                (
                    Some(
                        vm.bx
                            .heap
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
            } else if vm.bx.heap.is_fn(start_fn.into()) {
                Some(vm.bx.heap.new_fn_ref(start_fn.as_object().unwrap()))
            } else {
                return script_err_wrong_value!(vm.trap(), "std.promise expects fn or nil");
            };
            create_task_handle(vm, promise_type, start_task, 1)
        },
    );
}
