//! Opcode function and method call operations
//!
//! This module contains handle functions for function calls, method calls,
//! function definitions, and related operations.

use crate::function::*;
use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::pod::ScriptPodOffset;
use crate::thread::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::*;
use crate::*;

impl<'a> ScriptVm<'a> {
    // Calling handlers

    pub(crate) fn handle_call_args(&mut self) {
        let mut fnobj = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        // `mod(x, y)` is the modulo function; `mod` itself stays the module
        // root (see `define_mod_call`).
        if fnobj == self.bx.heap.modules.into() {
            fnobj = self.bx.code.builtins.mod_call.into();
        }
        if let Some(ty) = self.bx.heap.pod_type(fnobj) {
            let pod = self.bx.heap.new_pod(ty);
            self.bx.threads.cur().push_me(ScriptMe::Pod {
                pod,
                offset: ScriptPodOffset::default(),
            });
        } else {
            let scope = self.bx.heap.new_with_proto(fnobj);
            self.bx.heap.clear_object_deep(scope);
            self.bx.threads.cur().push_me(ScriptMe::Call {
                args: scope,
                sself: None,
                target: CallTarget::Resolve(None),
            });
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // Only the final poll publishes the result and applies the original call's pop-to-me.
    fn finish_native_call(
        &mut self, ip: ScriptIp, args: ScriptObject,
        native: OpcodeNative, opargs: OpcodeArgs, ret: ScriptValue, traced: Option<String>,
    ) {
        if self.bx.heap.allocation_error_pending {
            // Leave the builder and its arguments for the allocation-error unwind.
            let thread = self.bx.threads.cur();
            thread.is_paused = false;
            thread.trap.take_on();
            thread.trap.ip = ip;
            return;
        }
        self.propagate_native_failure();
        let thread = self.bx.threads.cur();
        let trap = thread.trap.get_on();
        if matches!(trap, Some(ScriptTrapOn::Pause | ScriptTrapOn::CallBack { .. })) {
            let Some(ScriptMe::Call { target, .. }) = thread.mes.last_mut() else {
                self.bail("call builder missing after native invocation");
                return;
            };
            if let Some(ScriptTrapOn::CallBack { scope, ip: callee }) = trap {
                if matches!(target, CallTarget::Resolve(_)) {
                    *target = CallTarget::Admitted(native);
                }
                thread.trap.take_on();
                thread.is_paused = false;
                self.enter_callback(scope, callee, Some(ip), opargs);
            } else {
                // A native's own pause retries the original call, even after a callback.
                let method = match native {
                    OpcodeNative::Function(_) => None,
                    OpcodeNative::TypeCall { method, .. } => Some(method),
                };
                *target = CallTarget::Resolve(method);
                self.force_budget_sample();
            }
            return;
        }
        self.force_budget_sample();
        self.bx.threads.cur().is_paused = false;
        if let Some(verb) = traced {
            crate::inspect_query::trace_record(&self.bx, verb, ip, args, ret);
        }
        let thread = self.bx.threads.cur();
        thread.mes.pop();
        thread.trap.ip = ip;
        thread.push_stack_value(ret);
        if ret.as_object() != Some(args) {
            self.bx.heap.free_object_if_unreffed(args);
        }
        thread.trap.goto_next();
        if opargs.is_pop_to_me() && !thread.has_execution_limit_exceeded()
            && !self.bx.heap.allocation_error_pending
        {
            self.pop_to_me();
        }
    }

    pub(crate) fn handle_call_exec(&mut self, opargs: OpcodeArgs) -> bool {
        let Some(me) = self.bx.threads.cur_ref().mes.last() else {
            self.bail("mes empty in handle_call_exec");
            return false;
        };

        let (args, sself, method) = match *me {
            ScriptMe::Call {
                args,
                sself,
                target: CallTarget::Resolve(method),
            } => (args, sself, method),
            ScriptMe::Pod { pod, offset } => {
                self.bx.threads.cur().mes.pop();
                self.bx
                    .heap
                    .pod_check_arg_total(pod, offset, self.bx.threads.cur().trap.pass());
                self.bx.threads.cur().push_stack_unchecked(pod.into());
                self.bx.threads.cur().trap.goto_next();
                return true; // Pod: caller should handle pop_to_me
            }
            _ => {
                self.bx.threads.cur().mes.pop();
                self.bail("unexpected me type in handle_call_exec");
                return false;
            }
        };

        // Argument expressions have run normally. A reported call target carries
        // its original result through CALL_EXEC, including the usual stack cleanup.
        let target = self.bx.heap.proto(args);
        if target.is_reported_err() {
            self.bx.threads.cur().mes.pop();
            self.bx.threads.cur().push_stack_unchecked(target);
            self.bx.heap.free_object_if_unreffed(args);
            self.bx.threads.cur().trap.goto_next();
            return true;
        }

        if let Some(sself) = sself {
            self.bx
                .heap
                .force_value_in_map(args, id!(self).into(), sself);
        }
        // Keep the call builder, arguments and IP intact for the allocation-error unwind.
        if self.bx.heap.allocation_error_pending {
            return false;
        }
        self.bx.heap.set_object_deep(args);
        self.bx.heap.set_object_storage_auto(args);

        // Dynamic dispatch via call: when handle_method_call_args set
        // method, there is no fn proto — dispatch through the type's
        // registered call function instead.
        if let Some(method) = method {
            let sself = sself.unwrap_or(NIL);
            let ty = sself.value_type().to_redux();
            let handler = self.bx.code.native.borrow().type_call_ids.get(ty.to_index()).copied().flatten();
            let Some(handler) = handler else {
                self.bx.threads.cur().mes.pop();
                let value = script_err_not_found!(
                    self.bx.threads.cur_ref().trap,
                    "no call handler for method {:?} on type {:?}",
                    method, ty
                );
                self.bx.threads.cur().push_stack_unchecked(value);
                self.bx.threads.cur().trap.goto_next();
                return true;
            };
            self.poll_opcode_native(args, OpcodeNative::TypeCall { handler, ty, method }, opargs);
            return false;
        }

        if let Some(fnptr) = self.bx.heap.parent_as_fn(args) {
            match fnptr {
                ScriptFnPtr::Native(ni) => {
                    self.poll_opcode_native(args, OpcodeNative::Function(ni), opargs);
                    false
                }
                ScriptFnPtr::Script(sip) => {
                    self.bx.threads.cur().mes.pop();
                    let call = CallFrame {
                        bases: self.bx.threads.cur_ref().new_bases(),
                        args: opargs,
                        return_ip: Some(ScriptIp {
                            index: self.bx.threads.cur_ref().trap.ip.index + 1,
                            body: self.bx.threads.cur_ref().trap.ip.body,
                        }),
                        prev_slot_base: self.bx.threads.cur_ref().slot_base,
                        body_root: false,
                        native_cont: false,
                    };
                    if !self.bx.threads.cur().push_call_frame(call) {
                        return false;
                    }
                    self.bx.threads.cur().push_scope(args);
                    self.bx.threads.cur().trap.ip = sip;
                    return false; // Script: skip pop_to_me, RETURN will handle it via call.args
                }
            }
        } else {
            self.bx.threads.cur().mes.pop();
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "call target is not a function (got {:?})",
                self.bx.heap.proto(args).value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
            self.bx.threads.cur().trap.goto_next();
            return true; // Error: caller should handle pop_to_me
        }
    }

    pub(crate) fn poll_opcode_native(&mut self, args: ScriptObject, native: OpcodeNative, opargs: OpcodeArgs) {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let (ret, traced) = match native {
            OpcodeNative::Function(id) => {
                let func = self.bx.code.native.borrow().functions[id.index as usize].view();
                self.bx.threads.cur().is_paused = true;
                // SAFETY: the callable is borrowed by index anew for this invocation.
                let ret = self.invoke_native(NativeInvocation::Opcode, |vm| unsafe { func.call(vm, args) });
                (ret, None)
            }
            OpcodeNative::TypeCall { handler, ty, method } => {
                let call = self.bx.code.native.borrow().call_handlers[handler].view();
                self.bx.threads.cur().is_paused = true;
                let traced = if crate::inspect_query::trace_on() {
                    let ty = crate::inspect_query::handle_type_name(&self.bx.code.native.borrow(), ty);
                    crate::inspect_query::trace_wants(&ty, method)
                } else {
                    None
                };
                // SAFETY: the append-only handler table owns the closure throughout this call.
                let ret = self.invoke_native(NativeInvocation::Opcode, |vm| unsafe { call.call(vm, args, method) });
                (ret, traced)
            }
        };
        self.finish_native_call(ip, args, native, opargs, ret, traced)
    }

    pub(crate) fn handle_method_call_args(&mut self) -> bool {
        let method = self.bx.threads.cur().pop_stack_value();
        let sself = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fnobj = if sself.is_reported_err() {
            sself
        } else if let Some(obj) = sself.as_object() {
            self.bx.heap.object_method(obj, method, NoTrap)
        } else if let Some(pod) = sself.as_pod() {
            self.bx.heap.pod_method(pod, method, NoTrap)
        } else {
            NIL
        };

        let args = if fnobj.is_reported_err() {
            self.bx.heap.new_with_proto(fnobj)
        } else if fnobj.is_err() || fnobj == NIL {
            let method = method.as_id().unwrap_or(id!());
            let type_index = sself.value_type().to_redux();
            let (found_method, has_call) = {
                let native = self.bx.code.native.borrow();
                let type_entry = &native.type_table[type_index.to_index()];
                let found = type_entry.get(&method).copied();
                let has_call = native
                    .type_call_ids
                    .get(type_index.to_index())
                    .map(|c| c.is_some())
                    .unwrap_or(false);
                (found, has_call)
            };
            if let Some(method_ptr) = found_method {
                self.bx.heap.new_with_proto(method_ptr.into())
            } else if has_call {
                // Dynamic dispatch via call: create an empty vec2 args object.
                // The call function will be invoked in handle_call_exec with
                // the method name and collected arguments.
                let args = self.bx.heap.new_with_proto(NIL);
                self.bx.heap.set_object_storage_vec2(args);
                self.bx.heap.clear_object_deep(args);

                self.bx.threads.cur().push_me(ScriptMe::Call {
                    args,
                    sself: Some(sself),
                    target: CallTarget::Resolve(Some(method)),
                });
                self.bx.threads.cur().trap.goto_next();
                return false;
            } else {
                let error = script_err_not_found!(
                    self.bx.threads.cur_ref().trap,
                    "method {:?} not found on {}",
                    method,
                    type_index
                );
                self.bx.heap.new_with_proto(error)
            }
        } else {
            if let Some(ty) = self.bx.heap.pod_type(fnobj) {
                let pod = self.bx.heap.new_pod(ty);
                self.bx.threads.cur().push_me(ScriptMe::Pod {
                    pod,
                    offset: ScriptPodOffset::default(),
                });
                self.bx.threads.cur().trap.goto_next();
                return true; // Pod: caller should return early, skip pop_to_me
            }
            self.bx.heap.new_with_proto(fnobj)
        };
        self.bx.heap.clear_object_deep(args);

        self.bx.threads.cur().push_me(ScriptMe::Call {
            args,
            sself: Some(sself),
            target: CallTarget::Resolve(None),
        });
        self.bx.threads.cur().trap.goto_next();
        false
    }

    // Fn def handlers

    pub(crate) fn handle_fn_args(&mut self) {
        let Some(&scope) = self.bx.threads.cur_ref().scopes.last() else {
            self.bail("scopes empty in handle_fn_args");
            return;
        };
        let me = self.bx.heap.new_with_proto(scope.into());
        self.bx.heap.set_object_storage_vec2(me);
        self.bx.heap.clear_object_deep(me);
        self.bx.threads.cur().push_me(ScriptMe::Object(me));
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_fn_let_args(&mut self) {
        let id = self
            .bx
            .threads
            .cur()
            .pop_stack_value()
            .as_id()
            .unwrap_or(id!());
        let Some(&scope) = self.bx.threads.cur_ref().scopes.last() else {
            self.bail("scopes empty in handle_fn_let_args");
            return;
        };
        let me = self.bx.heap.new_with_proto(scope.into());
        self.bx.heap.set_object_storage_vec2(me);
        self.bx.heap.clear_object_deep(me);
        self.bx.threads.cur().push_me(ScriptMe::Object(me));
        self.bx
            .threads
            .cur()
            .def_scope_value(&mut self.bx.heap, id, me.into());
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_fn_arg_dyn(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let id = self
            .bx
            .threads
            .cur()
            .pop_stack_value()
            .as_id()
            .unwrap_or(id!());

        let Some(me) = self.bx.threads.cur_ref().mes.last() else {
            self.bail("mes empty in handle_fn_arg_dyn");
            return;
        };
        match me {
            ScriptMe::Call { .. } | ScriptMe::Array(_) | ScriptMe::Pod { .. } => {
                script_err_unexpected!(
                    self.bx.threads.cur_ref().trap,
                    "FN_ARG_DYN for {:?}: expected Object context on stack",
                    id
                );
            }
            ScriptMe::Object(obj) => {
                if id == id!(self) && self.bx.heap.vec_len(*obj) == 0 {
                    // ignore self as first argument
                } else {
                    self.bx.heap.set_value(*obj, id.into(), value, NoTrap);
                }
            }
        };
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_fn_arg_typed(&mut self, opargs: OpcodeArgs) {
        let _value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let ty = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self
            .bx
            .threads
            .cur()
            .pop_stack_value()
            .as_id()
            .unwrap_or(id!());
        let Some(me) = self.bx.threads.cur_ref().mes.last() else {
            self.bail("mes empty in handle_fn_arg_typed");
            return;
        };
        match me {
            ScriptMe::Call { .. } | ScriptMe::Array(_) | ScriptMe::Pod { .. } => {
                script_err_unexpected!(
                    self.bx.threads.cur_ref().trap,
                    "FN_ARG_TYPED for {:?}: expected Object context on stack",
                    id
                );
            }
            ScriptMe::Object(obj) => {
                self.bx.heap.set_value(*obj, id.into(), ty, NoTrap);
            }
        };
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_fn_body_dyn(&mut self, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        if let Some(me) = self.bx.threads.cur().mes.pop() {
            match me {
                ScriptMe::Call { .. } | ScriptMe::Array(_) | ScriptMe::Pod { .. } => {
                    script_err_unexpected!(
                        self.bx.threads.cur_ref().trap,
                        "FN_BODY_DYN: expected Object context for function body, got {:?}",
                        me
                    );
                    self.bx.threads.cur().push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    self.bx.heap.set_fn(
                        obj,
                        ScriptFnPtr::Script(ScriptIp {
                            body: self.bx.threads.cur_ref().trap.ip.body,
                            index: (self.bx.threads.cur_ref().trap.ip() + 1),
                        }),
                    );
                    self.bx.threads.cur().push_stack_unchecked(obj.into());
                }
            };
            self.bx.threads.cur().trap.goto_rel(jump_over_fn);
        } else {
            script_err_unexpected!(
                self.bx.threads.cur_ref().trap,
                "FN_BODY_DYN: me stack is empty (function definition without arguments block)"
            );
            self.bx.threads.cur().push_stack_unchecked(NIL);
            self.bx.threads.cur().trap.goto_next();
        }
    }

    pub(crate) fn handle_fn_body_typed(&mut self, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        let _return_type = self.bx.threads.cur().pop_stack_value();
        if let Some(me) = self.bx.threads.cur().mes.pop() {
            match me {
                ScriptMe::Call { .. } | ScriptMe::Array(_) | ScriptMe::Pod { .. } => {
                    script_err_unexpected!(
                        self.bx.threads.cur_ref().trap,
                        "FN_BODY_TYPED: expected Object context for typed function body, got {:?}",
                        me
                    );
                    self.bx.threads.cur().push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    self.bx.heap.set_fn(
                        obj,
                        ScriptFnPtr::Script(ScriptIp {
                            body: self.bx.threads.cur_ref().trap.ip.body,
                            index: (self.bx.threads.cur_ref().trap.ip() + 1),
                        }),
                    );
                    self.bx.threads.cur().push_stack_unchecked(obj.into());
                }
            };
            self.bx.threads.cur().trap.goto_rel(jump_over_fn);
        } else {
            script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_BODY_TYPED: me stack is empty (typed function definition without arguments block)");
            self.bx.threads.cur().push_stack_unchecked(NIL);
            self.bx.threads.cur().trap.goto_next();
        }
    }
}
