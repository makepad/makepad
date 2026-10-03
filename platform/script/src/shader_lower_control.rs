//! Lowering of control flow: if/else (with value phis), for, loop, break,
//! continue, return and the loop guards (shader_control.rs's rules).

use crate::opcode::*;
use crate::shader::ShaderType;
use crate::shader_control::{LOOP_GUARD_MAX_ITERS, SHADER_ITERATION_BUDGET};
use crate::shader_ir::*;
use crate::shader_lower::*;
use crate::shader_output::*;
use crate::shader_tables::*;
use crate::suggest::format_pod_type_name;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;

impl IrFnCompiler {
    pub(crate) fn charge_loop_cost(&mut self, cost: u64) {
        match self.loop_frames.last_mut() {
            Some(frame) => frame.body = frame.body.saturating_add(cost),
            None => self.fn_cost = self.fn_cost.saturating_add(cost),
        }
    }

    pub(crate) fn static_cost(&self) -> u64 {
        self.fn_cost.saturating_add(1)
    }

    fn open_loop_frame(&mut self, literal: Option<u64>) -> ExprId {
        let charge = self.expr(ExprKind::Lit(IrLit::U32(1)), TY_U32);
        self.loop_frames.push(IrLoopFrame { literal, charge, body: 1 });
        charge
    }

    fn close_loop_frame(&mut self) {
        let Some(frame) = self.loop_frames.pop() else {
            return;
        };
        let cost = match frame.literal {
            Some(n) => n.saturating_mul(frame.body),
            None => {
                let charge = frame.body.min(SHADER_ITERATION_BUDGET);
                self.f.exprs[frame.charge as usize] = Expr { kind: ExprKind::Lit(IrLit::U32(charge as u32)), ty: TY_U32 };
                frame.body
            }
        };
        self.charge_loop_cost(cost);
    }

    /// The guard every runtime-bounded loop runs at the top of each pass.
    fn write_loop_guard(&mut self, output: &mut ShaderOutput, guard: LocalId, charge: ExprId) {
        let g = self.expr(ExprKind::Local(guard), TY_U32);
        let g2 = self.expr(ExprKind::Local(guard), TY_U32);
        let one = self.expr(ExprKind::Lit(IrLit::U32(1)), TY_U32);
        let inc = self.expr(ExprKind::Binary(BinOp::Add, g2, one), TY_U32);
        self.emit(Stmt::Assign(g, inc));
        let iter = output.ir.global(IrGlobalKind::IterCounter, id!(_mp_iter), TY_U32);
        let it = self.expr(ExprKind::Global(iter), TY_U32);
        let it2 = self.expr(ExprKind::Global(iter), TY_U32);
        let add = self.expr(ExprKind::Binary(BinOp::Add, it2, charge), TY_U32);
        self.emit(Stmt::Assign(it, add));
        let g3 = self.expr(ExprKind::Local(guard), TY_U32);
        let cap = self.expr(ExprKind::Lit(IrLit::U32(LOOP_GUARD_MAX_ITERS)), TY_U32);
        let over1 = self.expr(ExprKind::Binary(BinOp::Gt, g3, cap), TY_BOOL);
        let it3 = self.expr(ExprKind::Global(iter), TY_U32);
        let budget = self.expr(ExprKind::Lit(IrLit::U32(SHADER_ITERATION_BUDGET as u32)), TY_U32);
        let over2 = self.expr(ExprKind::Binary(BinOp::Gt, it3, budget), TY_BOOL);
        let over = self.expr(ExprKind::Binary(BinOp::LogicOr, over1, over2), TY_BOOL);
        self.emit(Stmt::If(over, vec![Stmt::Break], Vec::new()));
    }

