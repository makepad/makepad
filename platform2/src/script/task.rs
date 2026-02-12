use crate::script::vm::*;
use crate::*;
use makepad_script::id;
use makepad_script::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

pub type CxScriptTaskOnThreadCompletedHook = fn(&mut Cx, ScriptThreadId, ScriptValue) -> bool;
pub type CxScriptTaskPumpHook = fn(&mut Cx) -> bool;

#[derive(Default, Clone)]
pub struct CxScriptTaskHooks {
    pub on_thread_completed: Vec<CxScriptTaskOnThreadCompletedHook>,
    pub pump: Vec<CxScriptTaskPumpHook>,
}

#[derive(Clone)]
pub struct CxScriptTask {
    pub start_task: Option<ScriptFnRef>,
    pub handle: ScriptHandle,
    pub queue: ScriptArrayRef,
    pub max_depth: usize,
    pub ended: bool,
    pub send_pause: VecDeque<ScriptThreadId>,
    pub recv_pause: VecDeque<ScriptThreadId>,
}

#[derive(Default)]
pub struct CxScriptTasks {
    pub tasks: Rc<RefCell<Vec<CxScriptTask>>>,
    pub pending_resumes: VecDeque<ScriptThreadId>,
    pub hooks: CxScriptTaskHooks,
}

// this is a UI-thread pipe
pub struct CxScriptTaskGc {
    pub tasks: Rc<RefCell<Vec<CxScriptTask>>>,
    pub handle: ScriptHandle,
}

impl ScriptHandleGc for CxScriptTaskGc {
    fn gc(&mut self) {
        self.tasks.borrow_mut().retain(|v| v.handle != self.handle)
    }
    fn set_handle(&mut self, handle: ScriptHandle) {
        self.handle = handle
    }
}

impl Cx {
    pub fn add_script_task_on_thread_completed_hook(
        &mut self,
        hook: CxScriptTaskOnThreadCompletedHook,
    ) {
        if !self
            .script_data
            .tasks
            .hooks
            .on_thread_completed
            .iter()
            .any(|v| (*v as usize) == (hook as usize))
        {
            self.script_data.tasks.hooks.on_thread_completed.push(hook);
        }
    }

    pub fn add_script_task_pump_hook(&mut self, hook: CxScriptTaskPumpHook) {
        if !self
            .script_data
            .tasks
            .hooks
            .pump
            .iter()
            .any(|v| (*v as usize) == (hook as usize))
        {
            self.script_data.tasks.hooks.pump.push(hook);
        }
    }

    pub fn queue_script_thread_resume(&mut self, thread_id: ScriptThreadId) {
        self.script_data.tasks.pending_resumes.push_back(thread_id);
    }

    pub fn set_script_task_trace(&mut self, _enabled: bool) {
    }

    fn run_script_task_thread_completed_hooks(
        &mut self,
        thread_id: ScriptThreadId,
        result: ScriptValue,
    ) -> bool {
        let hooks = self.script_data.tasks.hooks.on_thread_completed.clone();
        let mut consumed = false;
        for hook in hooks {
            consumed |= hook(self, thread_id, result);
        }
        consumed
    }

    fn run_script_task_pump_hooks(&mut self) -> bool {
        let hooks = self.script_data.tasks.hooks.pump.clone();
        let mut progressed = false;
        for hook in hooks {
            progressed |= hook(self);
        }
        progressed
    }

    pub(crate) fn handle_script_tasks(&mut self) {
        loop {
            let mut progressed = false;

            let mut next_thread = None;
            let mut start_task = None;

            if let Some(thread_id) = self.script_data.tasks.pending_resumes.pop_front() {
                next_thread = Some(thread_id);
            } else {
                let mut tasks = self.script_data.tasks.tasks.borrow_mut();
                for task in tasks.iter_mut() {
                    // alright lets check each channels array len and if they are waiting
                    // ifso we call that thread
                    let queue = task.queue.as_array();

                    let queue_len = self.script_vm.as_ref().unwrap().heap.array_len(queue);
                    if let Some(st) = task.start_task.take() {
                        start_task = Some((st, task.handle));
                        break;
                    }
                    if task.recv_pause.len() > 0 && queue_len > 0 {
                        next_thread = task.recv_pause.pop_back();
                        break;
                    }
                    if task.send_pause.len() > 0 && queue_len < task.max_depth {
                        next_thread = task.send_pause.pop_back();
                        break;
                    }
                }
            }

            // alright execute this thread
            if let Some((start_task, handle)) = start_task.take() {
                progressed = true;
                self.with_vm(|vm| {
                    vm.call(start_task.into(), &[handle.into()]);
                })
            } else if let Some(next_thread) = next_thread.take() {
                progressed = true;
                let result = self.with_vm_thread(next_thread, |vm| vm.resume());

                let is_paused = self
                    .script_vm
                    .as_ref()
                    .and_then(|bx| bx.threads.get(next_thread.to_index()))
                    .map(|thread| thread.is_paused())
                    .unwrap_or(true);

                if !is_paused {
                    self.run_script_task_thread_completed_hooks(next_thread, result);
                }
            }

            if self.run_script_task_pump_hooks() {
                progressed = true;
            }

            if !progressed {
                break;
            }
        }
    }
}

