//! Opcode control flow operations
//!
//! This module contains handle functions for control flow: if/else statements,
//! for loops, return statements, range, is, and try/ok operations.

use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::thread::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::ScriptVm;
use crate::*;

impl<'a> ScriptVm<'a> {
    // IF handlers

    pub(crate) fn handle_if_test(&mut self, opargs: OpcodeArgs) {
        let test = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(test);
        if test {
            self.bx.threads.cur().trap.goto_next()
        } else {
            if opargs.is_need_nil() {
                self.bx.threads.cur().push_stack_unchecked(NIL);
            }
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        }
    }

    pub(crate) fn handle_if_else(&mut self, opargs: OpcodeArgs) {
        // NEED_NIL: the true arm left no value but the if's value is used
        if opargs.is_need_nil() {
            self.bx.threads.cur().push_stack_unchecked(NIL);
        }
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
    }

    /// The value of an if/match arm that leaves none (see the parser's
    /// close_if_else).
    pub(crate) fn handle_nil_arm(&mut self) {
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    // RETURN handlers

    pub(crate) fn handle_return(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let Some(call) = self.bx.threads.cur().calls.pop() else {
            self.bail("calls empty in handle_return");
            return;
        };
        // A fn that ended on a `for` statement: the fn (its args scope's proto, read before
        // the frame frees that scope) and where its loop is.
        let after_for = match opargs.after_for() {
            0 => None,
            back => {
                let at = self.bx.threads.cur_ref().trap.ip;
                let f = self.bx.threads.cur_ref().scopes.get(call.bases.scope).and_then(|&args| self.bx.heap.proto(args).as_object());
                Some((f, ScriptIp { body: at.body, index: at.index - back }))
            }
        };
        self.bx.threads.cur().slot_base = call.prev_slot_base;
        // A body run's last scope holds everything the body defined after
        // a shadowing let; keep it for host->script lookups. Only a body
        // root: pinning a host->fn call's scope would defeat its eager free
        // (one leaked scope per host tick until GC).
        if call.body_root {
            let end = self.bx.threads.cur_ref().scopes.last().copied();
            self.bx.threads.cur().root_end_scope = end.map(|s| self.bx.heap.new_object_ref(s));
        }
        self.bx
            .threads
            .cur()
            .truncate_bases(call.bases, &mut self.bx.heap);

        if let Some(ret) = call.return_ip {
            self.bx.threads.cur().trap.ip = ret;
            self.bx.threads.cur().push_stack_unchecked(value);
            if let Some((f, loop_at)) = after_for {
                self.after_for_result(f, loop_at, ret, call.args.is_pop_to_me());
            }
            if !self.bx.threads.cur_ref().has_execution_limit_exceeded() && call.args.is_pop_to_me()
            {
                self.pop_to_me();
            }
        } else {
            self.bx
                .threads
                .cur()
                .trap
                .set_on(Some(ScriptTrapOn::Return(value)));
        }
    }

    /// A fn whose last statement is a `for` statement returned nil to `ret` (its RETURN
    /// carried `OpcodeArgs::nil_after_for`): a `for` statement is a loop, not the list of its
    /// values. Nothing to say when the caller discards the result; when it uses it as a value,
    /// an error at that use names the fn `f` and its loop (at `loop_at`). `pops`: whether the
    /// call pops its result to `me`.
    #[cold]
    #[inline(never)]
    fn after_for_result(&mut self, f: Option<ScriptObject>, loop_at: ScriptIp, ret: ScriptIp, pops: bool) {
        let Some(use_at) = self.result_use(ret, pops) else { return };
        let name = f.and_then(|f| self.fn_name(f));
        let what = name.map_or_else(|| "this fn".to_string(), |n| format!("`{n}`"));
        let at = self.bx.code.ip_to_loc_plain(loop_at).map_or_else(String::new, |l| format!(" at {}:{}", l.file, l.line));
        let message = format!(
            "{what} returns nothing: its last statement is the `for`{at}, and a `for` statement is a loop, not a value. \
             Its result is used here; end it with `return for ..` to return the loop's list"
        );
        self.bx.threads.cur_ref().trap.push_err(ScriptValue::script_err_type_mismatch(use_at), &message, file!(), line!());
    }

    /// Where the value a call just returned to `ret` gets used, as the call it came from
    /// (the CALL just before a return ip); None when it is discarded. It is used unless it
    /// only passes on: over an if arm's jump to the join, as a fn's result to that fn's
    /// caller, or popped to `me` where there is none or `me` is an object body (a nil adds
    /// nothing to it). Popped into a call's args, a list or a pod, it is used.
    fn result_use(&self, mut ret: ScriptIp, mut pops: bool) -> Option<ScriptIp> {
        let thread = self.bx.threads.cur_ref();
        let bodies = self.bx.code.bodies.borrow();
        // The value is in frame `level - 1`, returned by the call just before `ret`.
        let mut level = thread.calls.len();
        let mut call_at = ScriptIp { body: ret.body, index: ret.index - 1 };
        loop {
            if pops {
                let lo = level.checked_sub(1).and_then(|f| thread.calls.get(f)).map_or(0, |c| c.bases.mes);
                let hi = thread.calls.get(level).map_or(thread.mes.len(), |c| c.bases.mes);
                return match thread.mes.get(lo..hi).and_then(|m| m.last()) {
                    None | Some(ScriptMe::Object(_)) => None,
                    Some(_) => Some(call_at),
                };
            }
            let code = bodies.get(ret.body as usize).and_then(|b| b.parser.opcodes.get(ret.index as usize));
            match code.and_then(|c| c.as_opcode()) {
                Some((Opcode::IF_ELSE, args)) => ret.index += args.to_u32(),
                Some((Opcode::POP_TO_ME, _)) => pops = true,
                Some((Opcode::RETURN, args)) if args.is_nil() => return None,
                Some((Opcode::RETURN, _)) => {
                    // The frame returns it; to the host, the host decides.
                    let frame = thread.calls.get(level.checked_sub(1)?)?;
                    ret = frame.return_ip?;
                    pops = frame.args.is_pop_to_me();
                    call_at = ScriptIp { body: ret.body, index: ret.index - 1 };
                    level -= 1;
                }
                _ => return Some(call_at),
            }
        }
    }

    /// A name `f` is bound to: in the scopes the caller sees, else where `f` was made.
    fn fn_name(&self, f: ScriptObject) -> Option<LiveId> {
        let heap = &self.bx.heap;
        let starts = [self.bx.threads.cur_ref().scopes.last().copied(), heap.proto(f).as_object()];
        for start in starts.into_iter().flatten() {
            let mut scope = Some(start);
            while let Some(s) = scope {
                let object = &heap.objects[s];
                let named = |key: ScriptValue, value: ScriptValue| (value.as_object() == Some(f)).then(|| key.as_id()).flatten();
                let found = object.vec.iter().find_map(|kv| named(kv.key, kv.value)).or_else(|| object.map_iter_ret(named));
                if found.is_some() {
                    return found;
                }
                scope = heap.proto(s).as_object();
            }
        }
        None
    }

    pub(crate) fn handle_return_if_err(&mut self, _opargs: OpcodeArgs) -> bool {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        if value.is_err() {
            let Some(call) = self.bx.threads.cur().calls.pop() else {
                self.bail("calls empty in handle_return_if_err");
                return true;
            };
            self.bx.threads.cur().slot_base = call.prev_slot_base;
            self.bx
                .threads
                .cur()
                .truncate_bases(call.bases, &mut self.bx.heap);
            if let Some(ret) = call.return_ip {
                self.bx.threads.cur().trap.ip = ret;
                self.bx.threads.cur().push_stack_unchecked(value);
                if !self.bx.threads.cur_ref().has_execution_limit_exceeded()
                    && call.args.is_pop_to_me()
                {
                    self.pop_to_me();
                }
            } else {
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Return(value)));
            }
            true
        } else {
            self.bx.threads.cur().trap.goto_next();
            false
        }
    }

    // For loop handlers

    pub(crate) fn handle_for_1(&mut self, opargs: OpcodeArgs) {
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_1 value_id not an id");
            return;
        };
        self.begin_for_loop(opargs.to_u32() as _, source, value_id, None, None);
    }

    pub(crate) fn handle_for_2(&mut self, opargs: OpcodeArgs) {
        // for k v in source: k = key (obj) or index (array/range), v = value
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_2 value_id not an id");
            return;
        };
        let Some(first_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_2 first_id not an id");
            return;
        };
        // Pass first_id as key_id - for objects it gets key, for arrays/ranges it gets index
        self.begin_for_loop(opargs.to_u32() as _, source, value_id, None, Some(first_id));
    }

    pub(crate) fn handle_for_3(&mut self, opargs: OpcodeArgs) {
        // for i k v in source: i = index, k = key, v = value (objects only)
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 value_id not an id");
            return;
        };
        let Some(key_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 key_id not an id");
            return;
        };
        let Some(index_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 index_id not an id");
            return;
        };
        self.begin_for_loop(
            opargs.to_u32() as _,
            source,
            value_id,
            Some(index_id),
            Some(key_id),
        );
    }

    pub(crate) fn handle_loop(&mut self, opargs: OpcodeArgs) {
        self.begin_loop(opargs.to_u32() as _);
    }

    pub(crate) fn handle_for_end(&mut self) {
        self.end_for_loop();
    }

    pub(crate) fn handle_break(&mut self) {
        self.break_for_loop();
    }

    pub(crate) fn handle_breakifnot(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if !self.bx.heap.cast_to_bool(value) {
            self.break_for_loop();
        } else {
            self.bx.threads.cur().trap.goto_next();
        }
    }

    pub(crate) fn handle_continue(&mut self) {
        self.end_for_loop();
    }

    // Range handler

    pub(crate) fn handle_range(&mut self) {
        let end = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let start = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if !start.is_number() {
            let err = script_err_type_mismatch!(
                self.bx.threads.cur().trap,
                "range must start with a number, did you forget a , before a splat operator?"
            );
            self.bx.threads.cur().push_stack_unchecked(err);
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        if !end.is_number() {
            let err =
                script_err_type_mismatch!(self.bx.threads.cur().trap, "range end must be a number");
            self.bx.threads.cur().push_stack_unchecked(err);
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        let range = self
            .bx
            .heap
            .new_with_proto(self.bx.code.builtins.range.into());
        self.bx.heap.set_value_def(range, id!(start).into(), start);
        self.bx.heap.set_value_def(range, id!(end).into(), end);
        self.bx.threads.cur().push_stack_unchecked(range.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Is handler

    pub(crate) fn handle_is(&mut self) {
        let rhs = self.bx.threads.cur().pop_stack_value();
        let lhs = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let cmp = if let Some(id) = rhs.as_id() {
            let ty = lhs.value_type();
            let is_number = id == id!(number).into();
            match ty {
                // All numeric types match "is number"
                ScriptValueType::F64
                | ScriptValueType::F32
                | ScriptValueType::F16
                | ScriptValueType::U32
                | ScriptValueType::I32
                | ScriptValueType::U40 => is_number,
                ScriptValueType::NAN => is_number || id == id!(nan).into(),
                ScriptValueType::BOOL => id == id!(bool).into(),
                ScriptValueType::NIL => id == id!(nil).into(),
                ScriptValueType::COLOR => id == id!(color).into(),
                ScriptValueType::OBJECT => {
                    id == id!(object).into() || {
                        if let Some(rhs) = self
                            .bx
                            .threads
                            .cur()
                            .scope_value(&self.bx.heap, id)
                            .as_object()
                        {
                            if let Some(obj) = lhs.as_object() {
                                self.bx.heap.has_proto(obj, rhs.into())
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                }
                ScriptValueType::ARRAY => id == id!(array).into(),
                ScriptValueType::REGEX => id == id!(regex).into(),
                _ if ty.to_redux() == ScriptValueType::REDUX_STRING => id == id!(string).into(),
                _ if ty.to_redux() == ScriptValueType::REDUX_ID => id == id!(id).into(),
                _ => false,
            }
        } else if rhs.is_nil() {
            // `x is nil` where nil is the actual value
            lhs.is_nil()
        } else if let Some(obj) = lhs.as_object() {
            self.bx.heap.has_proto(obj, rhs)
        } else {
            false
        };
        self.bx.threads.cur().push_stack_unchecked(cmp.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Short-circuit evaluation handlers

    /// || short-circuit: if first operand is truthy, skip second operand and keep value
    pub(crate) fn handle_logic_or_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(value);
        if test {
            // First operand is truthy - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is falsy - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    /// && short-circuit: if first operand is falsy, skip second operand and keep value
    pub(crate) fn handle_logic_and_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(value);
        if !test {
            // First operand is falsy - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is truthy - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    /// |? short-circuit: if first operand is not nil, skip second operand and keep value
    pub(crate) fn handle_nil_or_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        if !value.is_nil() {
            // First operand is not nil - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is nil - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    // Try / OK handlers

    pub(crate) fn handle_ok_test(&mut self, opargs: OpcodeArgs) {
        let ip = self.bx.threads.cur_ref().trap.ip();
        let bases = self.bx.threads.cur_ref().new_bases();
        self.bx.threads.cur().tries.push(TryFrame {
            push_nil: true,
            start_ip: ip,
            jump: opargs.to_u32() + 1,
            bases,
        });
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_ok_end(&mut self) {
        if self.bx.threads.cur().tries.pop().is_none() {
            self.bail("tries empty in handle_ok_end");
            return;
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_try_test(&mut self, opargs: OpcodeArgs) {
        let ip = self.bx.threads.cur_ref().trap.ip();
        let bases = self.bx.threads.cur_ref().new_bases();
        self.bx.threads.cur().tries.push(TryFrame {
            push_nil: false,
            start_ip: ip,
            jump: opargs.to_u32() + 1,
            bases,
        });
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_try_err(&mut self, opargs: OpcodeArgs) {
        if self.bx.threads.cur().tries.pop().is_none() {
            self.bail("tries empty in handle_try_err");
            return;
        }
        // The parser encodes the success-path distance directly, including
        // the extra skip over TRY_OK only when a legacy `ok` branch exists.
        // An unconditional +1 here skipped the enclosing opcode (e.g. LET)
        // whenever there was no `ok` branch.
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
    }

    pub(crate) fn handle_try_ok(&mut self, opargs: OpcodeArgs) {
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
    }
}