    pub(crate) fn is_unreachable(&self) -> bool {
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            match &self.mes[i] {
                IrMe::IfBody { has_return, .. } => {
                    if *has_return {
                        return true;
                    }
                }
                IrMe::FnBody { escaped, .. } => return *escaped,
                _ => {}
            }
        }
        false
    }

    pub(crate) fn is_parent_scope_unreachable(&self) -> bool {
        let mut skipped_first_if = false;
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            match &self.mes[i] {
                IrMe::IfBody { has_return, .. } => {
                    if !skipped_first_if {
                        skipped_first_if = true;
                        continue;
                    }
                    if *has_return {
                        return true;
                    }
                }
                IrMe::FnBody { escaped, .. } => return *escaped,
                _ => {}
            }
        }
        false
    }

    /// An outer if/else's phi (skipping the innermost if), marked assigned.
    fn find_and_mark_outer_phi(&mut self) -> Option<LocalId> {
        let mut skipped_first_if = false;
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            if let IrMe::IfBody { phi, phi_assigned_by_inner, .. } = &mut self.mes[i] {
                if !skipped_first_if {
                    skipped_first_if = true;
                    continue;
                }
                if let Some(phi) = phi {
                    *phi_assigned_by_inner = true;
                    return Some(*phi);
                }
            }
        }
        None
    }

    fn phi_local(&mut self) -> LocalId {
        let index = self.f.locals.len();
        self.f.local(LiveId(index as u64), 0, TY_VOID, true, LocalKind::Phi)
    }

    /// Assigns `val` to the phi: its type is settled when the if closes.
    fn assign_phi(&mut self, phi: LocalId, val: ExprId) {
        let target = self.expr(ExprKind::Local(phi), TY_VOID);
        self.emit(Stmt::Assign(target, val));
    }

    /// Gives a phi its type: the local, every read and write of it, and the
    /// abstract values assigned to it.
    fn settle_phi(&mut self, output: &mut ShaderOutput, phi: LocalId, ty: TyId) {
        self.f.locals[phi as usize].ty = ty;
        let scalar = output.ir.scalar_of(ty);
        let mut assigned = Vec::new();
        for i in 0..self.f.exprs.len() {
            if let ExprKind::Local(l) = self.f.exprs[i].kind {
                if l == phi {
                    self.f.exprs[i].ty = ty;
                }
            }
        }
        collect_assigned(&self.blocks, phi, &self.f.exprs, &mut assigned);
        for me in &self.mes {
            if let IrMe::IfBody { then_block: Some(t), .. } = me {
                collect_assigned_stmts(t, phi, &self.f.exprs, &mut assigned);
            }
        }
        for e in assigned {
            if let Some(s) = scalar {
                let et = self.f.exprs[e as usize].ty;
                if et == TY_AINT || et == TY_AFLOAT {
                    self.concretize(e, s);
                    if output.ir.lanes(ty) > 1 {
                        // A literal assigned to a vector phi is splatted.
                        let old = self.f.exprs[e as usize].clone();
                        let inner = self.f.expr(old.kind, old.ty);
                        self.f.exprs[e as usize] = Expr { kind: ExprKind::Construct(vec![inner]), ty };
                    }
                }
            }
        }
    }

    fn declare_phi(&mut self, phi: LocalId, ty: TyId) {
        let zero = self.expr(ExprKind::Construct(Vec::new()), ty);
        // The parent block is the one under the if's open branch.
        let n = self.blocks.len();
        if n >= 2 {
            self.blocks[n - 2].push(Stmt::Local(phi, zero));
        }
    }

    /// Closes the innermost if: its branches become one `If` in the parent.
    fn close_if_block(&mut self, cond: ExprId, then_block: Option<Vec<Stmt>>) {
        let last = self.blocks.pop().unwrap_or_default();
        let (then_b, else_b) = match then_block {
            Some(t) => (t, last),
            None => (last, Vec::new()),
        };
        self.emit(Stmt::If(cond, then_b, else_b));
    }

    pub(crate) fn handle_if_else_phi(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        loop {
            let should_handle = if let Some(IrMe::IfBody { target_ip, .. }) = self.mes.last() {
                self.trap.ip.index >= *target_ip
            } else {
                false
            };
            if !should_handle {
                break;
            }
            let (phi, stack_depth, phi_type, has_return, if_branch_returned, phi_assigned_by_inner) = match self.mes.last() {
                Some(IrMe::IfBody { phi, stack_depth, phi_type, has_return, if_branch_returned, phi_assigned_by_inner, .. }) => {
                    (*phi, *stack_depth, phi_type.clone(), *has_return, *if_branch_returned, *phi_assigned_by_inner)
                }
                _ => break,
            };
            let both_returned = if_branch_returned && has_return;
            let pods = vm.bx.code.builtins.pod.clone();
            let mut push_after: Option<(ShaderType, ExprId)> = None;

            if self.stack.types.len() > stack_depth {
                let (ty, val) = self.pop_resolved(vm, output);
                let else_is_void = ty.make_concrete(&pods).map(|t| t == pods.pod_void).unwrap_or(false);
                if else_is_void {
                    self.discard_value(val);
                } else if let (Some(phi), Some(phi_type)) = (phi, phi_type.clone()) {
                    let t = type_table_if_else(&phi_type, &ty, self.trap.pass(), &pods);
                    let t = t.make_concrete(&pods).unwrap_or(pods.pod_void);
                    if t != pods.pod_void {
                        self.assign_phi(phi, val);
                        let ir_t = Self::ir_ty(vm, output, t);
                        self.settle_phi(output, phi, ir_t);
                        self.declare_phi(phi, ir_t);
                        let e = self.expr(ExprKind::Local(phi), ir_t);
                        push_after = Some((ShaderType::Pod(t), e));
                    }
                } else if phi.is_none() {
                    match self.find_and_mark_outer_phi() {
                        Some(outer) => self.assign_phi(outer, val),
                        None => self.discard_value(val),
                    }
                }
            } else if let (Some(phi), Some(phi_type)) = (phi, phi_type.clone()) {
                let t = phi_type.make_concrete(&pods).unwrap_or(pods.pod_void);
                if t != pods.pod_void {
                    let ir_t = Self::ir_ty(vm, output, t);
                    self.settle_phi(output, phi, ir_t);
                    self.declare_phi(phi, ir_t);
                    if phi_assigned_by_inner {
                        let e = self.expr(ExprKind::Local(phi), ir_t);
                        push_after = Some((ShaderType::Pod(t), e));
                    }
                }
            } else if has_return {
                self.skip_next_pop_to_me = true;
            }

            let me = self.mes.pop();
            if let Some(IrMe::IfBody { cond, then_block, .. }) = me {
                self.close_if_block(cond, then_block);
            }
            self.scope.exit();
            if let Some((t, e)) = push_after {
                self.push(t, e);
            }
            if both_returned {
                self.mark_parent_returned();
            }
        }
    }

    fn mark_parent_returned(&mut self) {
        if let Some(parent) = self.mes.last_mut() {
            match parent {
                IrMe::IfBody { has_return, .. } => *has_return = true,
                IrMe::FnBody { escaped, .. } => *escaped = true,
                _ => {}
            }
        }
    }

    pub(crate) fn handle_if_test(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let (ty, cond) = self.pop();
        let (_ty, cond) = self.resolve_value(vm, output, ty, cond);
        self.blocks.push(Vec::new());
        self.scope.enter();
        self.mes.push(IrMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            stack_depth: self.stack.types.len(),
            cond,
            then_block: None,
            phi: None,
            phi_type: None,
            has_return: false,
            if_branch_returned: false,
            phi_assigned_by_inner: false,
            created_unreachable: false,
        });
    }

    pub(crate) fn handle_if_test_unreachable(&mut self, opargs: OpcodeArgs) {
        self.mes.push(IrMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            stack_depth: self.stack.types.len(),
            cond: NO_EXPR,
            then_block: None,
            phi: None,
            phi_type: None,
            has_return: true,
            if_branch_returned: false,
            phi_assigned_by_inner: false,
            created_unreachable: true,
        });
    }

    pub(crate) fn handle_if_else(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let popped = if let Some(IrMe::IfBody { stack_depth, .. }) = self.mes.last() {
            if self.stack.types.len() > *stack_depth {
                Some(self.pop_resolved(vm, output))
            } else {
                None
            }
        } else {
            None
        };
        if !matches!(self.mes.last(), Some(IrMe::IfBody { .. })) {
            script_err_unexpected!(self.trap, "unexpected in shader control");
            return;
        }
        if let Some((ty, val)) = popped {
            let pods = &vm.bx.code.builtins.pod;
            let is_void = ty.make_concrete(pods).map(|t| t == pods.pod_void).unwrap_or(false);
            if is_void {
                self.discard_value(val);
            } else {
                let existing = match self.mes.last() {
                    Some(IrMe::IfBody { phi, .. }) => *phi,
                    _ => None,
                };
                let phi = match existing {
                    Some(p) => p,
                    None => self.phi_local(),
                };
                if let Some(IrMe::IfBody { phi: p, phi_type, .. }) = self.mes.last_mut() {
                    *p = Some(phi);
                    *phi_type = Some(ty);
                }
                self.assign_phi(phi, val);
            }
        }
        let then_block = self.blocks.pop().unwrap_or_default();
        self.blocks.push(Vec::new());
        self.scope.exit();
        self.scope.enter();
        let ip = self.trap.ip.index;
        if let Some(IrMe::IfBody { target_ip, has_return, if_branch_returned, then_block: tb, .. }) = self.mes.last_mut() {
            *tb = Some(then_block);
            *target_ip = ip + opargs.to_u32();
            *if_branch_returned = *has_return;
            *has_return = false;
        }
    }

    pub(crate) fn handle_if_else_unreachable(&mut self, opargs: OpcodeArgs) {
        let ip = self.trap.ip.index;
        if let Some(IrMe::IfBody { target_ip, has_return, if_branch_returned, .. }) = self.mes.last_mut() {
            *target_ip = ip + opargs.to_u32();
            *if_branch_returned = *has_return;
            *has_return = true;
        }
    }

    pub(crate) fn handle_if_else_phi_unreachable(&mut self) {
        let (reached, both_returned, created_unreachable) = match self.mes.last() {
            Some(IrMe::IfBody { target_ip, has_return, if_branch_returned, created_unreachable, .. }) => {
                (self.trap.ip.index >= *target_ip, *if_branch_returned && *has_return, *created_unreachable)
            }
            _ => return,
        };
        if !reached {
            return;
        }
        let me = self.mes.pop();
        if !created_unreachable {
            if let Some(IrMe::IfBody { cond, then_block, phi, .. }) = me {
                // A phi assigned before the branch that left: declare it so
                // its writes stay valid.
                if let Some(phi) = phi {
                    if self.f.locals[phi as usize].ty != TY_VOID {
                        let ty = self.f.locals[phi as usize].ty;
                        self.declare_phi(phi, ty);
                    }
                }
                self.close_if_block(cond, then_block);
            }
            self.scope.exit();
        }
        if both_returned {
            self.mark_parent_returned();
        }
    }

    pub(crate) fn handle_return(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        let mut already_escaped = false;
        let mut fn_stack_depth = 0;
        let mut known_ret = None;
        let mut inside_if = false;
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            match &self.mes[i] {
                IrMe::FnBody { escaped, stack_depth, ret } => {
                    already_escaped = *escaped;
                    fn_stack_depth = *stack_depth;
                    known_ret = *ret;
                    break;
                }
                IrMe::IfBody { .. } => inside_if = true,
                _ => {}
            }
        }
        let no_value = opargs.is_nil() || self.stack.types.len() <= fn_stack_depth;
        if already_escaped {
            if !no_value {
                self.pop();
            }
            return;
        }
        let pods = vm.bx.code.builtins.pod.clone();
        let (ty, e) = if no_value {
            (pods.pod_void, NO_EXPR)
        } else {
            let (raw_ty, e) = self.pop_resolved(vm, output);
            let raw_ty = match known_ret {
                Some(r) if matches!(raw_ty, ShaderType::AbstractInt) && (r == pods.pod_u32 || r == pods.pod_f32 || r == pods.pod_f16) => {
                    let rt = Self::ir_ty(vm, output, r);
                    let s = output.ir.scalar_of(rt);
                    if let Some(s) = s {
                        self.concretize(e, s);
                    }
                    ShaderType::Pod(r)
                }
                _ => raw_ty,
            };
            let ty = raw_ty.make_concrete(&pods).unwrap_or(pods.pod_void);
            if ty != pods.pod_void {
                let target = Self::ir_ty(vm, output, ty);
                let e = self.coerce(vm, output, e, ty);
                let _ = target;
                (ty, e)
            } else {
                (ty, e)
            }
        };
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            if let IrMe::FnBody { ret, escaped, .. } = &mut self.mes[i] {
                if let Some(r) = ret {
                    if ty != *r {
                        script_err_inconsistent!(self.trap, "return type changed");
                    }
                }
                *ret = Some(ty);
                if !inside_if {
                    *escaped = true;
                }
                break;
            }
        }
        if ty == pods.pod_void {
            self.discard_value(e);
            self.emit(Stmt::Return(NO_EXPR));
        } else {
            self.emit(Stmt::Return(e));
        }
        let mut i = self.mes.len();
        while i > 0 {
            i -= 1;
            if let IrMe::IfBody { has_return, .. } = &mut self.mes[i] {
                *has_return = true;
                break;
            }
        }
    }

    pub(crate) fn handle_for_1(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (source, range_e) = self.pop();
        let (val_id, _) = self.pop();
        let ShaderType::Range { mut ty, .. } = source else {
            script_err_unexpected!(self.trap, "unexpected in shader control");
            return;
        };
        let ShaderType::Id(id) = val_id else {
            script_err_unexpected!(self.trap, "unexpected in shader control");
            return;
        };
        let pods = vm.bx.code.builtins.pod.clone();
        if ty == pods.pod_i32 {
            ty = pods.pod_u32;
        }
        if ty != pods.pod_u32 {
            script_err_type_mismatch!(
                self.trap,
                "shader for loop only supports u32 range, got {}",
                format_pod_type_name(&vm.bx.heap, ty)
            );
        }
        let (start, end) = match self.f.exprs.get(range_e as usize).map(|e| e.kind.clone()) {
            Some(ExprKind::Range(s, e)) => (s, e),
            _ => {
                script_err_unexpected!(self.trap, "unexpected in shader control");
                return;
            }
        };
        let start = self.to_u32(start);
        let end = self.to_u32(end);
        // A literal bound small enough to trust needs no guard.
        let literal = match self.f.exprs[end as usize].kind {
            ExprKind::Lit(IrLit::U32(n)) if n <= LOOP_GUARD_MAX_ITERS => Some(n as u64),
            _ => None,
        };
        let mut guard = None;
        let end = if literal.is_none() {
            // A runtime bound is evaluated once, before the loop.
            let end_local = self.f.local(id!(_mp_end), 0, TY_U32, false, LocalKind::LoopEnd);
            self.emit(Stmt::Local(end_local, end));
            let g = self.f.local(id!(_mp_loop_guard), 0, TY_U32, true, LocalKind::Guard);
            let zero = self.expr(ExprKind::Lit(IrLit::U32(0)), TY_U32);
            self.emit(Stmt::Local(g, zero));
            guard = Some(g);
            self.expr(ExprKind::Local(end_local), TY_U32)
        } else {
            end
        };
        self.scope.enter();
        let var = self.f.local(id, 0, TY_U32, false, LocalKind::User);
        self.scope.define(id, IrVar::Local(var, ty, false));
        self.blocks.push(Vec::new());
        let charge = self.open_loop_frame(literal);
        if let Some(g) = guard {
            self.write_loop_guard(output, g, charge);
        }
        self.mes.push(IrMe::ForLoop {
            var,
            start,
            end,
            stack_depth: self.stack.types.len(),
        });
    }

    /// A loop bound as u32.
    fn to_u32(&mut self, e: ExprId) -> ExprId {
        if e == NO_EXPR {
            return self.expr(ExprKind::Lit(IrLit::U32(0)), TY_U32);
        }
        let ty = self.f.exprs[e as usize].ty;
        if ty == TY_AINT || ty == TY_AFLOAT {
            self.concretize(e, IrScalar::U32);
            return e;
        }
        if ty == TY_U32 {
            return e;
        }
        self.expr(ExprKind::Convert(e), TY_U32)
    }

    pub(crate) fn handle_for_end(&mut self) {
        match self.mes.pop() {
            Some(IrMe::ForLoop { var, start, end, .. }) => {
                let body = self.blocks.pop().unwrap_or_default();
                self.emit(Stmt::For(var, start, end, body));
                self.scope.exit();
                self.close_loop_frame();
            }
            Some(IrMe::LoopBody { .. }) => {
                let body = self.blocks.pop().unwrap_or_default();
                self.emit(Stmt::Loop(body));
                self.scope.exit();
                self.close_loop_frame();
            }
            _ => {
                script_err_unexpected!(self.trap, "unexpected in shader control");
            }
        }
    }

    pub(crate) fn handle_loop(&mut self, _vm: &mut ScriptVm, output: &mut ShaderOutput) {
        self.scope.enter();
        let g = self.f.local(id!(_mp_loop_guard), 0, TY_U32, true, LocalKind::Guard);
        let zero = self.expr(ExprKind::Lit(IrLit::U32(0)), TY_U32);
        self.emit(Stmt::Local(g, zero));
        self.blocks.push(Vec::new());
        let charge = self.open_loop_frame(None);
        self.write_loop_guard(output, g, charge);
        self.mes.push(IrMe::LoopBody {
            stack_depth: self.stack.types.len(),
        });
    }

    pub(crate) fn handle_breakifnot(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (ty, cond) = self.pop();
        let (_ty, cond) = self.resolve_value(vm, output, ty, cond);
        let not = self.expr(ExprKind::Unary(UnOp::Not, cond), TY_BOOL);
        self.emit(Stmt::If(not, vec![Stmt::Break], Vec::new()));
    }

    pub(crate) fn handle_range(&mut self, vm: &mut ScriptVm) {
        let (end_ty, end) = self.pop();
        let (start_ty, start) = self.pop();
        let pods = vm.bx.code.builtins.pod.clone();
        // Bounds are names or values; names resolve through the scope.
        let (start_ty, start) = match start_ty {
            ShaderType::Id(id) => match self.scope.find(id) {
                Some(v) => (ShaderType::Pod(v.pod()), start),
                None => (start_ty, start),
            },
            _ => (start_ty, start),
        };
        let (end_ty, end) = match end_ty {
            ShaderType::Id(id) => match self.scope.find(id) {
                Some(v) => (ShaderType::Pod(v.pod()), end),
                None => (end_ty, end),
            },
            _ => (end_ty, end),
        };
        let start_concrete = start_ty.make_concrete(&pods);
        let end_concrete = end_ty.make_concrete(&pods);
        if let (Some(start_pod), Some(end_pod)) = (start_concrete, end_concrete) {
            let start_is_number = vm.bx.heap.pod_type_ref(start_pod).ty.is_number();
            let end_is_number = vm.bx.heap.pod_type_ref(end_pod).ty.is_number();
            if !start_is_number || !end_is_number {
                script_err_type_mismatch!(self.trap, "range requires numbers");
                return;
            }
            let e = self.expr(ExprKind::Range(start, end), TY_VOID);
            self.push(ShaderType::Range { start: String::new(), end: String::new(), ty: start_pod }, e);
        } else {
            script_err_type_mismatch!(self.trap, "range requires numbers");
        }
    }
}

/// The values assigned to `phi` in the open blocks (the if being closed
/// lies in the top ones).
fn collect_assigned(blocks: &[Vec<Stmt>], phi: LocalId, exprs: &[Expr], out: &mut Vec<ExprId>) {
    for b in blocks {
        collect_assigned_stmts(b, phi, exprs, out);
    }
}

fn collect_assigned_stmts(stmts: &[Stmt], phi: LocalId, exprs: &[Expr], out: &mut Vec<ExprId>) {
    for s in stmts {
        match s {
            Stmt::Assign(t, v) => {
                if let ExprKind::Local(l) = exprs[*t as usize].kind {
                    if l == phi {
                        out.push(*v);
                    }
                }
            }
            Stmt::If(_, a, b) => {
                collect_assigned_stmts(a, phi, exprs, out);
                collect_assigned_stmts(b, phi, exprs, out);
            }
            Stmt::For(_, _, _, b) | Stmt::Loop(b) => collect_assigned_stmts(b, phi, exprs, out),
            _ => {}
        }
    }
}