pub fn script_mod(vm: &mut ScriptVm) {
    let std = vm.module(id!(std));
    let task_type = vm.new_handle_type(id_lut!(task));

    for fn_id in [id_lut!(emit), id_lut!(end)] {
        vm.add_handle_method(task_type, fn_id, script_args_def!(), move |vm, args| {
            if let Some(handle) = script_value!(vm, args.self).as_handle() {
                let cx = vm.host.cx_mut();
                if let Some(chan) = cx
                    .script_data
                    .tasks
                    .tasks
                    .borrow_mut()
                    .iter_mut()
                    .find(|v| v.handle == handle)
                {
                    let array_len = vm.bx.heap.array_len(chan.queue.as_array());

                    if chan.max_depth == 0 || array_len < chan.max_depth {
                        let vec_len = vm.bx.heap.vec_len(args.into());
                        if vec_len == 0 {
                            vm.bx.heap.array_push(
                                chan.queue.as_array(),
                                NIL,
                                vm.bx.threads.cur().trap.pass(),
                            );
                        } else if vec_len == 1 {
                            let value =
                                vm.bx
                                    .heap
                                    .vec_value(args, 0, vm.bx.threads.cur().trap.pass());
                            vm.bx.heap.array_push(
                                chan.queue.as_array(),
                                value,
                                vm.bx.threads.trap(),
                            );
                        } else {
                            vm.bx.heap.array_push(
                                chan.queue.as_array(),
                                args.into(),
                                vm.bx.threads.trap(),
                            );
                        }
                        if fn_id == id!(end) {
                            chan.ended = true;
                        }
                        return ((array_len + 1) as f64).into();
                    } else {
                        if chan.send_pause.len() > 100 {
                            return script_err_limit!(
                                vm.bx.threads.trap(),
                                "too many paused calls"
                            );
                        }
                        chan.send_pause.push_front(vm.bx.threads.cur().pause());
                        return NIL;
                    }
                }
            }
            NIL
        });
    }
    for fn_id in [id_lut!(next), id_lut!(last)] {
        vm.add_handle_method(task_type, fn_id, script_args_def!(), move |vm, args| {
            // lets find the channel
            if let Some(handle) = script_value!(vm, args.self).as_handle() {
                let cx = vm.host.cx_mut();
                if let Some(task) = cx
                    .script_data
                    .tasks
                    .tasks
                    .borrow_mut()
                    .iter_mut()
                    .find(|v| v.handle == handle)
                {
                    if let Some(value) = vm.bx.heap.array_pop_front_option(task.queue.as_array()) {
                        if fn_id == id!(next) || fn_id == id!(last) && task.ended {
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

    vm.set_handle_getter(task_type, |vm, pself, prop| {
        // lets find the channel
        if prop == id!(queue) {
            if let Some(handle) = pself.as_handle() {
                let cx = vm.host.cx_mut();
                if let Some(chan) = cx
                    .script_data
                    .tasks
                    .tasks
                    .borrow_mut()
                    .iter_mut()
                    .find(|v| v.handle == handle)
                {
                    return chan.queue.as_array().into();
                }
            }
        }
        script_err_not_found!(vm.trap(), "invalid task prop")
    });

    vm.add_method(
        std,
        id_lut!(task),
        script_args_def!(start_fn_or_depth = NIL),
        move |vm, args| {
            // lets make a new channel
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

            let cx = vm.host.cx_mut();
            let handle_gc = CxScriptTaskGc {
                tasks: cx.script_data.tasks.tasks.clone(),
                handle: ScriptHandle::ZERO,
            };
            let handle = vm.bx.heap.new_handle(task_type, Box::new(handle_gc));
            let array = vm.bx.heap.new_array();
            let queue = vm.bx.heap.new_array_ref(array);
            cx.script_data.tasks.tasks.borrow_mut().push(CxScriptTask {
                max_depth,
                start_task,
                handle,
                ended: false,
                recv_pause: Default::default(),
                send_pause: Default::default(),
                queue,
            });

            handle.into()
        },
    );
}
